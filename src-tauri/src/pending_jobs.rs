//! Незавершённые задания между запусками (`pending-jobs.json` в папке данных программы):
//! программу закрыли или Mac выключился посреди очереди — при следующем запуске «Не закончили
//! прошлый раз: N приложений — Продолжить / Забыть». Сами не начинаются никогда.
//!
//! В файле — живые задания нынешнего Apple ID и «отложенные»: прочитанные из файла и ещё не
//! взятые в работу (`Продолжить`) или убранные сменой Apple ID. Задания другого Apple ID лежат,
//! пока в него не войдут и не нажмут «Продолжить» или «Забыть». Пишется атомарно, только когда
//! список изменился (прогресс скачивания файл не трогает).
use crate::accounts::same_id;
use crate::queue::{JobState, Place};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

/// Задание очереди, которое стоит продолжить после перезапуска.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingJob {
    pub track_id: i64,
    pub name: String,
    /// «Только скачать»: в «Мои приложения», на телефон не ставить.
    pub download_only: bool,
    /// Выбранная версия (external version id); `None` — последняя.
    pub version: Option<String>,
    /// Телефон, на который ставили; `None` — телефон не был выбран.
    pub udid: Option<String>,
    /// Apple ID, с которого скачивали: задания другого Apple ID без него не продолжаются.
    pub account: String,
    /// Номер постановки (`PendingStore::next_seq`): после перезапуска — тот же порядок очереди.
    #[serde(default)]
    pub seq: u64,
    /// Примерный размер (Lookup) на момент постановки: «осталось ≈ N мин» после «Продолжить».
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Что программа помнит о задании с постановки (`start`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    /// Номер постановки (`PendingStore::next_seq`).
    pub seq: u64,
    pub size_bytes: Option<u64>,
    /// Телефон задания при постановке (цель очереди тогда); у «Только скачать» — нет.
    pub udid: Option<String>,
    /// Ставить только на `udid` («Продолжить» прошлый раз): другой телефон его не возьмёт.
    pub pinned: bool,
}

/// Куда ставить скачанное. Продолженное задание — только на свой телефон; его сейчас нет —
/// остановиться на «Сохранено» (поставится, когда он станет целью). Обычное — на цель очереди.
pub fn place(m: &Meta, download_only: bool, connected: &[String]) -> Place {
    match (&m.udid, m.pinned) {
        _ if download_only => Place::Hold,
        (Some(u), true) if connected.contains(u) => Place::Phone(u.clone()),
        (_, true) => Place::Hold,
        _ => Place::Target,
    }
}

/// Размер для «Продолжить»: записанный при постановке, иначе из нынешнего кэша Lookup.
pub fn size_of(j: &PendingJob, sizes: &HashMap<i64, u64>) -> Option<u64> {
    j.size_bytes.or_else(|| sizes.get(&j.track_id).copied())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileData {
    jobs: Vec<PendingJob>,
}

/// Задание ещё не кончилось: ждёт, качается, ставится или скачано и ждёт телефона.
/// Установлено, ошибка и «Сохранено» у «Только скачать» — итог.
pub fn unfinished(state: &JobState, download_only: bool) -> bool {
    match state {
        JobState::Queued | JobState::Downloading { .. } | JobState::Waiting { .. } | JobState::Paused | JobState::Installing { .. } => true,
        JobState::Saved { .. } => !download_only,
        JobState::Installed | JobState::Failed { .. } => false,
    }
}

fn same_job(a: &PendingJob, b: &PendingJob) -> bool {
    a.track_id == b.track_id && same_id(&a.account, &b.account)
}

/// Содержимое файла: живые задания и отложенные без повторов — живое главнее. Порядок
/// постоянный (номер постановки, затем аккаунт и trackId): одинаковый список не переписывает файл.
pub fn merge(unclaimed: &[PendingJob], live: &[PendingJob]) -> Vec<PendingJob> {
    let mut out: Vec<PendingJob> = live.to_vec();
    for u in unclaimed {
        if !out.iter().any(|j| same_job(j, u)) {
            out.push(u.clone());
        }
    }
    out.sort_by(|a, b| a.seq.cmp(&b.seq).then(a.account.to_lowercase().cmp(&b.account.to_lowercase())).then(a.track_id.cmp(&b.track_id)));
    out
}

/// Из чего строятся живые задания нынешнего Apple ID (`live`).
pub struct Live<'a> {
    pub jobs: &'a HashMap<i64, JobState>,
    pub names: &'a HashMap<i64, String>,
    pub download_only: &'a HashSet<i64>,
    pub versions: &'a HashMap<i64, String>,
    pub account: &'a str,
    /// Что запомнено при постановке: номер, размер, телефон.
    pub meta: &'a HashMap<i64, Meta>,
}

/// Незавершённые задания Apple ID по trackId. Установки из «Все копии» (отрицательный ключ)
/// не в счёт: они не от Apple ID и без телефона-цели не имеют смысла.
pub fn live(l: &Live) -> Vec<PendingJob> {
    let mut out: Vec<PendingJob> = l
        .jobs
        .iter()
        .filter(|(id, st)| **id > 0 && unfinished(st, l.download_only.contains(id)))
        .map(|(id, _)| PendingJob {
            track_id: *id,
            name: l.names.get(id).cloned().unwrap_or_default(),
            download_only: l.download_only.contains(id),
            version: l.versions.get(id).cloned(),
            udid: l.meta.get(id).and_then(|m| m.udid.clone()),
            account: l.account.to_string(),
            seq: l.meta.get(id).map_or(0, |m| m.seq),
            size_bytes: l.meta.get(id).and_then(|m| m.size_bytes),
        })
        .collect();
    out.sort_by_key(|j| (j.seq, j.track_id));
    out
}

/// Пачка для `start` при «Продолжить»: «Только скачать» или нет, (trackId, имя), (trackId, версия).
pub type Batch = (bool, Vec<(i64, String)>, Vec<(i64, String)>);

/// «Продолжить»: задания по пачкам `start` в исходном порядке постановки; подряд идущие одного
/// режима — одна пачка.
pub fn batches(jobs: &[PendingJob]) -> Vec<Batch> {
    let mut sorted: Vec<&PendingJob> = jobs.iter().collect();
    sorted.sort_by_key(|j| (j.seq, j.track_id));
    let mut out: Vec<Batch> = vec![];
    for j in sorted {
        if out.last().is_none_or(|b| b.0 != j.download_only) {
            out.push((j.download_only, vec![], vec![]));
        }
        let b = out.last_mut().expect("just pushed");
        b.1.push((j.track_id, j.name.clone()));
        if let Some(v) = &j.version {
            b.2.push((j.track_id, v.clone()));
        }
    }
    out
}


/// Разбор файла. Битый — пустой список; записи без Apple ID или с неверным trackId выпадают.
fn parse(text: &str) -> Vec<PendingJob> {
    serde_json::from_str::<FileData>(text)
        .map(|d| d.jobs)
        .unwrap_or_default()
        .into_iter()
        .filter(|j| j.track_id > 0 && !j.account.trim().is_empty())
        .collect()
}

#[derive(Default)]
struct Inner {
    /// Из файла (или убранные сменой Apple ID) и ещё не взятые в работу.
    unclaimed: Vec<PendingJob>,
    /// Что сейчас в файле: тот же список второй раз не пишется.
    written: Vec<PendingJob>,
}

pub struct PendingStore {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
    /// Следующий номер постановки: после записанных в файле.
    seq: std::sync::atomic::AtomicU64,
}

impl PendingStore {
    /// Читает файл; нет его или он битый — пусто.
    pub fn open(path: PathBuf) -> Self {
        let jobs = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        let next = jobs.iter().map(|j| j.seq).max().unwrap_or(0) + 1;
        Self { path: Some(path), inner: Mutex::new(Inner { unclaimed: jobs.clone(), written: jobs }), seq: next.into() }
    }

    /// Номер постановки нового задания: растёт и между запусками.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Отложенные задания Apple ID `email`, кроме тех, что уже идут (`live`): для баннера.
    pub fn offered(&self, email: &str, live: &HashSet<i64>) -> Vec<PendingJob> {
        let inner = self.inner.lock().unwrap();
        inner.unclaimed.iter().filter(|j| same_id(&j.account, email) && !live.contains(&j.track_id)).cloned().collect()
    }

    /// «Продолжить»: отложенные задания `email` (кроме уже идущих `live`) забираются в работу.
    pub fn claim(&self, email: &str, live: &HashSet<i64>) -> Vec<PendingJob> {
        let mut inner = self.inner.lock().unwrap();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut inner.unclaimed).into_iter().partition(|j| same_id(&j.account, email));
        inner.unclaimed = rest;
        mine.into_iter().filter(|j| !live.contains(&j.track_id)).collect()
    }

    /// «Забыть» (баннер или сам Apple ID): отложенные задания `email` больше не предлагаются.
    pub fn forget(&self, email: &str) {
        self.inner.lock().unwrap().unclaimed.retain(|j| !same_id(&j.account, email));
    }

    /// Задания `ids` Apple ID `email` начаты заново («Вернуть»): отложенные копии не нужны —
    /// иначе после их конца они снова всплыли бы в баннере.
    pub fn unclaim(&self, email: &str, ids: &HashSet<i64>) {
        self.inner.lock().unwrap().unclaimed.retain(|j| !(same_id(&j.account, email) && ids.contains(&j.track_id)));
    }

    /// Смена Apple ID убрала ждущие задания из очереди: они остаются в файле до возвращения в него.
    pub fn release(&self, jobs: Vec<PendingJob>) {
        let mut inner = self.inner.lock().unwrap();
        for j in jobs {
            if let Some(old) = inner.unclaimed.iter_mut().find(|u| same_job(u, &j)) {
                *old = j;
            } else {
                inner.unclaimed.push(j);
            }
        }
    }

    /// Записать живые задания (`live`) вместе с отложенными, если список изменился.
    #[cfg(test)]
    pub fn sync(&self, live: &[PendingJob]) {
        self.sync_with(|| Some(live.to_vec()));
    }

    /// `sync` со снимком очереди, который `snapshot` снимает уже под замком файла: два
    /// одновременных вызова пишут в том порядке, в каком снимали, — старый снимок не ляжет поверх
    /// нового. `None` — писать нечего (нет входа). `snapshot` не должен трогать этот `PendingStore`.
    pub fn sync_with(&self, snapshot: impl FnOnce() -> Option<Vec<PendingJob>>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(live) = snapshot() else { return };
        let all = merge(&inner.unclaimed, &live);
        if all == inner.written {
            return;
        }
        if let Some(p) = &self.path {
            let r = serde_json::to_vec(&FileData { jobs: all.clone() }).map_err(|e| e.to_string()).and_then(|b| crate::accounts::write_atomic(p, &b));
            if let Err(e) = r {
                crate::log::write("pending jobs", &e);
                return;
            }
        }
        inner.written = all;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::AppError;

    fn job(id: i64, account: &str) -> PendingJob {
        PendingJob { track_id: id, name: format!("App {id}"), download_only: false, version: None, udid: Some("UDID-1".into()), account: account.into(), seq: id as u64, size_bytes: None }
    }

    fn ids(list: &[PendingJob]) -> Vec<i64> {
        list.iter().map(|j| j.track_id).collect()
    }

    #[test]
    fn live_takes_unfinished_account_jobs_with_their_settings() {
        let jobs: HashMap<i64, JobState> = [
            (3, JobState::Downloading { bytes: 5 }),
            (1, JobState::Queued),
            (2, JobState::Installed),
            (4, JobState::Saved { path: "/4.ipa".into() }),
            (-7, JobState::Queued),
        ]
        .into();
        let names = HashMap::from([(1, "Банк".to_string()), (3, "Такси".to_string()), (4, "Карты".to_string())]);
        let only = HashSet::from([4]);
        let versions = HashMap::from([(3, "812".to_string())]);
        let meta = HashMap::from([
            (1, Meta { seq: 8, size_bytes: Some(300), udid: Some("U1".into()), pinned: false }),
            (3, Meta { seq: 4, size_bytes: None, udid: Some("U3".into()), pinned: true }),
        ]);
        let got = live(&Live { jobs: &jobs, names: &names, download_only: &only, versions: &versions, account: "me@example.com", meta: &meta });
        assert_eq!(ids(&got), [3, 1], "итоги, «Сохранено» у «Только скачать» и копии — не в счёт; порядок постановки");
        let got: Vec<PendingJob> = got.into_iter().rev().collect();
        assert_eq!(got[0].name, "Банк");
        assert_eq!(got[1].version.as_deref(), Some("812"));
        assert_eq!((got[0].size_bytes, got[1].size_bytes), (Some(300), None));
        // телефон — свой у каждого задания, с постановки, а не нынешняя цель
        assert_eq!((got[0].udid.as_deref(), got[1].udid.as_deref()), (Some("U1"), Some("U3")));
        assert!(got.iter().all(|j| j.account == "me@example.com"));
    }

    fn with_seq(mut j: PendingJob, seq: u64) -> PendingJob {
        j.seq = seq;
        j
    }

    /// Пачки идут в исходном порядке постановки: подряд одного режима — одна пачка.
    #[test]
    fn batches_keep_the_original_order() {
        let mut a = with_seq(job(1, "me@example.com"), 10);
        a.version = Some("5".into());
        let mut b = with_seq(job(2, "me@example.com"), 11);
        b.download_only = true;
        let c = with_seq(job(3, "me@example.com"), 12);
        let d = with_seq(job(9, "me@example.com"), 13);
        let got = batches(&[d, b, c, a]);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (false, vec![(1, "App 1".into())], vec![(1, "5".into())]));
        assert_eq!(got[1], (true, vec![(2, "App 2".into())], vec![]));
        assert_eq!(got[2], (false, vec![(3, "App 3".into()), (9, "App 9".into())], vec![]));
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn file_keeps_queue_order_and_numbering_continues() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("p.json");
        let s = PendingStore::open(path.clone());
        let first = s.next_seq();
        assert!(s.next_seq() > first);
        // поставили 30, потом 5: в файле и после перезапуска — тот же порядок, не по trackId
        s.sync(&[with_seq(job(30, "me@example.com"), 1), with_seq(job(5, "me@example.com"), 2)]);
        let s = PendingStore::open(path);
        assert_eq!(ids(&s.offered("me@example.com", &HashSet::new())), [30, 5]);
        assert!(s.next_seq() > 2, "новые номера — после записанных");
    }

    #[test]
    fn size_from_enqueue_or_lookup_cache() {
        let mut a = job(1, "me@example.com");
        a.size_bytes = Some(10);
        let b = job(2, "me@example.com");
        let sizes = HashMap::from([(1, 99), (2, 20)]);
        assert_eq!(size_of(&a, &sizes), Some(10));
        assert_eq!(size_of(&b, &sizes), Some(20));
        assert_eq!(size_of(&b, &HashMap::new()), None);
        // старый файл без размера читается
        let old = r#"{"jobs":[{"track_id":5,"name":"A","download_only":false,"version":null,"udid":null,"account":"me@example.com"}]}"#;
        assert_eq!(parse(old)[0].size_bytes, None);
    }

    /// Куда ставить: продолженное — только на свой телефон, его нет — «Сохранено» (не на чужой).
    #[test]
    fn place_never_picks_another_phone_for_a_pinned_job() {
        let connected = ["U2".to_string()];
        let pinned = |u: &str| Meta { udid: Some(u.into()), pinned: true, ..Meta::default() };
        assert_eq!(place(&pinned("U2"), false, &connected), Place::Phone("U2".into()));
        assert_eq!(place(&pinned("U9"), false, &connected), Place::Hold, "телефона задания нет — ждёт его");
        // обычное задание — на цель очереди, как раньше
        let normal = Meta { udid: Some("U9".into()), ..Meta::default() };
        assert_eq!(place(&normal, false, &connected), Place::Target);
        assert_eq!(place(&pinned("U2"), true, &connected), Place::Hold, "«Только скачать» не ставит");
        assert_eq!(place(&Meta::default(), true, &connected), Place::Hold);
    }

    #[test]
    fn unfinished_states() {
        for st in [JobState::Queued, JobState::Downloading { bytes: 1 }, JobState::Waiting { seconds: 2 }, JobState::Paused, JobState::Installing { percent: 3 }] {
            assert!(unfinished(&st, false) && unfinished(&st, true), "{st:?}");
        }
        let saved = JobState::Saved { path: "/x.ipa".into() };
        assert!(unfinished(&saved, false), "скачано, ждёт телефона");
        assert!(!unfinished(&saved, true), "«Только скачать»: сохранено — итог");
        assert!(!unfinished(&JobState::Installed, false));
        assert!(!unfinished(&JobState::Failed { error: AppError::Timeout, saved: None }, false));
    }

    #[test]
    fn merge_prefers_live_and_keeps_other_accounts() {
        let mut live = job(1, "me@example.com");
        live.version = Some("777".into());
        let unclaimed = vec![job(1, "ME@example.com"), job(2, "me@example.com"), job(1, "other@example.com")];
        let all = merge(&unclaimed, &[live.clone()]);
        assert_eq!(all.len(), 3);
        assert!(all.contains(&live), "живое главнее отложенного того же Apple ID");
        assert!(all.contains(&job(1, "other@example.com")));
        // порядок не зависит от порядка входа
        assert_eq!(merge(&unclaimed.iter().rev().cloned().collect::<Vec<_>>(), &[live]), all);
    }

    #[test]
    fn parse_drops_garbage() {
        assert!(parse("not json").is_empty());
        assert!(parse("{}").is_empty());
        let text = serde_json::to_string(&FileData { jobs: vec![job(1, "a@example.com"), job(-5, "a@example.com"), job(2, " ")] }).unwrap();
        assert_eq!(ids(&parse(&text)), [1]);
    }

    #[test]
    fn file_survives_restart_and_other_accounts_wait() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("pending-jobs.json");
        let s = PendingStore::open(path.clone());
        s.sync(&[job(1, "me@example.com"), job(2, "me@example.com")]);
        s.release(vec![job(9, "other@example.com")]);
        s.sync(&[job(1, "me@example.com"), job(2, "me@example.com")]);
        // «перезапуск»
        let s = PendingStore::open(path.clone());
        assert_eq!(ids(&s.offered("ME@example.com", &HashSet::new())), [1, 2]);
        assert_eq!(ids(&s.offered("me@example.com", &HashSet::from([2]))), [1], "идущее не предлагается");
        assert_eq!(ids(&s.offered("other@example.com", &HashSet::new())), [9]);
        // «Продолжить» у me: забраны в работу, other ждёт
        let got = s.claim("me@example.com", &HashSet::new());
        assert_eq!(ids(&got), [1, 2]);
        assert!(s.offered("me@example.com", &HashSet::new()).is_empty());
        s.sync(&[job(2, "me@example.com")]); // 1 доделано
        let s = PendingStore::open(path.clone());
        assert_eq!(ids(&s.offered("me@example.com", &HashSet::new())), [2]);
        assert_eq!(ids(&s.offered("other@example.com", &HashSet::new())), [9]);
        // «Забыть» у other
        s.forget("other@example.com");
        s.sync(&[]);
        let s = PendingStore::open(path);
        assert!(s.offered("other@example.com", &HashSet::new()).is_empty());
        assert_eq!(ids(&s.offered("me@example.com", &HashSet::new())), [2], "отложенное me не тронуто");
    }

    /// Снимок очереди берётся под замком файла: старый снимок, снятый раньше, не запишется
    /// поверх нового (иначе законченное задание снова всплыло бы после перезапуска).
    #[test]
    fn older_snapshot_never_overwrites_a_newer_one() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("p.json");
        let s = std::sync::Arc::new(PendingStore::open(path.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let s2 = s.clone();
        let a = std::thread::spawn(move || {
            s2.sync_with(|| {
                tx.send(()).unwrap(); // снимок «старый» снят
                std::thread::sleep(std::time::Duration::from_millis(150));
                Some(vec![job(1, "me@example.com")])
            })
        });
        rx.recv().unwrap();
        s.sync_with(|| Some(vec![])); // задание 1 кончилось — снимок новее
        a.join().unwrap();
        assert!(PendingStore::open(path).offered("me@example.com", &HashSet::new()).is_empty());
    }

    /// Пока список тот же, файл не переписывается (прогресс скачивания идёт дважды в секунду).
    #[test]
    fn same_list_is_not_rewritten() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("pending-jobs.json");
        let s = PendingStore::open(path.clone());
        s.sync(&[job(1, "me@example.com")]);
        std::fs::remove_file(&path).unwrap();
        s.sync(&[job(1, "me@example.com")]);
        assert!(!path.exists());
        s.sync(&[]);
        assert!(path.exists());
        assert!(!t.path().join(".pending-jobs.json.app-restore.tmp").exists(), "атомарная запись прибрала за собой");
    }

    /// Начатое заново («Вернуть») из отложенных убирается: после конца не всплывает в баннере.
    #[test]
    fn restarted_job_is_not_offered_after_it_finishes() {
        let t = tempfile::tempdir().unwrap();
        let s = PendingStore::open(t.path().join("p.json"));
        s.release(vec![job(1, "me@example.com"), job(2, "me@example.com")]);
        s.unclaim("me@example.com", &HashSet::from([1]));
        s.sync(&[]);
        assert_eq!(ids(&s.offered("me@example.com", &HashSet::new())), [2]);
    }

    /// Смена Apple ID: убранные задания остаются в файле, повтор того же — без дубля.
    #[test]
    fn release_replaces_the_same_job() {
        let t = tempfile::tempdir().unwrap();
        let s = PendingStore::open(t.path().join("p.json"));
        s.release(vec![job(1, "me@example.com")]);
        let mut newer = job(1, "me@example.com");
        newer.download_only = true;
        s.release(vec![newer.clone()]);
        assert_eq!(s.offered("me@example.com", &HashSet::new()), [newer]);
    }
}
