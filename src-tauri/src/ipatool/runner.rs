
//! Запуск ipatool: боевой `SidecarRunner` и тестовый `FakeRunner`.
use async_trait::async_trait;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub stdout: String,
    pub exit_code: i32,
    /// Только для диагностики при пустом stdout. ipatool не печатает свои аргументы
    /// в stderr, поэтому пароль сюда не попадает.
    pub stderr: String,
}

#[async_trait]
pub trait IpaRunner: Send + Sync {
    async fn run(&self, args: &[String]) -> RunResult;
}

/// Аргументы ipatool. `Debug` скрывает секреты: у `auth login` значения `-p`/`--password`
/// и `--auth-code`, у любой команды `--keychain-passphrase`; формы `флаг=значение` тоже.
#[derive(Clone, PartialEq)]
pub struct Args(pub Vec<String>);

impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let login = self.0.len() >= 2 && self.0[0] == "auth" && self.0[1] == "login";
        let secret = |flag: &str| {
            flag == "--keychain-passphrase" || (login && matches!(flag, "-p" | "--password" | "--auth-code"))
        };
        let mut masked: Vec<String> = Vec::with_capacity(self.0.len());
        let mut hide_next = false;
        for a in &self.0 {
            if hide_next {
                masked.push("***".into());
                hide_next = false;
            } else if let Some((flag, _)) = a.split_once('=').filter(|(fl, _)| secret(fl)) {
                masked.push(format!("{flag}=***"));
            } else {
                masked.push(a.clone());
                hide_next = secret(a);
            }
        }
        write!(f, "{masked:?}")
    }
}

/// Запускает sidecar `ipatool` через tauri-plugin-shell.
/// Не покрыт юнит-тестом: проверяется живым прогоном.
pub struct SidecarRunner<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> SidecarRunner<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

#[async_trait]
impl<R: tauri::Runtime> IpaRunner for SidecarRunner<R> {
    async fn run(&self, args: &[String]) -> RunResult {
        use tauri_plugin_shell::ShellExt;
        let fail = |what: String| RunResult { stdout: String::new(), exit_code: -1, stderr: what };
        let cmd = match self.app.shell().sidecar("ipatool") {
            Ok(c) => c,
            Err(e) => return fail(format!("не удалось подготовить ipatool: {e}")),
        };
        match cmd.args(args).output().await {
            Ok(out) => RunResult {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(e) => fail(format!("не удалось запустить ipatool: {e}")),
        }
    }
}

#[cfg(test)]
pub use fake::FakeRunner;

#[cfg(test)]
mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Отдаёт заготовленные ответы по очереди и запоминает вызовы. С `with_delay` каждый вызов
    /// длится заданное время, а `max_concurrency` показывает, сколько вызовов шло одновременно.
    pub struct FakeRunner {
        responses: Mutex<VecDeque<RunResult>>,
        calls: Mutex<Vec<Vec<String>>>,
        delay: std::time::Duration,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    impl FakeRunner {
        pub fn new(responses: Vec<RunResult>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(vec![]),
                delay: std::time::Duration::ZERO,
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
            }
        }
        pub fn with_delay(mut self, d: std::time::Duration) -> Self {
            self.delay = d;
            self
        }
        pub fn max_concurrency(&self) -> usize {
            self.max_in_flight.load(Ordering::SeqCst)
        }
        pub fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl IpaRunner for FakeRunner {
        async fn run(&self, args: &[String]) -> RunResult {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            self.calls.lock().unwrap().push(args.to_vec());
            let res = self.responses.lock().unwrap().pop_front().expect("FakeRunner: ответы кончились");
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dbg(a: &[&str]) -> String {
        format!("{:?}", Args(a.iter().map(|s| s.to_string()).collect()))
    }

    #[test]
    fn debug_masks_password() {
        let d = dbg(&["auth", "login", "-e", "user@example.com", "-p", "hunter2", "--auth-code", "654321",
            "--keychain-passphrase", "k3y", "--format", "json"]);
        for s in ["hunter2", "654321", "k3y"] {
            assert!(!d.contains(s), "{d}");
        }
        assert!(d.contains("user@example.com") && d.contains("***"), "{d}");
    }

    #[test]
    fn debug_masks_long_and_equals_forms() {
        let d = dbg(&["auth", "login", "--password", "p1", "--password=p2", "-p=p3", "--auth-code=111111",
            "--keychain-passphrase=k4"]);
        for s in ["p1", "p2", "p3", "111111", "k4"] {
            assert!(!d.contains(s), "{d}");
        }
    }

    #[test]
    fn debug_keeps_p_page_in_list_purchases() {
        let d = dbg(&["list-purchases", "-l", "100", "-p", "2"]);
        assert!(d.contains("\"2\"") && !d.contains("***"), "{d}");
    }
}
