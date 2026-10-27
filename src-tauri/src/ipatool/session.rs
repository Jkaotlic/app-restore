
//! Сессия ipatool: вход, список покупок, скачивание, выход. Повторы и разбор вывода внутри.
use super::output::{IpaOutcome, OwnedApp, parse_output};
use super::runner::{Args, IpaRunner, RunResult};
use crate::errors::AppError;
use crate::retry::{APPLE_DELAYS, with_retry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Сроки ожидания. Запуск, переросший срок, обрывается: будущее `run` бросается, а
/// `SidecarRunner` при этом убивает процесс ipatool. Очередь замка — сколько ждать своей очереди.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Короткие запросы к Apple: вход проверить, покупки, версии, выход.
    pub quick_run: Duration,
    /// Очередь коротких: дольше обычно значит зависшую связку ключей у предыдущего шага.
    pub quick_wait: Duration,
    /// Вход: ipatool ходит к Apple дважды.
    pub login_run: Duration,
    /// Вход ждёт своей очереди недолго: человек смотрит на «Входим…».
    pub login_wait: Duration,
    /// Скачивание: страховка от вечного зависания; простой без роста ловит очередь (`queue`).
    pub download_run: Duration,
    /// Шаг со связкой ключей (`exclusive`): дольше — сдаёмся (`KeychainWaiting`).
    pub keychain_run: Duration,
    /// Дольше этого шаг со связкой ключей — скорее всего, macOS спрашивает разрешение: подсказка
    /// человеку (`exclusive_cancelable`), ожидание продолжается.
    pub keychain_hint: Duration,
    /// Очередь шага со связкой ключей.
    pub keychain_wait: Duration,
}

pub const LIMITS: Limits = Limits {
    quick_run: Duration::from_secs(90),
    quick_wait: Duration::from_secs(15 * 60),
    login_run: Duration::from_secs(180),
    login_wait: Duration::from_secs(30),
    download_run: Duration::from_secs(4 * 3600),
    keychain_run: Duration::from_secs(180),
    keychain_hint: Duration::from_secs(20),
    keychain_wait: Duration::from_secs(30),
};

/// Ответ ipatool не того вида: в подробности только вид (`LoggedIn`, `Purchases`…). Поля не
/// пишутся — в `LoggedIn` настоящие имя и адрес человека.
fn unexpected(o: &IpaOutcome) -> AppError {
    let full = format!("{o:?}");
    let kind = full.split([' ', '{', '(']).next().unwrap_or_default();
    AppError::Unknown { detail: format!("неожиданный ответ ipatool: {kind}") }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Account {
    pub email: String,
    pub name: String,
}

pub struct Session {
    runner: Arc<dyn IpaRunner>,
    extra_args: Vec<String>,
    delays: Vec<Duration>,
    /// ipatool запускается строго по одному: общий Keychain и токен не любят параллельных вызовов.
    /// Замок держится на время одного запуска, поэтому check_owned и load_purchases идут вперемешку.
    one_at_a_time: Arc<tokio::sync::Mutex<()>>,
    limits: Limits,
    /// Файл cookies ipatool: испорченный (ipatool падает на старте) откладываем и повторяем вызов.
    cookies: Option<PathBuf>,
}

/// Какой срок у запуска (`Limits`).
#[derive(Clone, Copy)]
enum Kind {
    Quick,
    Login,
    Download,
}

impl Session {
    pub fn new(runner: Arc<dyn IpaRunner>, extra_args: Vec<String>) -> Self {
        Self { runner, extra_args, delays: APPLE_DELAYS.to_vec(), one_at_a_time: Arc::default(), limits: LIMITS, cookies: None }
    }

    pub fn with_cookies(mut self, path: PathBuf) -> Self {
        self.cookies = Some(path);
        self
    }

    /// ipatool не читает свой файл cookies (`cannot load cookies`) — откладываем файл рядом.
    /// `true`, если отложили и вызов стоит повторить.
    fn set_aside_broken_cookies(&self, res: &RunResult) -> bool {
        let Some(path) = &self.cookies else { return false };
        if !(res.stderr.contains("cannot load cookies") || res.stdout.contains("cannot load cookies")) {
            return false;
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let aside = path.with_file_name(format!("cookies.broken-{secs}"));
        match std::fs::rename(path, &aside) {
            Ok(()) => {
                crate::log::write("ipatool", "cookies file unreadable, moved aside");
                true
            }
            Err(e) => {
                crate::log::write("ipatool", &format!("cookies file unreadable, not moved: {}", e.kind()));
                false
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Своя очередь к ipatool не дольше `wait`; не дождались — `Busy` (идёт скачивание или
    /// предыдущий шаг ждёт связку ключей).
    async fn turn(&self, wait: Duration) -> Result<tokio::sync::OwnedMutexGuard<()>, AppError> {
        tokio::time::timeout(wait, self.one_at_a_time.clone().lock_owned()).await.map_err(|_| AppError::Busy)
    }

    #[cfg(test)]
    pub(crate) fn with_delays(mut self, delays: Vec<Duration>) -> Self {
        self.delays = delays;
        self
    }

    /// Один вызов ipatool: глобальные флаги и extra_args добавляются сюда, вывод разбирается.
    async fn call(&self, args: Vec<String>) -> Result<IpaOutcome, AppError> {
        self.call_as(Kind::Quick, args).await
    }

    async fn call_as(&self, kind: Kind, args: Vec<String>) -> Result<IpaOutcome, AppError> {
        self.call_marked(kind, args, None).await
    }

    /// `call_as`; `started` ставится в `true`, когда очередь к ipatool дошла и он запущен.
    async fn call_marked(&self, kind: Kind, mut args: Vec<String>, started: Option<&AtomicBool>) -> Result<IpaOutcome, AppError> {
        args.extend(["--non-interactive", "--format", "json"].map(String::from));
        args.extend(self.extra_args.iter().cloned());
        let args = Args(args);
        let l = self.limits;
        let (wait, run) = match kind {
            Kind::Quick => (l.quick_wait, l.quick_run),
            Kind::Login => (l.login_wait, l.login_run),
            Kind::Download => (l.quick_wait.max(l.download_run), l.download_run),
        };
        let res = {
            let _turn = self.turn(wait).await?;
            if let Some(f) = started {
                f.store(true, Ordering::SeqCst);
            }
            let mut repeated = false;
            loop {
                let r = match tokio::time::timeout(run, self.runner.run(&args.0)).await {
                    Ok(r) => r,
                    Err(_) => {
                        // команду без аргументов: среди них может быть пароль
                        crate::log::write("ipatool", &format!("{} {}: no answer in {} s, stopped", args.0[0], args.0.get(1).map_or("", |s| s.as_str()), run.as_secs()));
                        return Err(AppError::Timeout);
                    }
                };
                if repeated || !self.set_aside_broken_cookies(&r) {
                    break r;
                }
                repeated = true;
            }
        };
        if res.stdout.trim().is_empty() && !res.stderr.trim().is_empty() {
            return Err(AppError::Unknown { detail: res.stderr.trim().chars().take(300).collect() });
        }
        parse_output(&res.stdout, res.exit_code)
    }

    /// `f` в отдельном потоке, пока ни один запуск ipatool не идёт и не начнётся: для смены
    /// записи входа и cookies (`accounts`). Связка ключей молчит дольше `keychain_run` (macOS
    /// показал окно разрешения, возможно за окном программы) — `KeychainWaiting`; `f` при этом
    /// доделывается в фоне и держит замок до конца: ipatool не читает вход посреди записи.
    pub async fn exclusive<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Result<T, AppError> {
        self.exclusive_inner(move |_| f(), false, &|| {}).await
    }

    /// `exclusive` с подсказкой `on_slow` через `keychain_hint` (окно macOS); ожидание идёт дальше.
    pub async fn exclusive_hinted<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
        on_slow: &(dyn Fn() + Send + Sync),
    ) -> Result<T, AppError> {
        self.exclusive_inner(move |_| f(), false, on_slow).await
    }

    /// Как `exclusive`, но по истечении срока замок отпускается сразу, а `f` получает отмену
    /// (`true` во флаге): после неё `f` ничего не должен записывать. Для снимка перед входом:
    /// вход тогда может идти дальше без него. `on_slow` — один раз через `keychain_hint`, если
    /// шаг ещё идёт (подсказка про окно macOS); ожидание при этом не прерывается.
    pub async fn exclusive_cancelable<T: Send + 'static>(
        &self,
        f: impl FnOnce(&AtomicBool) -> T + Send + 'static,
        on_slow: &(dyn Fn() + Send + Sync),
    ) -> Result<T, AppError> {
        self.exclusive_inner(f, true, on_slow).await
    }

    async fn exclusive_inner<T: Send + 'static>(
        &self,
        f: impl FnOnce(&AtomicBool) -> T + Send + 'static,
        release: bool,
        on_slow: &(dyn Fn() + Send + Sync),
    ) -> Result<T, AppError> {
        let turn = self.turn(self.limits.keychain_wait).await?;
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let mut job = tokio::task::spawn_blocking(move || f(&c));
        let deadline = tokio::time::Instant::now() + self.limits.keychain_run;
        let early = if self.limits.keychain_hint < self.limits.keychain_run {
            match tokio::time::timeout(self.limits.keychain_hint, &mut job).await {
                Ok(done) => Some(done),
                Err(_) => {
                    crate::log::write("keychain", &format!("still waiting after {} s (macOS permission dialog?)", self.limits.keychain_hint.as_secs()));
                    on_slow();
                    None
                }
            }
        } else {
            None
        };
        let outcome = match early {
            Some(done) => Ok(done),
            None => tokio::time::timeout_at(deadline, &mut job).await,
        };
        match outcome {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => std::panic::resume_unwind(e.into_panic()),
            Err(_) => {
                crate::log::write("keychain", &format!("no answer in {} s (macOS permission dialog?)", self.limits.keychain_run.as_secs()));
                cancel.store(true, Ordering::SeqCst);
                if release {
                    drop(turn);
                } else {
                    tokio::spawn(async move {
                        let _ = job.await;
                        drop(turn);
                    });
                }
                Err(AppError::KeychainWaiting)
            }
        }
    }

    async fn call_retry(&self, args: Vec<String>) -> Result<IpaOutcome, AppError> {
        with_retry(&self.delays, || self.call(args.clone())).await
    }

    fn account(outcome: IpaOutcome) -> Result<Account, AppError> {
        match outcome {
            IpaOutcome::LoggedIn { email, name } => Ok(Account { email, name }),
            o => Err(unexpected(&o)),
        }
    }

    pub async fn login(&self, email: &str, password: &str, code: Option<String>) -> Result<Account, AppError> {
        let mut args = ["auth", "login", "-e", email, "-p", password].map(String::from).to_vec();
        if let Some(c) = code {
            args.extend(["--auth-code".to_string(), c]);
        }
        // Без повторов: пароль уходит в ipatool один раз на действие пользователя,
        // повтор мог бы сжечь одноразовый код 2FA.
        Self::account(self.call_as(Kind::Login, args).await?)
    }

    /// `Ok(None)`, если входа нет: это нормальное состояние, не сбой.
    pub async fn info(&self) -> Result<Option<Account>, AppError> {
        match self.call_retry(vec!["auth".into(), "info".into()]).await {
            Ok(o) => Self::account(o).map(Some),
            Err(AppError::NotLoggedIn) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Все покупки для iPhone, по 100 на страницу. `on_page(страница, всего покупок)` после каждой.
    /// `current()` вернул false — цепочка устарела (новый `load_purchases` или выход): после текущей
    /// страницы останавливаемся с `Err(Unknown "purchases cancelled")`.
    pub async fn all_purchases(
        &self,
        on_page: &(dyn Fn(u32, u32) + Send + Sync),
        current: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<OwnedApp>, AppError> {
        const MAX_PAGES: u32 = 1000;
        let mut all: Vec<OwnedApp> = Vec::new();
        let mut total_seen = 0usize;
        for page in 1..=MAX_PAGES {
            let args = ["list-purchases", "--platform", "iphone", "-l", "100", "-p", &page.to_string()]
                .map(String::from)
                .to_vec();
            match self.call_retry(args).await? {
                IpaOutcome::Purchases { apps, total, .. } => {
                    total_seen = total as usize;
                    on_page(page, total);
                    let empty = apps.is_empty();
                    all.extend(apps);
                    if all.len() >= total as usize {
                        return Ok(all);
                    }
                    if !current() {
                        return Err(AppError::Unknown { detail: "purchases cancelled".into() });
                    }
                    if empty {
                        break;
                    }
                }
                o => return Err(unexpected(&o)),
            }
        }
        Err(AppError::Unknown { detail: format!("purchases incomplete: got {} of {}", all.len(), total_seen) })
    }

    /// Права и последняя версия по trackId. Права: `list-versions -i` (≈2 с, ничего не скачивает,
    /// работает и для удалённых из магазина); прав нет — `Err(NotPurchased)`. Версия:
    /// `get-version-metadata` по последнему id из списка. Права к этому моменту доказаны, поэтому
    /// сбой метаданных даёт `Ok(None)` (версия просто не показывается); ошибки входа идут наверх.
    /// Только чтение, поэтому с повторами.
    pub async fn latest_version(&self, track_id: i64) -> Result<Option<String>, AppError> {
        let id = track_id.to_string();
        let ids = match self.call_retry(["list-versions", "-i", &id].map(String::from).to_vec()).await? {
            IpaOutcome::Versions { ids } => ids,
            o => return Err(unexpected(&o)),
        };
        let Some(last) = ids.last() else { return Ok(None) };
        let args = ["get-version-metadata", "-i", &id, "--external-version-id", last].map(String::from).to_vec();
        match self.call_retry(args).await {
            Ok(IpaOutcome::VersionMetadata { display_version }) => Ok(Some(display_version).filter(|v| !v.trim().is_empty())),
            Err(e @ (AppError::NotLoggedIn | AppError::AccountDisabled | AppError::AppleChangedLogin)) => Err(e),
            Ok(o) => {
                crate::log::write(&format!("version {track_id}"), &unexpected(&o).to_string());
                Ok(None)
            }
            Err(e) => {
                // Секретов нет: аргументы ipatool в AppError не попадают.
                crate::log::write(&format!("version {track_id}"), &e.to_string());
                Ok(None)
            }
        }
    }

    /// Скачивание по trackId (`-i`): `-b` не работает для удалённых из магазина приложений.
    #[cfg(test)]
    pub async fn download(&self, track_id: i64, out_dir: &Path) -> Result<PathBuf, AppError> {
        self.download_marked(track_id, out_dir, &AtomicBool::new(false)).await
    }

    /// `download`; `started` — ipatool запущен (своя очередь дошла). До этого скачивание не
    /// идёт, и простоем ожидание очереди не считается (`queue`).
    pub async fn download_marked(&self, track_id: i64, out_dir: &Path, started: &AtomicBool) -> Result<PathBuf, AppError> {
        let out = out_dir
            .to_str()
            .ok_or_else(|| AppError::Unknown { detail: "output path is not valid UTF-8".into() })?;
        let args = ["download", "-i", &track_id.to_string(), "-o", out].map(String::from).to_vec();
        match with_retry(&self.delays, || self.call_marked(Kind::Download, args.clone(), Some(started))).await? {
            IpaOutcome::Downloaded { path } => Ok(path),
            o => Err(unexpected(&o)),
        }
    }

    /// Вход живой? Самый дешёвый запрос к Apple: одна покупка с первой страницы. Только чтение.
    pub async fn check_token(&self) -> Result<(), AppError> {
        let args = ["list-purchases", "--platform", "iphone", "-l", "1", "-p", "1"].map(String::from).to_vec();
        self.call_retry(args).await.map(|_| ())
    }

    pub async fn logout(&self) -> Result<(), AppError> {
        self.call_retry(vec!["auth".into(), "revoke".into()]).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipatool::runner::{FakeRunner, RunResult};

    fn fx(name: &str) -> RunResult {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ipatool/");
        RunResult {
            stdout: std::fs::read_to_string(format!("{dir}{name}.jsonl")).unwrap(),
            exit_code: std::fs::read_to_string(format!("{dir}{name}.exit")).unwrap().trim().parse().unwrap(),
            stderr: String::new(),
        }
    }
    fn page(page: u32, count: u32, total: u32) -> RunResult {
        let apps: Vec<String> = (0..count)
            .map(|i| {
                format!(
                    "{{\"id\":{},\"bundleID\":\"com.example.app{}\",\"name\":\"App\",\"version\":\"1.0\",\"platforms\":[\"iphone\"]}}",
                    1000 + i,
                    i
                )
            })
            .collect();
        RunResult {
            stdout: format!(
                "{{\"level\":\"info\",\"count\":{count},\"totalCount\":{total},\"page\":{page},\"apps\":[{}]}}\n",
                apps.join(",")
            ),
            exit_code: 0,
            stderr: String::new(),
        }
    }
    fn session(r: &Arc<FakeRunner>, extra: &[&str]) -> Session {
        Session::new(r.clone(), extra.iter().map(|s| s.to_string()).collect()).with_delays(vec![Duration::ZERO; 3])
    }
    fn strs(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn broken_cookies() -> RunResult {
        RunResult {
            stdout: String::new(),
            exit_code: 2,
            stderr: "panic: cannot load cookies: invalid character '#' looking for beginning of value\n\ngoroutine 1 [running]:".into(),
        }
    }

    #[tokio::test]
    async fn broken_cookies_are_moved_aside_and_the_call_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let cookies = dir.path().join("cookies");
        std::fs::write(&cookies, "# Netscape HTTP Cookie File\n").unwrap();
        let r = Arc::new(FakeRunner::new(vec![broken_cookies(), fx("auth-info")]));
        let s = session(&r, &[]).with_cookies(cookies.clone());
        assert!(matches!(s.info().await, Ok(Some(_))));
        assert_eq!(r.calls().len(), 2, "повтор после того, как файл отложен");
        assert!(!cookies.exists());
        let kept: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(kept.iter().any(|n| n.starts_with("cookies.broken-")), "{kept:?}");
    }

    #[tokio::test]
    async fn broken_cookies_twice_is_one_retry_then_error() {
        let dir = tempfile::tempdir().unwrap();
        let cookies = dir.path().join("cookies");
        std::fs::write(&cookies, "#").unwrap();
        let r = Arc::new(FakeRunner::new(vec![broken_cookies(), broken_cookies()]));
        let s = session(&r, &[]).with_cookies(cookies);
        assert!(matches!(s.info().await, Err(AppError::Unknown { .. })));
        assert_eq!(r.calls().len(), 2);
    }

    #[tokio::test]
    async fn login_without_code_asks_for_code() {
        let r = Arc::new(FakeRunner::new(vec![fx("login-no-code")]));
        let res = session(&r, &[]).login("user@example.com", "pw", None).await;
        assert_eq!(res, Err(AppError::AuthCodeRequired));
        assert_eq!(
            r.calls(),
            vec![strs(&["auth", "login", "-e", "user@example.com", "-p", "pw", "--non-interactive", "--format", "json"])]
        );
    }

    #[tokio::test]
    async fn login_with_code_adds_auth_code() {
        let r = Arc::new(FakeRunner::new(vec![fx("login-ok")]));
        let acc = session(&r, &[]).login("user@example.com", "pw", Some("123456".into())).await.unwrap();
        assert_eq!(acc, Account { email: "user@example.com".into(), name: "Test User".into() });
        let c = &r.calls()[0];
        let i = c.iter().position(|a| a == "--auth-code").unwrap();
        assert_eq!(c[i + 1], "123456");
    }

    #[tokio::test]
    async fn all_purchases_pages_until_total() {
        let r = Arc::new(FakeRunner::new(vec![page(1, 100, 250), page(2, 100, 250), page(3, 50, 250)]));
        let seen = std::sync::Mutex::new(vec![]);
        let apps = session(&r, &[]).all_purchases(&|p, t| seen.lock().unwrap().push((p, t)), &|| true).await.unwrap();
        assert_eq!(apps.len(), 250);
        assert_eq!(*seen.lock().unwrap(), [(1, 250), (2, 250), (3, 250)]);
        let calls = r.calls();
        assert_eq!(calls.len(), 3);
        for (i, c) in calls.iter().enumerate() {
            assert_eq!(
                c,
                &strs(&[
                    "list-purchases", "--platform", "iphone", "-l", "100", "-p", &(i + 1).to_string(),
                    "--non-interactive", "--format", "json"
                ])
            );
        }
    }

    #[tokio::test]
    async fn stale_purchase_chain_stops_after_current_page() {
        let r = Arc::new(FakeRunner::new(vec![page(1, 100, 250), page(2, 100, 250), page(3, 50, 250)]));
        let pages = std::sync::atomic::AtomicU32::new(0);
        let res = session(&r, &[])
            .all_purchases(&|_, _| { pages.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }, &|| pages.load(std::sync::atomic::Ordering::SeqCst) < 1)
            .await;
        assert_eq!(res, Err(AppError::Unknown { detail: "purchases cancelled".into() }));
        assert_eq!(r.calls().len(), 1, "после устаревания следующую страницу не просим");
    }

    #[tokio::test]
    async fn ipatool_calls_never_overlap() {
        // как check_owned и load_purchases одновременно: вызовы идут вперемешку, но не внахлёст
        let r = Arc::new(FakeRunner::new(vec![fx("auth-info"); 4]).with_delay(Duration::from_millis(20)));
        let s = session(&r, &[]);
        let (a, b, c, d) = tokio::join!(s.info(), s.info(), s.info(), s.info());
        assert!(a.is_ok() && b.is_ok() && c.is_ok() && d.is_ok());
        assert_eq!(r.calls().len(), 4);
        assert_eq!(r.max_concurrency(), 1, "ipatool запускается по одному");
    }

    #[tokio::test]
    async fn exclusive_keeps_ipatool_waiting() {
        let r = Arc::new(FakeRunner::new(vec![fx("auth-info")]));
        let s = session(&r, &[]);
        let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));
        let l1 = log.clone();
        let busy = s.exclusive(move || {
            l1.lock().unwrap().push("swap-start");
            std::thread::sleep(Duration::from_millis(50));
            l1.lock().unwrap().push("swap-end");
        });
        let info = async {
            s.info().await.unwrap();
            log.lock().unwrap().push("info");
        };
        let (swap, _) = tokio::join!(busy, info);
        swap.unwrap();
        assert_eq!(*log.lock().unwrap(), ["swap-start", "swap-end", "info"]);
    }

    /// Висит, пока его не бросят; `dropped` — бросили (так `SidecarRunner` убивает процесс).
    struct HangRunner {
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    struct OnDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl IpaRunner for HangRunner {
        async fn run(&self, _args: &[String]) -> RunResult {
            let _guard = OnDrop(self.dropped.clone());
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    fn short() -> Limits {
        let ms = Duration::from_millis;
        Limits { quick_run: ms(100), quick_wait: ms(100), login_run: ms(100), login_wait: ms(100), download_run: ms(300), keychain_run: ms(100), keychain_hint: ms(50), keychain_wait: ms(100) }
    }

    #[tokio::test]
    async fn hung_ipatool_times_out_and_is_stopped() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = Session::new(Arc::new(HangRunner { dropped: dropped.clone() }), vec![]).with_delays(vec![]).with_limits(short());
        assert_eq!(s.info().await, Err(AppError::Timeout));
        assert!(dropped.load(Ordering::SeqCst), "процесс остановлен");
        assert_eq!(s.login("user@example.com", "pw", None).await, Err(AppError::Timeout));
        assert_eq!(s.download(1, Path::new("/tmp")).await, Err(AppError::Timeout));
    }

    #[tokio::test]
    async fn login_does_not_wait_forever_behind_a_download() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = Arc::new(Session::new(Arc::new(HangRunner { dropped }), vec![]).with_delays(vec![]).with_limits(Limits { download_run: Duration::from_secs(5), ..short() }));
        let s2 = s.clone();
        let dl = tokio::spawn(async move { s2.download(1, Path::new("/tmp")).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let t = std::time::Instant::now();
        assert_eq!(s.login("user@example.com", "pw", None).await, Err(AppError::Busy));
        assert!(t.elapsed() < Duration::from_secs(1));
        assert_eq!(AppError::Busy.user_text(), "Программа занята другой операцией (скачивание или Связка ключей) — подождите немного и попробуйте снова.");
        dl.abort();
    }

    #[tokio::test]
    async fn keychain_step_that_hangs_holds_the_lock_until_done() {
        let r = Arc::new(FakeRunner::new(vec![fx("auth-info")]));
        let s = session(&r, &[]).with_limits(Limits { quick_wait: Duration::from_secs(5), ..short() });
        let slow = s.exclusive(|| std::thread::sleep(Duration::from_millis(300))).await;
        assert_eq!(slow, Err(AppError::KeychainWaiting));
        let t = std::time::Instant::now();
        s.info().await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(150), "ipatool ждал конца записи");
    }

    /// В журнал уходит только вид ответа: в `LoggedIn` настоящие имя и адрес человека.
    #[test]
    fn unexpected_answer_names_only_the_kind() {
        let e = unexpected(&IpaOutcome::LoggedIn { email: "user@example.com".into(), name: "Real Person".into() });
        let AppError::Unknown { detail } = e else { panic!() };
        assert_eq!(detail, "неожиданный ответ ipatool: LoggedIn");
        let AppError::Unknown { detail } = unexpected(&IpaOutcome::Ok) else { panic!() };
        assert_eq!(detail, "неожиданный ответ ipatool: Ok");
    }

    #[tokio::test]
    async fn check_token_is_one_cheap_purchase_page() {
        let r = Arc::new(FakeRunner::new(vec![page(1, 1, 5)]));
        assert_eq!(session(&r, &[]).check_token().await, Ok(()));
        assert_eq!(r.calls()[0], strs(&["list-purchases", "--platform", "iphone", "-l", "1", "-p", "1", "--non-interactive", "--format", "json"]));
        let r = Arc::new(FakeRunner::new(vec![fx("purchases-logged-out")]));
        assert_eq!(session(&r, &[]).check_token().await, Err(AppError::NotLoggedIn));
    }

    #[tokio::test]
    async fn all_purchases_empty_is_one_call() {
        let r = Arc::new(FakeRunner::new(vec![page(1, 0, 0)]));
        assert_eq!(session(&r, &[]).all_purchases(&|_, _| (), &|| true).await.unwrap(), vec![]);
        assert_eq!(r.calls().len(), 1);
    }

    #[tokio::test]
    async fn all_purchases_incomplete_is_error() {
        let r = Arc::new(FakeRunner::new(vec![page(1, 2, 10), page(2, 0, 10)]));
        assert_eq!(
            session(&r, &[]).all_purchases(&|_, _| (), &|| true).await,
            Err(AppError::Unknown { detail: "purchases incomplete: got 2 of 10".into() })
        );
        assert_eq!(r.calls().len(), 2);
    }

    #[tokio::test]
    async fn login_is_not_retried() {
        let busy = RunResult {
            stdout: "{\"level\":\"error\",\"error\":\"unexpected status code 503\",\"success\":false}\n".into(),
            exit_code: 1,
            stderr: String::new(),
        };
        let r = Arc::new(FakeRunner::new(vec![busy.clone(), busy.clone(), busy]));
        let res = session(&r, &[]).login("user@example.com", "pw", Some("123456".into())).await;
        assert_eq!(res, Err(AppError::AppleUnavailable { status: Some(503) }));
        assert_eq!(r.calls().len(), 1);
    }

    #[tokio::test]
    async fn empty_stdout_uses_stderr_detail() {
        let r = Arc::new(FakeRunner::new(vec![RunResult { stdout: String::new(), exit_code: 1, stderr: "boom".into() }]));
        assert_eq!(session(&r, &[]).info().await, Err(AppError::Unknown { detail: "boom".into() }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_rejects_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let r = Arc::new(FakeRunner::new(vec![]));
        let p = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
        assert_eq!(
            session(&r, &[]).download(492224193, p).await,
            Err(AppError::Unknown { detail: "output path is not valid UTF-8".into() })
        );
        assert!(r.calls().is_empty());
    }

    #[tokio::test]
    async fn all_purchases_logged_out() {
        let r = Arc::new(FakeRunner::new(vec![fx("purchases-logged-out")]));
        assert_eq!(session(&r, &[]).all_purchases(&|_, _| (), &|| true).await, Err(AppError::NotLoggedIn));
    }

    #[tokio::test]
    async fn never_purchase_flag() {
        let r = Arc::new(FakeRunner::new(vec![
            fx("login-ok"),
            fx("auth-info"),
            page(1, 1, 1),
            fx("download-by-id"),
            fx("auth-revoke"),
        ]));
        let s = session(&r, &["--keychain-passphrase", "k"]);
        s.login("user@example.com", "pw", Some("1".into())).await.unwrap();
        s.info().await.unwrap();
        s.all_purchases(&|_, _| (), &|| true).await.unwrap();
        s.download(492224193, Path::new("/tmp/out")).await.unwrap();
        s.logout().await.unwrap();
        let calls = r.calls();
        assert_eq!(calls.len(), 5);
        assert!(calls.iter().flatten().all(|a| !a.contains("purchase") || a == "list-purchases"));
    }

    #[tokio::test]
    async fn info_not_logged_in_is_none() {
        let r = Arc::new(FakeRunner::new(vec![fx("purchases-logged-out")]));
        assert_eq!(session(&r, &[]).info().await, Ok(None));
        let r = Arc::new(FakeRunner::new(vec![fx("auth-info")]));
        assert!(session(&r, &[]).info().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn info_other_errors_propagate() {
        let r = Arc::new(FakeRunner::new(vec![RunResult { stdout: String::new(), exit_code: 1, stderr: String::new() }]));
        assert!(matches!(session(&r, &[]).info().await, Err(AppError::Unknown { .. })));
    }

    #[tokio::test]
    async fn extra_args_in_every_call() {
        let r = Arc::new(FakeRunner::new(vec![fx("auth-info"), page(1, 1, 1), fx("download-by-id"), fx("auth-revoke")]));
        let s = session(&r, &["--keychain-passphrase", "k3y"]);
        s.info().await.unwrap();
        s.all_purchases(&|_, _| (), &|| true).await.unwrap();
        s.download(492224193, Path::new("/tmp/out")).await.unwrap();
        s.logout().await.unwrap();
        for c in r.calls() {
            let i = c.iter().position(|a| a == "--keychain-passphrase").expect("нет passphrase");
            assert_eq!(c[i + 1], "k3y");
            assert!(c.contains(&"--non-interactive".to_string()));
        }
    }

    #[tokio::test]
    async fn download_args_and_path() {
        let r = Arc::new(FakeRunner::new(vec![fx("download-by-id")]));
        let p = session(&r, &[]).download(492224193, Path::new("/tmp/out")).await.unwrap();
        assert!(p.to_string_lossy().ends_with(".ipa"));
        assert_eq!(
            r.calls()[0],
            strs(&["download", "-i", "492224193", "-o", "/tmp/out", "--non-interactive", "--format", "json"])
        );
        let c = &r.calls()[0];
        assert!(!c.contains(&"-b".to_string()) && !c.iter().any(|a| a.contains("--purchase")));
    }

    fn busy() -> RunResult {
        RunResult {
            stdout: "{\"level\":\"error\",\"error\":\"unexpected status code 503\",\"success\":false}\n".into(),
            exit_code: 1,
            stderr: String::new(),
        }
    }

    #[tokio::test]
    async fn latest_version_on_live_fixtures() {
        let r = Arc::new(FakeRunner::new(vec![fx("list-versions-owned"), fx("version-metadata")]));
        assert_eq!(session(&r, &[]).latest_version(492224193).await, Ok(Some("12.15.0".into())));
        let calls = r.calls();
        assert_eq!(calls[0], strs(&["list-versions", "-i", "492224193", "--non-interactive", "--format", "json"]));
        // последний id из list-versions-owned
        assert_eq!(
            calls[1],
            strs(&[
                "get-version-metadata", "-i", "492224193", "--external-version-id", "847801982",
                "--non-interactive", "--format", "json"
            ])
        );
        assert!(calls.iter().flatten().all(|a| !a.contains("purchase")));
    }

    #[tokio::test]
    async fn latest_version_not_owned_and_errors() {
        let r = Arc::new(FakeRunner::new(vec![fx("list-versions-not-owned")]));
        assert_eq!(session(&r, &[]).latest_version(334325516).await, Err(AppError::NotPurchased));
        assert_eq!(r.calls().len(), 1, "без прав метаданные не спрашиваем");
        let r = Arc::new(FakeRunner::new(vec![fx("purchases-logged-out")]));
        assert_eq!(session(&r, &[]).latest_version(1).await, Err(AppError::NotLoggedIn));
    }

    #[tokio::test]
    async fn latest_version_metadata_failure_keeps_ownership() {
        let garbage = RunResult { stdout: "{\"level\":\"error\",\"error\":\"weird\",\"success\":false}\n".into(), exit_code: 1, stderr: String::new() };
        let r = Arc::new(FakeRunner::new(vec![fx("list-versions-owned"), garbage]));
        assert_eq!(session(&r, &[]).latest_version(492224193).await, Ok(None));
        // ошибка входа на метаданных идёт наверх
        let r = Arc::new(FakeRunner::new(vec![fx("list-versions-owned"), fx("purchases-logged-out")]));
        assert_eq!(session(&r, &[]).latest_version(492224193).await, Err(AppError::NotLoggedIn));
        // пустой список версий: права есть, версии нет
        let empty = RunResult {
            stdout: "{\"level\":\"info\",\"externalVersionIdentifiers\":[],\"bundleID\":\"\",\"success\":true}\n".into(),
            exit_code: 0,
            stderr: String::new(),
        };
        let r = Arc::new(FakeRunner::new(vec![empty]));
        assert_eq!(session(&r, &[]).latest_version(1).await, Ok(None));
        assert_eq!(r.calls().len(), 1);
    }

    #[tokio::test]
    async fn latest_version_retries_apple_unavailable() {
        let r = Arc::new(FakeRunner::new(vec![busy(), fx("list-versions-owned"), busy(), fx("version-metadata")]));
        assert_eq!(session(&r, &[]).latest_version(492224193).await, Ok(Some("12.15.0".into())));
        assert_eq!(r.calls().len(), 4);
    }

    #[tokio::test]
    async fn retries_apple_unavailable() {
        let busy = RunResult {
            stdout: "{\"level\":\"error\",\"error\":\"unexpected status code 503\",\"success\":false}\n".into(),
            exit_code: 1,
            stderr: String::new(),
        };
        let r = Arc::new(FakeRunner::new(vec![busy.clone(), busy, fx("auth-info")]));
        assert!(session(&r, &[]).info().await.unwrap().is_some());
        assert_eq!(r.calls().len(), 3);
    }
}
