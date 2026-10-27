
//! Сессия ipatool: вход, список покупок, скачивание, выход. Повторы и разбор вывода внутри.
use super::output::{IpaOutcome, OwnedApp, parse_output};
use super::runner::{Args, IpaRunner};
use crate::errors::AppError;
use crate::retry::{APPLE_DELAYS, with_retry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
    one_at_a_time: tokio::sync::Mutex<()>,
}

impl Session {
    pub fn new(runner: Arc<dyn IpaRunner>, extra_args: Vec<String>) -> Self {
        Self { runner, extra_args, delays: APPLE_DELAYS.to_vec(), one_at_a_time: tokio::sync::Mutex::new(()) }
    }

    #[cfg(test)]
    pub(crate) fn with_delays(mut self, delays: Vec<Duration>) -> Self {
        self.delays = delays;
        self
    }

    /// Один вызов ipatool: глобальные флаги и extra_args добавляются сюда, вывод разбирается.
    async fn call(&self, mut args: Vec<String>) -> Result<IpaOutcome, AppError> {
        args.extend(["--non-interactive", "--format", "json"].map(String::from));
        args.extend(self.extra_args.iter().cloned());
        let args = Args(args);
        let res = {
            let _turn = self.one_at_a_time.lock().await;
            self.runner.run(&args.0).await
        };
        if res.stdout.trim().is_empty() && !res.stderr.trim().is_empty() {
            return Err(AppError::Unknown { detail: res.stderr.trim().chars().take(300).collect() });
        }
        parse_output(&res.stdout, res.exit_code)
    }

    /// `f` в отдельном потоке, пока ни один запуск ipatool не идёт и не начнётся: для смены
    /// записи входа и cookies (`accounts`).
    pub async fn exclusive<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> T {
        let _turn = self.one_at_a_time.lock().await;
        match tokio::task::spawn_blocking(f).await {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }

    async fn call_retry(&self, args: Vec<String>) -> Result<IpaOutcome, AppError> {
        with_retry(&self.delays, || self.call(args.clone())).await
    }

    fn account(outcome: IpaOutcome) -> Result<Account, AppError> {
        match outcome {
            IpaOutcome::LoggedIn { email, name } => Ok(Account { email, name }),
            o => Err(AppError::Unknown { detail: format!("неожиданный ответ ipatool: {o:?}") }),
        }
    }

    pub async fn login(&self, email: &str, password: &str, code: Option<String>) -> Result<Account, AppError> {
        let mut args = ["auth", "login", "-e", email, "-p", password].map(String::from).to_vec();
        if let Some(c) = code {
            args.extend(["--auth-code".to_string(), c]);
        }
        // Без повторов: пароль уходит в ipatool один раз на действие пользователя,
        // повтор мог бы сжечь одноразовый код 2FA.
        Self::account(self.call(args).await?)
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
                o => return Err(AppError::Unknown { detail: format!("неожиданный ответ ipatool: {o:?}") }),
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
            o => return Err(AppError::Unknown { detail: format!("неожиданный ответ ipatool: {o:?}") }),
        };
        let Some(last) = ids.last() else { return Ok(None) };
        let args = ["get-version-metadata", "-i", &id, "--external-version-id", last].map(String::from).to_vec();
        match self.call_retry(args).await {
            Ok(IpaOutcome::VersionMetadata { display_version }) => Ok(Some(display_version).filter(|v| !v.trim().is_empty())),
            Err(e @ (AppError::NotLoggedIn | AppError::AccountDisabled | AppError::AppleChangedLogin)) => Err(e),
            Ok(o) => {
                crate::log::write(&format!("version {track_id}"), &format!("неожиданный ответ ipatool: {o:?}"));
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
    pub async fn download(&self, track_id: i64, out_dir: &Path) -> Result<PathBuf, AppError> {
        let out = out_dir
            .to_str()
            .ok_or_else(|| AppError::Unknown { detail: "output path is not valid UTF-8".into() })?;
        let args = ["download", "-i", &track_id.to_string(), "-o", out].map(String::from).to_vec();
        match self.call_retry(args).await? {
            IpaOutcome::Downloaded { path } => Ok(path),
            o => Err(AppError::Unknown { detail: format!("неожиданный ответ ipatool: {o:?}") }),
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
        tokio::join!(busy, info);
        assert_eq!(*log.lock().unwrap(), ["swap-start", "swap-end", "info"]);
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
