
//! Очередь: один рабочий поток делает скачивание, затем установку на телефон, строго
//! последовательно (следующее скачивание ждёт конца предыдущей установки).
//! Прогресс скачивания — опросом размера `.partial/<track_id>/`.
use crate::device::DeviceHub;
use crate::errors::AppError;
use crate::ipatool::session::Session;
use crate::library::Library;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

pub const MIN_FREE_MB: u64 = 500;
pub const POLL: Duration = Duration::from_millis(500);

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

enum Job {
    Download(i64),
    /// Телефон задан явно («Все копии») — он главнее цели очереди и её не меняет.
    Install(i64, PathBuf, Option<String>),
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
}

/// Ставит сохранённую ipa на телефон; возвращает итоговое состояние. `Installed` только при `Ok`.
async fn run_install(
    hub: &dyn DeviceHub,
    udid: Option<String>,
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

async fn run_job(
    id: i64,
    session: &Session,
    library: &Library,
    space: &dyn SpaceProbe,
    emit: &(dyn Fn(JobState) + Send + Sync),
) -> Result<PathBuf, AppError> {
    let _ = std::fs::create_dir_all(library.base());
    // Если свободное место определить не вышло, не блокируем скачивание.
    if let Ok(avail) = space.available_bytes(library.base()) {
        let avail_mb = avail / (1024 * 1024);
        if avail_mb < MIN_FREE_MB {
            return Err(AppError::NoDiskSpace { need_mb: (MIN_FREE_MB - avail_mb).max(1) });
        }
    }
    let partial = library.partial_dir(id)?;
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| AppError::Unknown { detail: format!("partial: {e}") })?;
    emit(JobState::Downloading { bytes: 0 });
    let result = {
        let download = session.download(id, &partial);
        tokio::pin!(download);
        let mut tick = tokio::time::interval(POLL);
        tick.tick().await; // первый тик срабатывает сразу
        loop {
            tokio::select! {
                r = &mut download => break r,
                _ = tick.tick() => emit(JobState::Downloading { bytes: dir_size(&partial) }),
            }
        }
    };
    let saved = result.and_then(|file| library.commit(&partial, &file));
    let _ = std::fs::remove_dir_all(&partial);
    saved
}

/// Один job целиком: скачивание (если нужно), затем установка (если задан телефон).
async fn process(
    job: Job,
    session: Arc<Session>,
    library: Arc<Library>,
    space: Arc<dyn SpaceProbe>,
    hub: Arc<dyn DeviceHub>,
    tgt: Arc<Mutex<Option<String>>>,
    emit: Arc<dyn Fn(JobState) + Send + Sync>,
) {
    // explicit: установку попросили прямо (`install_saved`) — без телефона это ошибка.
    let (saved, explicit) = match job {
        Job::Download(id) => match run_job(id, &session, &library, space.as_ref(), emit.as_ref()).await {
            Ok(path) => {
                emit(JobState::Saved { path: path.clone() });
                (Some((path, None)), false)
            }
            Err(error) => {
                emit(JobState::Failed { error, saved: None });
                (None, false)
            }
        },
        Job::Install(_, path, device) => (Some((path, device)), true),
    };
    if let Some((path, device)) = saved {
        let udid = device.or_else(|| tgt.lock().unwrap().clone());
        // Без телефона скачанное остаётся в `Saved`; `install_saved` поставит позже.
        if udid.is_some() || explicit {
            emit(run_install(hub.as_ref(), udid, &path, emit.as_ref()).await);
        }
    }
}

impl Queue {
    /// Запускает единственный рабочий поток очереди; нужен tokio runtime.
    pub fn new(
        session: Arc<Session>,
        library: Arc<Library>,
        space: Arc<dyn SpaceProbe>,
        hub: Arc<dyn DeviceHub>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, Job)>();
        let pending: Arc<Mutex<Pending>> = Arc::default();
        let current = pending.clone();
        let (events, _) = broadcast::channel(4096);
        let target: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (ev, tgt) = (events.clone(), target.clone());
        tokio::spawn(async move {
            while let Some((seq, job)) = rx.recv().await {
                let id = match &job {
                    Job::Download(id) | Job::Install(id, _, _) => *id,
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
                let work = tokio::spawn(process(
                    job,
                    session.clone(),
                    library.clone(),
                    space.clone(),
                    hub.clone(),
                    tgt.clone(),
                    emit.clone(),
                ));
                if work.await.is_err() {
                    emit(JobState::Failed { error: AppError::Unknown { detail: "internal error".into() }, saved: None });
                }
            }
        });
        Self { tx, pending, events, target }
    }

    /// Телефон, на который ставить после `Saved`; `None` — останавливаться на `Saved`.
    pub fn set_target(&self, udid: Option<String>) {
        *self.target.lock().unwrap() = udid;
    }

    fn send(&self, job: Job) {
        let id = match &job {
            Job::Download(id) | Job::Install(id, _, _) => *id,
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
    pub fn enqueue(&self, apps: Vec<(i64, String)>) {
        for (id, _name) in apps {
            if id <= 0 {
                let error = AppError::Unknown { detail: "invalid track id".into() };
                let _ = self.events.send((id, JobState::Failed { error, saved: None }));
                continue;
            }
            let _ = self.events.send((id, JobState::Queued));
            self.send(Job::Download(id));
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

    fn of<'a>(ev: &'a [(i64, JobState)], id: i64) -> Vec<&'a JobState> {
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
