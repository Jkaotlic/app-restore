
//! Очередь: один рабочий поток делает скачивание, затем установку на телефон, строго
//! последовательно (следующее скачивание ждёт конца предыдущей установки).
//! Прогресс скачивания — опросом размера `.partial/<track_id>/`.
//! «Пауза» (`pause`): новые задания не начинаются; идущее скачивание обрывается (ipatool
//! останавливается, недокачанное удаляется) и после «Продолжить» (`resume`) начинается заново
//! первым. Идущую установку на телефон пауза не прерывает, установку копии на явно выбранный
//! телефон («Все копии») не держит.
use crate::device::DeviceHub;
use crate::errors::AppError;
use crate::ipatool::session::Session;
use crate::library::Library;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};

pub const MIN_FREE_MB: u64 = 500;
pub const POLL: Duration = Duration::from_millis(500);
/// Скачивание без роста файла столько — зависло (сеть пропала, после сна Mac ipatool часто
/// мёртв): ipatool останавливается, задание `DownloadStalled`
/// («Повторить» скачает заново), очередь идёт дальше.
pub const DOWNLOAD_IDLE: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Queued,
    Downloading { bytes: u64 },
    Saved { path: PathBuf },
    /// Установка на iPhone; `percent` сквозной: загрузка 0–50, установка 50–100.
    Installing { percent: u8 },
    Installed,
    /// `saved`: ipa уже в библиотеке (упала установка) — можно «Повторить» без скачивания.
    Failed { error: AppError, saved: Option<PathBuf> },
    /// Apple ответил 429/503: повтор скачивания через `seconds` (`retry::with_retry_notify`).
    Waiting { seconds: u64 },
    /// «Пауза» оборвала скачивание: после «Продолжить» оно начнётся заново.
    Paused,
}

/// Пауза повтора в целых секундах, с округлением вверх: «через 0 с» при паузе 300 мс не врёт.
fn wait_seconds(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

pub trait SpaceProbe: Send + Sync {
    fn available_bytes(&self, path: &Path) -> std::io::Result<u64>;
}

pub struct FsSpace;

impl SpaceProbe for FsSpace {
    fn available_bytes(&self, path: &Path) -> std::io::Result<u64> {
        fs2::available_space(path)
    }
}

/// Куда ставить скачанное.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// На цель очереди (`set_target`); её нет — остановиться на `Saved`.
    Target,
    /// Только на этот телефон («Продолжить» прошлый раз): цель очереди не в счёт.
    Phone(String),
    /// Не ставить: «Только скачать» или телефона задания сейчас нет — остановиться на `Saved`.
    Hold,
}

enum Job {
    /// Скачать и поставить по `Place`. Версия — выбранная человеком (external version id),
    /// `None` — последняя.
    Download(i64, Place, Option<String>),
    /// Телефон задан явно («Все копии») — он главнее цели очереди и её не меняет.
    Install(i64, PathBuf, Option<String>),
}

/// Установка на явно выбранный телефон («Все копии») идёт и на «Паузе»: пауза держит очередь
/// скачиваний, а не ручную установку копии.
fn bypasses_pause(job: &Job) -> bool {
    matches!(job, Job::Install(_, _, Some(_)))
}

/// Задания, которые ждут в канале (ещё не взяты в работу): (номер постановки, trackId). Под одним
/// замком: задание либо взято до `drop_pending` (и идёт), либо отброшено им — третьего нет.
#[derive(Default)]
struct Pending {
    next: u64,
    waiting: Vec<(u64, i64)>,
}

pub struct Queue {
    /// Задание и номер его постановки (`Pending`).
    tx: mpsc::UnboundedSender<(u64, Job)>,
    pending: Arc<Mutex<Pending>>,
    events: broadcast::Sender<(i64, JobState)>,
    target: Arc<Mutex<Option<String>>>,
    /// iOS подключённых телефонов (udid → ProductVersion): проверка перед установкой.
    ios: Arc<Mutex<HashMap<String, String>>>,
    /// «Пауза»: `true` — новые задания не начинаются, идущее скачивание обрывается.
    pause: watch::Sender<bool>,
}

/// Ставит сохранённую ipa на телефон; возвращает итоговое состояние. `Installed` только при `Ok`.
/// `ios` — iOS телефона, если известна: копия, которой нужна iOS новее, не ставится.
async fn run_install(
    hub: &dyn DeviceHub,
    udid: Option<String>,
    ios: Option<String>,
    path: &Path,
    emit: &(dyn Fn(JobState) + Send + Sync),
) -> JobState {
    let failed = |error| JobState::Failed { error, saved: Some(path.to_path_buf()) };
    let Some(udid) = udid else {
        return failed(AppError::DeviceNotFound { drivers_missing: false });
    };
    // Диск компьютера проверяем до телефона: его ошибка не вина iPhone.
    if let Err(e) = crate::device::open_local(path).await {
        return failed(e);
    }
    // Минимальная iOS из Info.plist копии — до телефона: иначе непонятный отказ installation_proxy.
    if ios.is_some() {
        let p = path.to_path_buf();
        let need = tokio::task::spawn_blocking(move || crate::library::read_min_os(&p)).await.ok().flatten();
        if let Err(e) = crate::ios_compat::check(need.as_deref(), ios.as_deref()) {
            return failed(e);
        }
    }
    emit(JobState::Installing { percent: 0 });
    let (ptx, mut prx) = mpsc::channel::<u8>(64);
    let install = hub.install(&udid, path, ptx);
    tokio::pin!(install);
    let result = loop {
        tokio::select! {
            r = &mut install => break r,
            Some(p) = prx.recv() => emit(JobState::Installing { percent: p.min(100) }),
        }
    };
    while let Ok(p) = prx.try_recv() {
        emit(JobState::Installing { percent: p.min(100) });
    }
    match result {
        Ok(()) => JobState::Installed,
        Err(error) => failed(error),
    }
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_file() => m.len(),
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Скачивание в библиотеку. `Ok(None)` — оборвала «Пауза» (недокачанное удалено).
async fn run_job(
    id: i64,
    version: Option<&str>,
    w: &Worker,
    mut pause: watch::Receiver<bool>,
    emit: &(dyn Fn(JobState) + Send + Sync),
) -> Result<Option<PathBuf>, AppError> {
    let (session, library, space, idle) = (&w.session, &w.library, w.space.as_ref(), w.idle);
    let _ = std::fs::create_dir_all(library.base());
    // Если свободное место определить не вышло, не блокируем скачивание.
    match space.available_bytes(library.base()) {
        Ok(avail) => {
            let avail_mb = avail / (1024 * 1024);
            if avail_mb < MIN_FREE_MB {
                return Err(AppError::NoDiskSpace { need_mb: (MIN_FREE_MB - avail_mb).max(1) });
            }
        }
        Err(e) => crate::log::write(&format!("job {id}"), &format!("free space unknown ({e}), downloading anyway")),
    }
    let partial = library.partial_dir(id)?;
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| AppError::Unknown { detail: format!("partial: {e}") })?;
    emit(JobState::Downloading { bytes: 0 });
    let result = {
        let started = std::sync::atomic::AtomicBool::new(false);
        let on_wait = |d: Duration| emit(JobState::Waiting { seconds: wait_seconds(d) });
        let download = session.download_marked(id, &partial, version, &started, &on_wait);
        tokio::pin!(download);
        let mut tick = tokio::time::interval(POLL);
        tick.tick().await; // первый тик срабатывает сразу
        let (mut last, mut grew) = (0u64, tokio::time::Instant::now());
        loop {
            tokio::select! {
                r = &mut download => break Some(r),
                // «Пауза»: `download` бросается ниже — ipatool убивается, как при простое
                Ok(_) = pause.wait_for(|p| *p) => break None,
                _ = tick.tick() => {
                    let bytes = dir_size(&partial);
                    // пока ждём очереди к ipatool или паузы повтора, часы простоя стоят, а прогресс
                    // не перебивает «Apple просит подождать»
                    if !started.load(std::sync::atomic::Ordering::SeqCst) {
                        grew = tokio::time::Instant::now();
                        continue;
                    } else if bytes != last {
                        (last, grew) = (bytes, tokio::time::Instant::now());
                    } else if grew.elapsed() >= idle {
                        // `download` бросается ниже: ipatool убивается, замок ipatool свободен
                        crate::log::write(&format!("job {id}"), &format!("download stalled at {bytes} bytes for {} s, stopped", idle.as_secs()));
                        break Some(Err(AppError::DownloadStalled));
                    }
                    emit(JobState::Downloading { bytes });
                }
            }
        }
    };
    let Some(result) = result else {
        let _ = std::fs::remove_dir_all(&partial);
        crate::log::write(&format!("job {id}"), "paused, download stopped");
        return Ok(None);
    };
    let saved = result.and_then(|file| library.commit(&partial, &file));
    let _ = std::fs::remove_dir_all(&partial);
    saved.map(Some)
}

/// Всё, чем рабочий поток делает задания.
#[derive(Clone)]
struct Worker {
    session: Arc<Session>,
    library: Arc<Library>,
    space: Arc<dyn SpaceProbe>,
    hub: Arc<dyn DeviceHub>,
    tgt: Arc<Mutex<Option<String>>>,
    ios: Arc<Mutex<HashMap<String, String>>>,
    idle: Duration,
}

/// Один job целиком: скачивание (если нужно), затем установка (если задан телефон).
/// Скачивание оборвала «Пауза» — задание возвращается: его поставят снова первым.
async fn process(job: Job, w: Worker, pause: watch::Receiver<bool>, emit: Arc<dyn Fn(JobState) + Send + Sync>) -> Option<Job> {
    // explicit: установку попросили прямо (`install_saved`) — без телефона это ошибка.
    let (saved, explicit) = match job {
        Job::Download(id, place, version) => match run_job(id, version.as_deref(), &w, pause, emit.as_ref()).await {
            Ok(None) => return Some(Job::Download(id, place, version)),
            Ok(Some(path)) => {
                emit(JobState::Saved { path: path.clone() });
                match place {
                    Place::Target => (Some((path, None)), false),
                    // свой телефон — как явная установка: его нет — ошибка с копией, не чужой телефон
                    Place::Phone(udid) => (Some((path, Some(udid))), true),
                    Place::Hold => (None, false),
                }
            }
            Err(error) => {
                emit(JobState::Failed { error, saved: None });
                (None, false)
            }
        },
        Job::Install(_, path, device) => (Some((path, device)), true),
    };
    let Worker { hub, tgt, ios, .. } = w;
    if let Some((path, device)) = saved {
        let udid = device.or_else(|| tgt.lock().unwrap().clone());
        // Без телефона скачанное остаётся в `Saved`; `install_saved` поставит позже.
        if udid.is_some() || explicit {
            let phone_ios = udid.as_ref().and_then(|u| ios.lock().unwrap().get(u).cloned());
            emit(run_install(hub.as_ref(), udid, phone_ios, &path, emit.as_ref()).await);
        }
    }
    None
}

impl Queue {
    /// Запускает единственный рабочий поток очереди; нужен tokio runtime.
    pub fn new(
        session: Arc<Session>,
        library: Arc<Library>,
        space: Arc<dyn SpaceProbe>,
        hub: Arc<dyn DeviceHub>,
    ) -> Self {
        Self::with_idle(session, library, space, hub, DOWNLOAD_IDLE)
    }

    /// `new` с другим сроком простоя скачивания (`DOWNLOAD_IDLE`).
    pub fn with_idle(
        session: Arc<Session>,
        library: Arc<Library>,
        space: Arc<dyn SpaceProbe>,
        hub: Arc<dyn DeviceHub>,
        idle: Duration,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, Job)>();
        let pending: Arc<Mutex<Pending>> = Arc::default();
        let current = pending.clone();
        let (events, _) = broadcast::channel(4096);
        let target: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let ev = events.clone();
        let ios: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let worker = Worker { session, library, space, hub, tgt: target.clone(), ios: ios.clone(), idle };
        let (pause, mut paused) = watch::channel(false);
        tokio::spawn(async move {
            // На паузе задания ждут здесь по порядку (прерванное «Паузой» — первым) и остаются в
            // `waiting`: `drop_pending` их видит. Установка на явный телефон паузу обходит.
            let mut held: std::collections::VecDeque<(u64, Job)> = Default::default();
            let mut closed = false;
            // второй приёмник — только читать флаг, пока первый ждёт «Продолжить» в `select!`
            let now_paused = paused.clone();
            loop {
                let is_paused = *paused.borrow();
                let next = if is_paused { None } else { held.pop_front() };
                let (seq, job) = match next {
                    Some(j) => j,
                    None if closed => {
                        if held.is_empty() || paused.wait_for(|p| !*p).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    None => tokio::select! {
                        r = rx.recv() => match r {
                            Some(j) if !*now_paused.borrow() || bypasses_pause(&j.1) => j,
                            Some(j) => {
                                held.push_back(j);
                                continue;
                            }
                            None => {
                                closed = true;
                                continue;
                            }
                        },
                        Ok(_) = paused.wait_for(|p| !*p), if is_paused => continue,
                    },
                };
                let id = match &job {
                    Job::Download(id, _, _) | Job::Install(id, _, _) => *id,
                };
                {
                    let mut p = current.lock().unwrap();
                    match p.waiting.iter().position(|(s, _)| *s == seq) {
                        Some(i) => {
                            p.waiting.remove(i);
                        }
                        None => continue, // сброшено `drop_pending`, его id уже отдан
                    }
                }
                let ev2 = ev.clone();
                let emit: Arc<dyn Fn(JobState) + Send + Sync> = Arc::new(move |s| {
                    let _ = ev2.send((id, s));
                });
                // Работа в отдельной задаче: паника не должна убить очередь.
                let work = tokio::spawn(process(job, worker.clone(), paused.clone(), emit.clone()));
                match work.await {
                    Ok(Some(job)) => {
                        // снова ждёт: до события, чтобы смена Apple ID уже видела его в `waiting`
                        current.lock().unwrap().waiting.push((seq, id));
                        held.push_front((seq, job));
                        emit(JobState::Paused);
                    }
                    Ok(None) => {}
                    Err(_) => emit(JobState::Failed { error: AppError::Unknown { detail: "internal error".into() }, saved: None }),
                }
            }
        });
        Self { tx, pending, events, target, ios, pause }
    }

    /// «Пауза»: новые задания не начинаются; идущее скачивание обрывается (`JobState::Paused`).
    pub fn pause(&self) {
        self.pause.send_replace(true);
    }

    /// «Продолжить»: прерванное скачивание начинается заново, за ним остальные.
    pub fn resume(&self) {
        self.pause.send_replace(false);
    }

    pub fn paused(&self) -> bool {
        *self.pause.borrow()
    }

    /// iOS подключённых телефонов (udid, ProductVersion) после опроса: по ней установка
    /// отказывает копии, которой нужна iOS новее. Прежние записи заменяются.
    pub fn set_phone_versions(&self, phones: Vec<(String, String)>) {
        *self.ios.lock().unwrap() = phones.into_iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    }

    /// Телефон, на который ставить после `Saved`; `None` — останавливаться на `Saved`.
    pub fn set_target(&self, udid: Option<String>) {
        *self.target.lock().unwrap() = udid;
    }

    fn send(&self, job: Job) {
        let id = match &job {
            Job::Download(id, _, _) | Job::Install(id, _, _) => *id,
        };
        let mut p = self.pending.lock().unwrap();
        let seq = p.next;
        p.next += 1;
        p.waiting.push((seq, id));
        let _ = self.tx.send((seq, job));
    }

    /// Смена Apple ID: забыть ждущие задания аккаунта (trackId > 0) и вернуть их trackId.
    /// Установки из «Все копии» (отрицательный ключ) остаются. Задание, уже взятое в работу,
    /// доводится до конца: установку посреди записи на телефон не прерываем. Событий нет.
    pub fn drop_pending(&self) -> Vec<i64> {
        let mut p = self.pending.lock().unwrap();
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut p.waiting).into_iter().partition(|(_, id)| *id > 0);
        p.waiting = keep;
        gone.into_iter().map(|(_, id)| id).collect()
    }

    /// Ставит уже сохранённую ipa (для случая, когда телефон подключили позже).
    pub fn install_saved(&self, track_id: i64, path: PathBuf) {
        self.send(Job::Install(track_id, path, None));
    }

    /// Ставит ipa на телефон `udid`, не трогая цель очереди («Все копии»).
    pub fn install_on(&self, track_id: i64, path: PathBuf, udid: String) {
        self.send(Job::Install(track_id, path, Some(udid)));
    }

    /// `(track_id, name)`: имя нужно экрану, очереди достаточно trackId. Ключ событий: `track_id`.
    #[cfg(test)]
    pub fn enqueue(&self, apps: Vec<(i64, String)>) {
        self.enqueue_with(apps, false);
    }

    /// «Только скачать»: как `enqueue`, но скачанное остаётся в `Saved` и на телефон не ставится.
    #[cfg(test)]
    pub fn enqueue_download_only(&self, apps: Vec<(i64, String)>) {
        self.enqueue_with(apps, true);
    }

    #[cfg(test)]
    fn enqueue_with(&self, apps: Vec<(i64, String)>, only: bool) {
        self.enqueue_versions(apps.into_iter().map(|(id, _)| (id, None)).collect(), only);
    }

    /// Скачать каждое (trackId, версия): версия — выбранная человеком («Другая версия…»), `None`
    /// — последняя. `only` — «Только скачать».
    #[cfg(test)]
    pub fn enqueue_versions(&self, apps: Vec<(i64, Option<String>)>, only: bool) {
        let place = if only { Place::Hold } else { Place::Target };
        self.enqueue_placed(apps.into_iter().map(|(id, v)| (id, v, place.clone())).collect());
    }

    /// Скачать каждое (trackId, версия) и поставить по своему `Place`.
    pub fn enqueue_placed(&self, apps: Vec<(i64, Option<String>, Place)>) {
        for (id, version, place) in apps {
            if id <= 0 {
                let error = AppError::Unknown { detail: "invalid track id".into() };
                let _ = self.events.send((id, JobState::Failed { error, saved: None }));
                continue;
            }
            let _ = self.events.send((id, JobState::Queued));
            self.send(Job::Download(id, place, version));
        }
    }

    /// Потребитель обязан на `RecvError::Lagged` продолжать приём: пропущены лишь промежуточные
    /// события прогресса, итоговое состояние придёт следом.
    pub fn subscribe(&self) -> broadcast::Receiver<(i64, JobState)> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::fake::{Behavior, FakeHub};
    use crate::ipatool::runner::{FakeRunner, IpaRunner, RunResult};
    use crate::library::testutil::make_ipa;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Space(u64);
    impl SpaceProbe for Space {
        fn available_bytes(&self, _: &Path) -> std::io::Result<u64> {
            Ok(self.0)
        }
    }
    const GB: u64 = 1024 * 1024 * 1024;

    /// Как FakeRunner, но кладёт ipa в каталог `-o`, как настоящий ipatool.
    /// Ответы по trackId; `fail` -> download-not-owned.
    struct DownloadRunner {
        fail: Vec<i64>,
        calls: Mutex<Vec<i64>>,
    }
    #[async_trait]
    impl IpaRunner for DownloadRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let pos = |f: &str| args.iter().position(|a| a == f).unwrap();
            assert!(!args.contains(&"-b".to_string()));
            let track: i64 = args[pos("-i") + 1].parse().unwrap();
            let out = Path::new(&args[pos("-o") + 1]);
            self.calls.lock().unwrap().push(track);
            if self.fail.contains(&track) {
                let d = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ipatool/download-not-owned");
                return RunResult {
                    stdout: std::fs::read_to_string(format!("{d}.jsonl")).unwrap(),
                    exit_code: 1,
                    stderr: String::new(),
                };
            }
            let ipa = out.join(format!("{track}_9.9.ipa"));
            make_ipa(&ipa, &format!("com.example.t{track}"), "1.2.3");
            RunResult {
                stdout: format!("{{\"level\":\"info\",\"output\":{:?},\"purchased\":false,\"success\":true}}\n", ipa.to_str().unwrap()),
                exit_code: 0,
                stderr: String::new(),
            }
        }
    }

    fn setup(runner: Arc<dyn IpaRunner>, free: u64) -> (Queue, tempfile::TempDir, Arc<Library>) {
        let (q, t, lib, _) = setup_hub(runner, free, Behavior::Ok);
        (q, t, lib)
    }

    fn setup_hub(
        runner: Arc<dyn IpaRunner>,
        free: u64,
        behavior: Behavior,
    ) -> (Queue, tempfile::TempDir, Arc<Library>, Arc<FakeHub>) {
        let t = tempfile::tempdir().unwrap();
        let lib = Arc::new(Library::new(t.path().join("lib")));
        lib.set_account(Some("me@example.com")).unwrap();
        let session = Arc::new(Session::new(runner, vec![]));
        let hub = Arc::new(FakeHub::new(behavior));
        (Queue::new(session, lib.clone(), Arc::new(Space(free)), hub.clone()), t, lib, hub)
    }

    /// Ждёт `n` итогов установки (Installed или Failed).
    async fn collect_installs(rx: &mut broadcast::Receiver<(i64, JobState)>, n: usize) -> Vec<(i64, JobState)> {
        let mut all = vec![];
        let mut done = 0;
        while done < n {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            if matches!(ev.1, JobState::Installed | JobState::Failed { .. }) {
                done += 1;
            }
            all.push(ev);
        }
        all
    }

    async fn collect(rx: &mut broadcast::Receiver<(i64, JobState)>, terminals: usize) -> Vec<(i64, JobState)> {
        let mut all = vec![];
        let mut done = 0;
        while done < terminals {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            if matches!(ev.1, JobState::Saved { .. } | JobState::Failed { .. }) {
                done += 1;
            }
            all.push(ev);
        }
        all
    }

    fn of(ev: &[(i64, JobState)], id: i64) -> Vec<&JobState> {
        ev.iter().filter(|(i, _)| *i == id).map(|(_, s)| s).collect()
    }

    #[tokio::test]
    async fn two_jobs_run_in_order() {
        let r = Arc::new(DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) });
        let (q, _t, lib) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        let ev = collect(&mut rx, 2).await;
        for id in [111i64, 222] {
            let s = of(&ev, id);
            assert_eq!(s[0], &JobState::Queued);
            assert_eq!(s[1], &JobState::Downloading { bytes: 0 });
            assert!(matches!(s.last().unwrap(), JobState::Saved { path } if path.ends_with(format!("{id}_com.example.t{id}_1.2.3.ipa"))));
        }
        // строго по одному: b начинает только после того, как a сохранён
        let pos = |id: i64, f: &dyn Fn(&JobState) -> bool| ev.iter().position(|(i, s)| *i == id && f(s)).unwrap();
        assert!(pos(111, &|s| matches!(s, JobState::Saved { .. })) < pos(222, &|s| matches!(s, JobState::Downloading { .. })));
        assert_eq!(*r.calls.lock().unwrap(), [111, 222]);
        let saved = lib.list();
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().any(|s| s.track_id == Some(111) && s.bundle_id == "com.example.t111"));
        assert!(!lib.root().unwrap().join(".partial/111").exists());
    }

    #[tokio::test]
    async fn first_failure_does_not_stop_second() {
        let r = Arc::new(DownloadRunner { fail: vec![111], calls: Mutex::new(vec![]) });
        let (q, _t, _) = setup(r, 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        let ev = collect(&mut rx, 2).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Failed { error: AppError::NotPurchased, saved: None }));
        assert!(matches!(of(&ev, 222).last().unwrap(), JobState::Saved { .. }));
    }

    #[tokio::test]
    async fn low_disk_fails_without_calling_ipatool() {
        let r = Arc::new(FakeRunner::new(vec![]));
        let (q, _t, _) = setup(r.clone(), 100 * 1024 * 1024);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert_eq!(of(&ev, 111), [&JobState::Queued, &JobState::Failed { error: AppError::NoDiskSpace { need_mb: 400 }, saved: None }]);
        assert!(r.calls().is_empty());
    }

    /// Пишет частичный файл и долго «качает»: опрос размера должен заметить байты.
    struct SlowRunner;
    #[async_trait]
    impl IpaRunner for SlowRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let o = args.iter().position(|a| a == "-o").unwrap() + 1;
            let out = Path::new(&args[o]);
            std::fs::write(out.join("part.tmp"), vec![0u8; 1234]).unwrap();
            tokio::time::sleep(Duration::from_millis(700)).await;
            let ipa = out.join("111_1.0.ipa");
            make_ipa(&ipa, "com.example.a", "1.0");
            RunResult {
                stdout: format!("{{\"output\":{:?},\"success\":true}}\n", ipa.to_str().unwrap()),
                exit_code: 0,
                stderr: String::new(),
            }
        }
    }

    /// Пишет немного и замолкает навсегда; `dropped` — его бросили (процесс убит).
    struct StallRunner {
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    struct OnDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl IpaRunner for StallRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let _g = OnDrop(self.dropped.clone());
            let o = args.iter().position(|a| a == "-o").unwrap() + 1;
            std::fs::write(Path::new(&args[o]).join("part.tmp"), vec![0u8; 10]).unwrap();
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[tokio::test]
    async fn stalled_download_fails_with_timeout_and_frees_the_queue() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let t = tempfile::tempdir().unwrap();
        let lib = Arc::new(Library::new(t.path().join("lib")));
        lib.set_account(Some("me@example.com")).unwrap();
        let session = Arc::new(Session::new(Arc::new(StallRunner { dropped: dropped.clone() }), vec![]));
        let hub = Arc::new(FakeHub::new(Behavior::Ok));
        let q = Queue::with_idle(session, lib.clone(), Arc::new(Space(10 * GB)), hub, Duration::from_millis(1200));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert_eq!(of(&ev, 111).last().unwrap(), &&JobState::Failed { error: AppError::DownloadStalled, saved: None });
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst), "ipatool остановлен");
        assert!(!lib.root().unwrap().join(".partial/111").exists());
    }

    /// Ожидание своей очереди к ipatool — не простой скачивания.
    #[tokio::test]
    async fn waiting_for_the_ipatool_lock_is_not_a_stall() {
        let t = tempfile::tempdir().unwrap();
        let lib = Arc::new(Library::new(t.path().join("lib")));
        lib.set_account(Some("me@example.com")).unwrap();
        let session = Arc::new(Session::new(ok_runner(), vec![]));
        let hub = Arc::new(FakeHub::new(Behavior::Ok));
        let q = Queue::with_idle(session.clone(), lib, Arc::new(Space(10 * GB)), hub, Duration::from_millis(600));
        let mut rx = q.subscribe();
        // кто-то держит ipatool 2 с (например, шаг со связкой ключей)
        let s2 = session.clone();
        let hold = tokio::spawn(async move { s2.exclusive(|| std::thread::sleep(Duration::from_millis(2000))).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }), "{:?}", of(&ev, 111));
        hold.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn progress_polls_partial_dir_size() {
        let (q, _t, _) = setup(Arc::new(SlowRunner), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(of(&ev, 111).iter().any(|s| matches!(s, JobState::Downloading { bytes } if *bytes >= 1234)));
    }

    #[tokio::test]
    async fn invalid_track_ids_fail_and_delete_nothing() {
        let r = Arc::new(FakeRunner::new(vec![]));
        let (q, t, lib) = setup(r.clone(), 10 * GB);
        std::fs::create_dir_all(lib.root().unwrap()).unwrap();
        std::fs::write(lib.root().unwrap().join("keep_1.0.ipa"), b"x").unwrap();
        std::fs::write(t.path().join("outside.txt"), b"x").unwrap();
        let mut rx = q.subscribe();
        let bad = [0i64, -1, i64::MIN];
        q.enqueue(bad.iter().map(|id| (*id, "X".to_string())).collect());
        let ev = collect(&mut rx, bad.len()).await;
        let want = JobState::Failed { error: AppError::Unknown { detail: "invalid track id".into() }, saved: None };
        assert_eq!(ev.iter().filter(|(_, s)| *s == want).count(), bad.len());
        assert!(r.calls().is_empty());
        assert!(lib.root().unwrap().join("keep_1.0.ipa").exists() && t.path().join("outside.txt").exists());
    }

    fn ok_runner() -> Arc<DownloadRunner> {
        Arc::new(DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) })
    }

    #[tokio::test]
    async fn saved_job_is_installed_when_target_set() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect_installs(&mut rx, 1).await;
        let s = of(&ev, 111);
        let saved = s.iter().position(|s| matches!(s, JobState::Saved { .. })).unwrap();
        assert_eq!(s[saved + 1], &JobState::Installing { percent: 0 });
        assert!(s.contains(&&JobState::Installing { percent: 60 }));
        assert_eq!(s.last().unwrap(), &&JobState::Installed);
        let calls = hub.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "UDID-1");
        assert!(calls[0].1.ends_with("111_com.example.t111_1.2.3.ipa"));
    }

    #[tokio::test]
    async fn disconnect_midway_fails_and_never_installed() {
        let (q, _t, _, _) = setup_hub(ok_runner(), 10 * GB, Behavior::DisconnectMidway);
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect_installs(&mut rx, 1).await;
        let s = of(&ev, 111);
        let JobState::Failed { error, saved } = (*s.last().unwrap()).clone() else { panic!() };
        assert_eq!(error, AppError::DeviceNotFound { drivers_missing: false });
        assert!(saved.unwrap().ends_with("111_com.example.t111_1.2.3.ipa"));
        assert!(s.contains(&&JobState::Installing { percent: 30 }));
        assert!(!s.contains(&&JobState::Installed));
        // скачанный файл остаётся Saved до повторной попытки: ошибку установки сохранили в событии
        assert!(s.iter().any(|s| matches!(s, JobState::Saved { .. })));
    }

    #[tokio::test]
    async fn device_error_passes_through_with_saved_copy() {
        let (q, _t, _, _) = setup_hub(ok_runner(), 10 * GB, Behavior::Fail(AppError::DeviceNotTrusted));
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect_installs(&mut rx, 1).await;
        let JobState::Failed { error, saved } = (*of(&ev, 111).last().unwrap()).clone() else { panic!() };
        assert_eq!(error, AppError::DeviceNotTrusted);
        assert!(saved.is_some(), "«Повторить» поставит копию без скачивания");
    }

    #[tokio::test]
    async fn download_only_stops_at_saved_even_with_target() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.enqueue_download_only(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(hub.calls.lock().unwrap().is_empty(), "«Только скачать» на телефон не ставит");
        assert!(rx.try_recv().is_err(), "после Saved событий быть не должно");
    }

    #[tokio::test]
    async fn no_target_stops_at_saved() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(hub.calls.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "после Saved событий быть не должно");
    }

    #[tokio::test]
    async fn install_saved_installs_later() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        let JobState::Saved { path } = of(&ev, 111).last().unwrap().to_owned().clone() else { panic!() };
        q.set_target(Some("UDID-2".into()));
        q.install_saved(111, path.clone());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, 111).last().unwrap(), &&JobState::Installed);
        assert_eq!(hub.calls.lock().unwrap()[0], ("UDID-2".to_string(), path));
    }

    /// «Все копии» ставит на свой телефон: цель очереди (главный поток) не меняется и не мешает.
    #[tokio::test]
    async fn install_on_uses_its_own_phone_not_the_target() {
        let (q, t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        let copy = t.path().join("copy.ipa");
        make_ipa(&copy, "com.example.c", "1.0");
        q.set_target(Some("UDID-A".into()));
        let mut rx = q.subscribe();
        q.install_on(-5, copy.clone(), "UDID-B".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, -5).last().unwrap(), &&JobState::Installed);
        assert_eq!(hub.calls.lock().unwrap()[0].0, "UDID-B");
        assert_eq!(*q.target.lock().unwrap(), Some("UDID-A".to_string()));
        // и без цели очереди тоже ставит на свой телефон
        q.set_target(None);
        q.install_on(-6, copy, "UDID-B".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, -6).last().unwrap(), &&JobState::Installed);
        assert_eq!(hub.calls.lock().unwrap()[1].0, "UDID-B");
    }

    /// Как DownloadRunner, но запоминает аргументы каждого запуска.
    struct ArgsRunner {
        inner: DownloadRunner,
        args: Mutex<Vec<Vec<String>>>,
    }
    #[async_trait]
    impl IpaRunner for ArgsRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            self.args.lock().unwrap().push(args.to_vec());
            self.inner.run(args).await
        }
    }

    #[tokio::test]
    async fn chosen_version_is_downloaded() {
        let r = Arc::new(ArgsRunner { inner: DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) }, args: Mutex::new(vec![]) });
        let (q, _t, _) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue_versions(vec![(111, Some("812236754".into())), (222, None)], false);
        let ev = collect(&mut rx, 2).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }));
        let args = r.args.lock().unwrap();
        let ver = |a: &Vec<String>| a.iter().position(|x| x == "--external-version-id").map(|i| a[i + 1].clone());
        assert_eq!(ver(&args[0]).as_deref(), Some("812236754"));
        assert_eq!(ver(&args[1]), None, "без выбора — последняя");
        assert!(args.iter().flatten().all(|a| !a.contains("purchase")));
    }

    /// Копия требует iOS новее телефона: не ставим, понятная ошибка, копия остаётся.
    #[tokio::test]
    async fn too_old_ios_refuses_install_before_touching_phone() {
        let (q, t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        let copy = t.path().join("5_com.example.c_2.0.ipa");
        crate::library::testutil::make_ipa_min_os(&copy, "com.example.c", "2.0", "16.0");
        q.set_phone_versions(vec![("UDID-1".into(), "15.7".into()), ("UDID-2".into(), "16.1".into())]);
        let mut rx = q.subscribe();
        q.install_on(-5, copy.clone(), "UDID-1".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(
            of(&ev, -5).last().unwrap(),
            &&JobState::Failed { error: AppError::IosTooOld { need: "16.0".into(), have: "15.7".into() }, saved: Some(copy.clone()) }
        );
        assert!(hub.calls.lock().unwrap().is_empty(), "телефон не трогали");
        // телефон с подходящей iOS — ставит
        q.install_on(-6, copy.clone(), "UDID-2".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, -6).last().unwrap(), &&JobState::Installed);
        // iOS телефона неизвестна — ставит (откажет сам iPhone, если что)
        q.install_on(-7, copy, "UDID-3".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, -7).last().unwrap(), &&JobState::Installed);
    }

    #[tokio::test]
    async fn install_saved_without_target_fails_not_found() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        let mut rx = q.subscribe();
        q.install_saved(7, PathBuf::from("/x/7_a.ipa"));
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(
            of(&ev, 7).last().unwrap(),
            &&JobState::Failed { error: AppError::DeviceNotFound { drivers_missing: false }, saved: Some(PathBuf::from("/x/7_a.ipa")) }
        );
        assert!(hub.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_local_ipa_fails_unknown_without_touching_phone() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.install_saved(9, PathBuf::from("/nonexistent/9_a.ipa"));
        let ev = collect_installs(&mut rx, 1).await;
        assert!(matches!(of(&ev, 9).last().unwrap(),
            JobState::Failed { error: AppError::Unknown { detail }, .. } if detail.starts_with("local file: ")));
        assert!(hub.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn panicking_install_fails_job_and_queue_survives() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Panic);
        q.set_target(Some("UDID-1".into()));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        let ev = collect_installs(&mut rx, 2).await;
        for id in [111i64, 222] {
            assert!(matches!(of(&ev, id).last().unwrap(),
                JobState::Failed { error: AppError::Unknown { detail }, .. } if detail == "internal error"), "{id}");
        }
        assert_eq!(hub.calls.lock().unwrap().len(), 2);
    }

    /// Первое скачивание ждёт `gate`: можно поймать очередь посреди работы.
    struct GateRunner {
        inner: DownloadRunner,
        gate: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl IpaRunner for GateRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let _p = self.gate.acquire().await.unwrap();
            self.inner.run(args).await
        }
    }

    #[tokio::test]
    async fn drop_pending_skips_waiting_jobs_not_the_running_one() {
        let r = Arc::new(GateRunner { inner: DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) }, gate: tokio::sync::Semaphore::new(0) });
        let (q, _t, _lib) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        q.install_saved(333, PathBuf::from("/x/333_a.ipa"));
        // установка из «Все копии» (свой отрицательный ключ) от аккаунта не зависит
        q.install_saved(-5, PathBuf::from("/x/copy.ipa"));
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            if ev == (111, JobState::Downloading { bytes: 0 }) {
                break;
            }
        }
        let mut dropped = q.drop_pending();
        dropped.sort();
        assert_eq!(dropped, vec![222, 333], "111 уже взято в работу");
        r.gate.add_permits(10);
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }), "идущее задание доводится до конца");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut late = vec![];
        while let Ok(e) = rx.try_recv() {
            late.push(e);
        }
        assert!(late.iter().all(|(id, _)| *id == 111 || *id == -5), "{late:?}");
        assert!(late.iter().any(|(id, _)| *id == -5) || !of(&ev, -5).is_empty(), "копия не отброшена: {late:?}");
        assert_eq!(*r.inner.calls.lock().unwrap(), vec![111]);
        // очередь жива и принимает новое
        q.enqueue(vec![(444, "D".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 444).last().unwrap(), JobState::Saved { .. }));
    }

    /// Первый ответ Apple — 503, дальше как DownloadRunner.
    struct BusyOnceRunner {
        inner: DownloadRunner,
        busy: std::sync::atomic::AtomicBool,
    }
    #[async_trait]
    impl IpaRunner for BusyOnceRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            if !self.busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return RunResult {
                    stdout: "{\"level\":\"error\",\"error\":\"unexpected status code 503\",\"success\":false}\n".into(),
                    exit_code: 1,
                    stderr: String::new(),
                };
            }
            self.inner.run(args).await
        }
    }

    /// Apple просит подождать: задание говорит «повтор через N с» и потом скачивается.
    #[tokio::test]
    async fn apple_busy_shows_the_wait_then_downloads() {
        let t = tempfile::tempdir().unwrap();
        let lib = Arc::new(Library::new(t.path().join("lib")));
        lib.set_account(Some("me@example.com")).unwrap();
        let r = Arc::new(BusyOnceRunner { inner: DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) }, busy: Default::default() });
        let session = Arc::new(Session::new(r, vec![]).with_delays(vec![Duration::from_millis(1100)]));
        let q = Queue::new(session, lib, Arc::new(Space(10 * GB)), Arc::new(FakeHub::new(Behavior::Ok)));
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        let ev = collect(&mut rx, 1).await;
        let s = of(&ev, 111);
        let wait = s.iter().position(|s| **s == JobState::Waiting { seconds: 2 }).expect("«повтор через 2 с»");
        // пока ждём повтора, прогресс скачивания не перебивает «подождите»
        assert!(!matches!(s[wait + 1], JobState::Downloading { bytes: 0 }), "{s:?}");
        assert!(matches!(s.last().unwrap(), JobState::Saved { .. }));
    }

    /// Пауза до старта: новые задания не начинаются; «Продолжить» — идут по порядку.
    #[tokio::test]
    async fn pause_holds_new_jobs_until_resume() {
        let r = ok_runner();
        let (q, _t, _) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.pause();
        assert!(q.paused());
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(r.calls.lock().unwrap().is_empty(), "на паузе ipatool не запускается");
        q.resume();
        assert!(!q.paused());
        let ev = collect(&mut rx, 2).await;
        assert!(matches!(of(&ev, 222).last().unwrap(), JobState::Saved { .. }));
        assert_eq!(*r.calls.lock().unwrap(), [111, 222]);
    }

    /// Самый первый запуск висит (пока его не бросят), следующие — как DownloadRunner.
    struct HangFirstRunner {
        inner: DownloadRunner,
        hung: std::sync::atomic::AtomicBool,
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl IpaRunner for HangFirstRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let track: i64 = args[args.iter().position(|a| a == "-i").unwrap() + 1].parse().unwrap();
            if !self.hung.swap(true, std::sync::atomic::Ordering::SeqCst) {
                self.inner.calls.lock().unwrap().push(track);
                let _g = OnDrop(self.dropped.clone());
                let o = args.iter().position(|a| a == "-o").unwrap() + 1;
                std::fs::write(Path::new(&args[o]).join("half.ipa.part"), vec![0u8; 10]).unwrap();
                std::future::pending::<()>().await;
            }
            self.inner.run(args).await
        }
    }

    /// «Пауза» посреди скачивания: ipatool остановлен, недокачанное удалено, задание «на паузе»;
    /// «Продолжить» качает его заново первым, потом остальные.
    #[tokio::test]
    async fn pause_cancels_the_running_download_and_resume_restarts_it() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let r = Arc::new(HangFirstRunner { inner: DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) }, hung: Default::default(), dropped: dropped.clone() });
        let (q, _t, lib) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into()), (222, "B".into())]);
        while !lib.root().unwrap().join(".partial/111/half.ipa.part").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        q.pause();
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            assert!(!matches!(ev.1, JobState::Failed { .. }), "пауза — не ошибка: {ev:?}");
            if ev == (111, JobState::Paused) {
                break;
            }
        }
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst), "ipatool остановлен");
        assert!(!lib.root().unwrap().join(".partial/111").exists(), "недокачанное удалено");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(*r.inner.calls.lock().unwrap(), [111], "222 на паузе не начинается");
        q.resume();
        let ev = collect(&mut rx, 2).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }));
        assert!(matches!(of(&ev, 222).last().unwrap(), JobState::Saved { .. }));
        assert_eq!(*r.inner.calls.lock().unwrap(), [111, 111, 222], "прерванное — первым");
    }

    /// Смена Apple ID на паузе: прерванное задание аккаунта тоже отбрасывается.
    #[tokio::test]
    async fn drop_pending_takes_the_paused_job_too() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let r = Arc::new(HangFirstRunner { inner: DownloadRunner { fail: vec![], calls: Mutex::new(vec![]) }, hung: Default::default(), dropped });
        let (q, _t, lib) = setup(r.clone(), 10 * GB);
        let mut rx = q.subscribe();
        q.enqueue(vec![(111, "A".into())]);
        while !lib.root().unwrap().join(".partial/111/half.ipa.part").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        q.pause();
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            if ev == (111, JobState::Paused) {
                break;
            }
        }
        assert_eq!(q.drop_pending(), vec![111]);
        q.resume();
        q.enqueue(vec![(222, "B".into())]);
        let ev = collect(&mut rx, 1).await;
        assert!(of(&ev, 111).is_empty(), "{ev:?}");
        assert_eq!(*r.inner.calls.lock().unwrap(), [111, 222]);
    }

    /// Задание со своим телефоном («Продолжить» прошлый раз): ставится только туда — не на цель
    /// очереди; телефона нет — остаётся «Сохранено».
    #[tokio::test]
    async fn placed_jobs_install_only_on_their_own_phone() {
        let (q, _t, _, hub) = setup_hub(ok_runner(), 10 * GB, Behavior::Ok);
        q.set_target(Some("UDID-OTHER".into()));
        let mut rx = q.subscribe();
        q.enqueue_placed(vec![(111, None, Place::Phone("UDID-MINE".into())), (222, None, Place::Hold)]);
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, 111).last().unwrap(), &&JobState::Installed);
        let ev2 = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev2, 222).last().unwrap(), JobState::Saved { .. }) || matches!(of(&ev, 222).last(), Some(JobState::Saved { .. })));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let calls = hub.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "222 на чужой телефон не ставится");
        assert_eq!(calls[0].0, "UDID-MINE");
    }

    /// «Все копии» ставит на свой телефон и на паузе: пауза держит только очередь скачиваний.
    #[tokio::test]
    async fn copy_install_on_its_phone_bypasses_the_pause() {
        let r = ok_runner();
        let (q, t, _, hub) = setup_hub(r.clone(), 10 * GB, Behavior::Ok);
        let copy = t.path().join("copy.ipa");
        make_ipa(&copy, "com.example.c", "1.0");
        let mut rx = q.subscribe();
        q.pause();
        q.enqueue(vec![(111, "A".into())]);
        q.install_saved(333, copy.clone()); // без своего телефона — ждёт паузу, как все
        q.install_on(-5, copy, "UDID-B".into());
        let ev = collect_installs(&mut rx, 1).await;
        assert_eq!(of(&ev, -5).last().unwrap(), &&JobState::Installed);
        assert!(r.calls.lock().unwrap().is_empty(), "скачивание на паузе не начато");
        assert_eq!(hub.calls.lock().unwrap().len(), 1, "333 ждёт «Продолжить»");
        q.resume();
        let ev = collect(&mut rx, 1).await;
        assert!(matches!(of(&ev, 111).last().unwrap(), JobState::Saved { .. }));
        assert_eq!(*r.calls.lock().unwrap(), [111]);
    }

    #[test]
    fn wait_seconds_round_up() {
        assert_eq!(wait_seconds(Duration::ZERO), 0);
        assert_eq!(wait_seconds(Duration::from_millis(1)), 1);
        assert_eq!(wait_seconds(Duration::from_secs(5)), 5);
        assert_eq!(wait_seconds(Duration::from_millis(10_200)), 11);
    }

    #[test]
    fn dir_size_recurses() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("sub/deeper")).unwrap();
        std::fs::write(t.path().join("a"), vec![0u8; 10]).unwrap();
        std::fs::write(t.path().join("sub/b"), vec![0u8; 5]).unwrap();
        std::fs::write(t.path().join("sub/deeper/c"), vec![0u8; 7]).unwrap();
        assert_eq!(dir_size(t.path()), 22);
    }

    #[test]
    fn dir_size_sums_files() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a"), vec![0u8; 10]).unwrap();
        std::fs::write(t.path().join("b"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_size(t.path()), 15);
        assert_eq!(dir_size(&t.path().join("none")), 0);
    }
}
