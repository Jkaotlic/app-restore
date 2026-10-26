//! Apple ID, которые входили в программу, и снимки их сессий ipatool.
//!
//! ipatool помнит один аккаунт: запись `account` (на Mac — Keychain, сервис `ipatool-auth.service`;
//! на Windows — файл `~/.ipatool/account`, зашифрованный нашим `--keychain-passphrase`) и файл
//! cookies `~/.ipatool/cookies`. После каждого входа программа кладёт копию обоих в своё хранилище
//! (`app-restore.session`, пользователь — Apple ID) и при выборе аккаунта возвращает её на место.
//! Пароль не хранится. Содержимое записи и cookies не логируется и не уходит в интерфейс.
//!
//! Credential Manager на Windows держит не больше 2560 байт на запись, поэтому снимок режется на
//! куски (`CHUNK_WINDOWS`): запись `<apple id>` хранит поколение и число кусков (`v2:<g>:<N>`),
//! `<apple id>#<g>.1…N` — сами куски. Новый снимок пишется новым поколением, старый убирается
//! только после заголовка: оборванная запись не портит прежний снимок.
//! На Mac кусок один.
use crate::ipatool::session::{Account, Session};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Сервис наших снимков в Keychain / Credential Manager.
pub const SNAPSHOT_SERVICE: &str = "app-restore.session";
/// Где ipatool держит вход на Mac (99designs/keyring, бэкенд keychain).
pub const IPATOOL_SERVICE: &str = "ipatool-auth.service";
pub const IPATOOL_KEY: &str = "account";
/// Кусок снимка в Credential Manager (предел записи 2560 байт).
pub const CHUNK_WINDOWS: usize = 2048;
pub const CHUNK_MAC: usize = 1 << 20;

/// Секреты по имени пользователя внутри одного сервиса. В тестах — память.
pub trait SecretStore: Send + Sync {
    fn get(&self, user: &str) -> Result<Option<Vec<u8>>, String>;
    fn set(&self, user: &str, data: &[u8]) -> Result<(), String>;
    fn delete(&self, user: &str) -> Result<(), String>;
}

/// Keychain (Mac) / Credential Manager (Windows) через крейт `keyring`.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new(service: &str) -> Self {
        Self { service: service.to_string() }
    }
    fn entry(&self, user: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(&self.service, user).map_err(|e| format!("keyring: {e}"))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, user: &str) -> Result<Option<Vec<u8>>, String> {
        match self.entry(user)?.get_secret() {
            Ok(b) => Ok(Some(b)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("keyring: {e}")),
        }
    }
    fn set(&self, user: &str, data: &[u8]) -> Result<(), String> {
        self.entry(user)?.set_secret(data).map_err(|e| format!("keyring: {e}"))
    }
    fn delete(&self, user: &str) -> Result<(), String> {
        match self.entry(user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("keyring: {e}")),
        }
    }
}

/// Где лежит вход ipatool: запись `account` и файл cookies.
pub trait IpatoolAuth: Send + Sync {
    fn read_account(&self) -> Result<Option<Vec<u8>>, String>;
    fn write_account(&self, data: &[u8]) -> Result<(), String>;
    /// Нет записи — не ошибка.
    fn delete_account(&self) -> Result<(), String>;
    fn read_cookies(&self) -> Result<Option<Vec<u8>>, String>;
    /// Целиком, через временный файл рядом.
    fn write_cookies(&self, data: &[u8]) -> Result<(), String>;
    /// Нет файла — не ошибка.
    fn delete_cookies(&self) -> Result<(), String>;
}

/// Настоящее хранилище ipatool. `state_dir` — `~/.ipatool`.
pub struct RealAuth {
    state_dir: PathBuf,
    #[cfg(target_os = "macos")]
    keychain: KeyringStore,
}

impl RealAuth {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            #[cfg(target_os = "macos")]
            keychain: KeyringStore::new(IPATOOL_SERVICE),
        }
    }
    fn cookies(&self) -> PathBuf {
        self.state_dir.join("cookies")
    }
}

fn remove_opt(p: &Path) -> Result<(), String> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", p.display())),
    }
}

impl IpatoolAuth for RealAuth {
    #[cfg(target_os = "macos")]
    fn read_account(&self) -> Result<Option<Vec<u8>>, String> {
        self.keychain.get(IPATOOL_KEY)
    }
    #[cfg(target_os = "macos")]
    fn write_account(&self, data: &[u8]) -> Result<(), String> {
        self.keychain.set(IPATOOL_KEY, data)
    }
    #[cfg(target_os = "macos")]
    fn delete_account(&self) -> Result<(), String> {
        self.keychain.delete(IPATOOL_KEY)
    }
    /// Windows: файловый бэкенд ipatool, файл `account` в `~/.ipatool`.
    #[cfg(not(target_os = "macos"))]
    fn read_account(&self) -> Result<Option<Vec<u8>>, String> {
        read_opt(&self.state_dir.join(IPATOOL_KEY))
    }
    #[cfg(not(target_os = "macos"))]
    fn write_account(&self, data: &[u8]) -> Result<(), String> {
        write_atomic(&self.state_dir.join(IPATOOL_KEY), data)
    }
    #[cfg(not(target_os = "macos"))]
    fn delete_account(&self) -> Result<(), String> {
        remove_opt(&self.state_dir.join(IPATOOL_KEY))
    }
    fn read_cookies(&self) -> Result<Option<Vec<u8>>, String> {
        read_opt(&self.cookies())
    }
    fn write_cookies(&self, data: &[u8]) -> Result<(), String> {
        write_atomic(&self.cookies(), data)
    }
    fn delete_cookies(&self) -> Result<(), String> {
        remove_opt(&self.cookies())
    }
}

/// Вход ipatool целиком: запись `account` и cookies (`None` — нет).
#[derive(Debug, Clone, PartialEq)]
pub struct Raw {
    pub account: Option<Vec<u8>>,
    pub cookies: Option<Vec<u8>>,
}

/// Файл целиком; нет файла — `None`.
fn read_opt(p: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

/// Запись целиком через временный файл рядом и перенос поверх: читатель видит старый или новый файл.
fn write_atomic(p: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = p.parent().ok_or("no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.app-restore.tmp"));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, p)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("write {name}: {e}"))
}

/// Ключ аккаунта: без пробелов по краям и без регистра.
pub fn key(email: &str) -> String {
    email.trim().to_lowercase()
}

pub fn same_id(a: &str, b: &str) -> bool {
    !key(a).is_empty() && key(a) == key(b)
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    v: u32,
    account: String,
    cookies: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Known {
    email: String,
    /// Снимок сессии есть (без самого снимка — его в этом файле нет).
    saved: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    accounts: Vec<Known>,
    /// «Забыли»: при запуске такой вход ipatool не подхватывается, пока не войдут в него явно.
    #[serde(default)]
    forgotten: Vec<String>,
}

/// Строка экрана «Apple ID».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountEntry {
    pub email: String,
    /// Копий в папке этого Apple ID.
    pub copies: usize,
    /// «Вход сохранён»: есть снимок или это нынешний вход.
    pub saved: bool,
    pub current: bool,
}

/// Список аккаунтов: кто входил (`known`, со снимком или без) плюс папки с копиями (`counts`,
/// имя папки — `library::account_dir_name`). Нынешний вход — всегда в списке и «сохранён».
pub fn account_list(known: &[(String, bool)], counts: &[(String, usize)], current: Option<&str>) -> Vec<AccountEntry> {
    let folder = |e: &str| crate::library::account_dir_name(e).ok();
    let count = |e: &str| folder(e).and_then(|f| counts.iter().find(|(n, _)| *n == f)).map_or(0, |c| c.1);
    let mut out: Vec<AccountEntry> = known
        .iter()
        .map(|(e, saved)| AccountEntry { email: e.clone(), copies: count(e), saved: *saved, current: false })
        .collect();
    if let Some(c) = current.filter(|c| !out.iter().any(|a| same_id(&a.email, c))) {
        out.push(AccountEntry { email: c.to_string(), copies: count(c), saved: true, current: false });
    }
    for (f, n) in counts {
        if !out.iter().any(|a| folder(&a.email).as_deref() == Some(f)) {
            out.push(AccountEntry { email: f.clone(), copies: *n, saved: false, current: false });
        }
    }
    for a in &mut out {
        if current.is_some_and(|c| same_id(&a.email, c)) {
            a.current = true;
            a.saved = true;
        }
    }
    out.sort_by_key(|a| key(&a.email));
    out
}

/// Снимки сессий и список Apple ID (`accounts.json` в папке данных программы: только адреса).
pub struct Sessions {
    index_path: PathBuf,
    store: Arc<dyn SecretStore>,
    auth: Arc<dyn IpatoolAuth>,
    chunk: usize,
    /// Чтение и запись `accounts.json` по одной.
    index_lock: Mutex<()>,
}

impl Sessions {
    pub fn new(index_path: PathBuf, store: Arc<dyn SecretStore>, auth: Arc<dyn IpatoolAuth>, chunk: usize) -> Self {
        Self { index_path, store, auth, chunk: chunk.max(1), index_lock: Mutex::new(()) }
    }

    /// Боевое: снимки в Keychain / Credential Manager, ipatool в `~/.ipatool`.
    pub fn real(index_path: PathBuf, home: &Path) -> Self {
        let chunk = if cfg!(windows) { CHUNK_WINDOWS } else { CHUNK_MAC };
        Self::new(index_path, Arc::new(KeyringStore::new(SNAPSHOT_SERVICE)), Arc::new(RealAuth::new(home.join(".ipatool"))), chunk)
    }

    fn load(&self) -> Index {
        std::fs::read(&self.index_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn edit(&self, f: impl FnOnce(&mut Index)) {
        let _one = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut ix = self.load();
        f(&mut ix);
        match serde_json::to_vec_pretty(&ix) {
            Ok(b) => {
                if let Err(e) = write_atomic(&self.index_path, &b) {
                    eprintln!("[app-restore] accounts index: {e}");
                }
            }
            Err(e) => eprintln!("[app-restore] accounts index: {e}"),
        }
    }

    /// Кто входил: (адрес, есть ли снимок).
    pub fn known(&self) -> Vec<(String, bool)> {
        let _one = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().accounts.into_iter().map(|k| (k.email, k.saved)).collect()
    }

    /// Запомнить адрес (без снимка, если его ещё нет).
    /// Явный вход: адрес в списке (без снимка, если его ещё нет), отметка «забыли» снята.
    pub fn remember(&self, email: &str) {
        self.edit(|ix| {
            ix.forgotten.retain(|f| !same_id(f, email));
            if !ix.accounts.iter().any(|k| same_id(&k.email, email)) {
                ix.accounts.push(Known { email: email.trim().to_string(), saved: false });
            }
        });
    }

    /// Этот Apple ID «забыли» и с тех пор в него явно не входили.
    pub fn is_forgotten(&self, email: &str) -> bool {
        let _one = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().forgotten.iter().any(|f| same_id(f, email))
    }

    fn set_saved(&self, email: &str, saved: bool) {
        self.edit(|ix| match ix.accounts.iter_mut().find(|k| same_id(&k.email, email)) {
            Some(k) => k.saved = saved,
            None => ix.accounts.push(Known { email: email.trim().to_string(), saved }),
        });
    }

    /// Кусок `i` поколения `generation` (`None` — первый формат `v1`, куски `<id>#i`).
    fn chunk_user(k: &str, generation: Option<u64>, i: usize) -> String {
        match generation {
            Some(g) => format!("{k}#{g}.{i}"),
            None => format!("{k}#{i}"),
        }
    }

    /// Заголовок снимка: поколение и число кусков. `v2:<поколение>:<N>`, старый `v1:<N>`.
    fn header(&self, k: &str) -> Result<Option<(Option<u64>, usize)>, String> {
        let Some(h) = self.store.get(k)? else { return Ok(None) };
        let h = std::str::from_utf8(&h).map_err(|_| "bad snapshot header")?;
        let parsed = if let Some(n) = h.strip_prefix("v1:") {
            n.parse().ok().map(|n| (None, n))
        } else {
            h.strip_prefix("v2:").and_then(|r| r.split_once(':')).and_then(|(g, n)| Some((Some(g.parse().ok()?), n.parse().ok()?)))
        };
        parsed.map(Some).ok_or_else(|| "bad snapshot header".to_string())
    }

    /// Новый снимок рядом со старым: куски нового поколения, затем заголовок, затем старые куски
    /// убираются. Запись оборвалась — заголовок и куски старого снимка целы, новые убраны.
    fn put_blob(&self, k: &str, blob: &[u8]) -> Result<(), String> {
        let old = self.header(k).ok().flatten();
        let generation = old.and_then(|o| o.0).map_or(1, |g| g.wrapping_add(1));
        let parts: Vec<&[u8]> = blob.chunks(self.chunk).collect();
        let undo = |upto: usize| {
            for i in 1..=upto {
                let _ = self.store.delete(&Self::chunk_user(k, Some(generation), i));
            }
        };
        for (i, c) in parts.iter().enumerate() {
            if let Err(e) = self.store.set(&Self::chunk_user(k, Some(generation), i + 1), c) {
                undo(i);
                return Err(e);
            }
        }
        if let Err(e) = self.store.set(k, format!("v2:{generation}:{}", parts.len()).as_bytes()) {
            undo(parts.len());
            return Err(e);
        }
        if let Some((g, n)) = old {
            for i in 1..=n {
                let _ = self.store.delete(&Self::chunk_user(k, g, i));
            }
        }
        Ok(())
    }

    fn get_blob(&self, k: &str) -> Result<Option<Vec<u8>>, String> {
        let Some((g, n)) = self.header(k)? else { return Ok(None) };
        let mut out = Vec::new();
        for i in 1..=n {
            out.extend(self.store.get(&Self::chunk_user(k, g, i))?.ok_or("snapshot chunk missing")?);
        }
        Ok(Some(out))
    }

    fn drop_blob(&self, k: &str) -> Result<(), String> {
        let old = self.header(k).ok().flatten();
        self.store.delete(k)?;
        if let Some((g, n)) = old {
            for i in 1..=n {
                self.store.delete(&Self::chunk_user(k, g, i))?;
            }
        }
        Ok(())
    }

    /// Нынешний вход ipatool целиком.
    pub fn capture(&self) -> Result<Raw, String> {
        Ok(Raw { account: self.auth.read_account()?, cookies: self.auth.read_cookies()? })
    }

    /// Поставить вход ipatool: сначала запись `account`, потом cookies. `None` — убрать.
    pub fn apply(&self, raw: &Raw) -> Result<(), String> {
        match &raw.account {
            Some(a) => self.auth.write_account(a)?,
            None => self.auth.delete_account()?,
        }
        match &raw.cookies {
            Some(c) => self.auth.write_cookies(c),
            None => self.auth.delete_cookies(),
        }
    }

    /// Вернуть вход ipatool к `before`; не вышло — убрать его совсем, чтобы ipatool был чистым,
    /// а не наполовину чужим. `true` — вход как до смены.
    pub fn rollback(&self, before: &Raw) -> bool {
        match self.apply(before) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[app-restore] rollback session: {e}");
                let a = self.auth.delete_account();
                let c = self.auth.delete_cookies();
                if let Err(e) = a.and(c) {
                    eprintln!("[app-restore] clear session: {e}");
                }
                false
            }
        }
    }

    /// Снимок нынешнего входа ipatool под адресом `email`. Записи `account` нет — ошибка.
    pub fn save(&self, email: &str) -> Result<(), String> {
        let raw = self.capture()?;
        let account = raw.account.ok_or("ipatool has no account")?;
        let snap = Snapshot { v: 1, account: STANDARD.encode(account), cookies: raw.cookies.map(|c| STANDARD.encode(c)) };
        let blob = serde_json::to_vec(&snap).map_err(|e| e.to_string())?;
        self.put_blob(&key(email), &blob)?;
        self.set_saved(email, true);
        Ok(())
    }

    /// Снимок `email` (без записи куда-либо). `Ok(None)` — снимка нет.
    pub fn snapshot_of(&self, email: &str) -> Result<Option<Raw>, String> {
        let Some(blob) = self.get_blob(&key(email))? else { return Ok(None) };
        let snap: Snapshot = serde_json::from_slice(&blob).map_err(|_| "bad snapshot")?;
        let account = STANDARD.decode(&snap.account).map_err(|_| "bad snapshot")?;
        let cookies = snap.cookies.map(|c| STANDARD.decode(c)).transpose().map_err(|_| "bad snapshot")?;
        Ok(Some(Raw { account: Some(account), cookies }))
    }

    #[cfg(test)]
    /// Вернуть снимок `email` на место ipatool. `Ok(false)` — снимка нет. Если в снимке не было
    /// cookies, нынешние убираются (они чужого аккаунта).
    pub fn restore(&self, email: &str) -> Result<bool, String> {
        let Some(raw) = self.snapshot_of(email)? else { return Ok(false) };
        self.apply(&raw)?;
        Ok(true)
    }

    /// Снимок не того аккаунта или испорчен: убрать его, адрес остаётся «нужен вход».
    pub fn drop_snapshot(&self, email: &str) {
        if let Err(e) = self.drop_blob(&key(email)) {
            eprintln!("[app-restore] drop snapshot: {e}");
        }
        self.set_saved(email, false);
    }

    /// «Забыть»: убрать снимок и адрес из списка. Копии не трогаются.
    pub fn forget(&self, email: &str) -> Result<(), String> {
        self.drop_blob(&key(email))?;
        self.edit(|ix| {
            ix.accounts.retain(|k| !same_id(&k.email, email));
            if !ix.forgotten.iter().any(|f| same_id(f, email)) {
                ix.forgotten.push(key(email));
            }
        });
        Ok(())
    }

    /// Снимок испорчен или не подошёл: в списке «нужен вход».
    pub fn mark_unsaved(&self, email: &str) {
        self.set_saved(email, false);
    }
}

/// Итог выбора аккаунта: вошли снимком или нужен вход с подставленным адресом.
#[derive(Debug, Clone, PartialEq)]
pub struct UseOutcome {
    pub restored: bool,
    /// Вход ipatool после смены (при `restored`).
    pub account: Option<Account>,
    /// Не вошли, но вход ipatool как до смены (ничего не тронули или откатили). `false` при
    /// неудаче — ipatool чистый, без входа.
    pub kept: bool,
    pub error: Option<crate::errors::AppError>,
}

/// Работа с хранилищами под замком ipatool: ни один его запуск не читает вход, пока его меняем.
async fn quiet<T: Send + 'static>(session: &Session, f: impl FnOnce() -> T + Send + 'static) -> T {
    session.exclusive(f).await
}

/// Снимок нынешнего входа под `email` (до того, как ipatool его перезапишет). Ошибки — в лог;
/// `false` — снимок не сохранился.
pub async fn save_current(session: &Session, sessions: &Arc<Sessions>, email: &str) -> bool {
    let (s, e) = (sessions.clone(), email.to_string());
    match quiet(session, move || s.save(&e)).await {
        Ok(()) => true,
        Err(err) => {
            eprintln!("[app-restore] save session: {err}");
            false
        }
    }
}

/// Ошибки, при которых вход из снимка мёртв: Apple просит войти заново.
fn stale(e: &crate::errors::AppError) -> bool {
    use crate::errors::AppError::*;
    matches!(e, NotLoggedIn | AccountDisabled | AppleChangedLogin | WrongCredentials | CodeRejected | AuthCodeRequired)
}

/// Перейти на аккаунт `email`, как одна операция:
/// 1. нынешний вход (`current`, другой адрес) сохраняется снимком; не вышло — ничего не меняем,
///    `SaveSessionFailed`;
/// 2. вход ipatool запоминается как есть, снимок `email` ставится на место (запись `account`,
///    потом cookies);
/// 3. `auth info` должен вернуть тот же адрес (без регистра), затем `list-purchases -l 1` у Apple
///    подтверждает, что вход живой.
///
/// Любой сбой записи, чужой адрес или мёртвый вход — откат к запомненному (`kept: true`); откат
/// не удался — вход ipatool убирается совсем (`kept: false`). Чужой адрес — снимок удаляется,
/// мёртвый вход — «нужен вход». Сеть недоступна на шаге 3 — вход оставляем.
pub async fn switch_to(session: &Session, sessions: &Arc<Sessions>, current: Option<&str>, email: &str) -> UseOutcome {
    let kept = |error| UseOutcome { restored: false, account: None, kept: true, error };
    if let Some(cur) = current {
        if same_id(cur, email) {
            if let Ok(Some(acc)) = session.info().await
                && same_id(&acc.email, email)
            {
                return UseOutcome { restored: true, account: Some(acc), kept: false, error: None };
            }
        } else {
            let (s, c) = (sessions.clone(), cur.to_string());
            if let Err(err) = quiet(session, move || s.save(&c)).await {
                eprintln!("[app-restore] save session: {err}");
                return kept(Some(crate::errors::AppError::SaveSessionFailed));
            }
        }
    }
    let (s, e) = (sessions.clone(), email.to_string());
    let snap = match quiet(session, move || s.snapshot_of(&e)).await {
        Ok(Some(raw)) => raw,
        Ok(None) => return kept(None),
        Err(err) => {
            eprintln!("[app-restore] restore session: {err}");
            sessions.drop_snapshot(email);
            return kept(None);
        }
    };
    let s = sessions.clone();
    let before = match quiet(session, move || s.capture()).await {
        Ok(raw) => raw,
        Err(err) => {
            eprintln!("[app-restore] restore session: {err}");
            return kept(None);
        }
    };
    let rollback = |before: Raw| {
        let s = sessions.clone();
        async move {
            let ok = quiet(session, move || s.rollback(&before)).await;
            UseOutcome { restored: false, account: None, kept: ok, error: None }
        }
    };
    let s = sessions.clone();
    if let Err(err) = quiet(session, move || s.apply(&snap)).await {
        eprintln!("[app-restore] restore session: {err}");
        return rollback(before).await;
    }
    let acc = match session.info().await {
        Ok(Some(acc)) if same_id(&acc.email, email) => acc,
        Ok(Some(_)) => {
            sessions.drop_snapshot(email);
            return rollback(before).await;
        }
        other => {
            if let Err(e) = other {
                eprintln!("[app-restore] restore session: {e}");
            }
            sessions.mark_unsaved(email);
            return rollback(before).await;
        }
    };
    match session.check_token().await {
        Err(e) if stale(&e) => {
            sessions.mark_unsaved(email);
            rollback(before).await
        }
        other => {
            if let Err(e) = other {
                // сеть или серверы Apple: вход оставляем, его проверит первый же запрос
                eprintln!("[app-restore] token check: {e}");
            }
            UseOutcome { restored: true, account: Some(acc), kept: false, error: None }
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// Хранилище в памяти; `max` — предел записи, как у Credential Manager; `fail_sets` — запись падает.
    #[derive(Default)]
    pub struct MemStore {
        pub items: Mutex<HashMap<String, Vec<u8>>>,
        pub max: Option<usize>,
        pub fail_sets: AtomicBool,
        /// Сколько записей ещё пройдёт, дальше падают (`None` — без предела).
        pub sets_left: Mutex<Option<usize>>,
    }
    impl SecretStore for MemStore {
        fn get(&self, user: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.items.lock().unwrap().get(user).cloned())
        }
        fn set(&self, user: &str, data: &[u8]) -> Result<(), String> {
            if self.fail_sets.load(Ordering::SeqCst) {
                return Err("keychain denied".into());
            }
            if let Some(n) = self.sets_left.lock().unwrap().as_mut() {
                if *n == 0 {
                    return Err("keychain full".into());
                }
                *n -= 1;
            }
            if self.max.is_some_and(|m| data.len() > m) {
                return Err("too long".into());
            }
            self.items.lock().unwrap().insert(user.into(), data.to_vec());
            Ok(())
        }
        fn delete(&self, user: &str) -> Result<(), String> {
            self.items.lock().unwrap().remove(user);
            Ok(())
        }
    }

    /// Вход ipatool в памяти. `fail_*` — сколько следующих записей упадёт.
    #[derive(Default)]
    pub struct MemAuth {
        pub account: Mutex<Option<Vec<u8>>>,
        pub cookies: Mutex<Option<Vec<u8>>>,
        pub fail_account: AtomicU32,
        pub fail_cookies: AtomicU32,
    }
    fn take(n: &AtomicU32) -> bool {
        n.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1)).is_ok()
    }
    impl MemAuth {
        pub fn state(&self) -> Raw {
            Raw { account: self.account.lock().unwrap().clone(), cookies: self.cookies.lock().unwrap().clone() }
        }
    }
    impl IpatoolAuth for MemAuth {
        fn read_account(&self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.account.lock().unwrap().clone())
        }
        fn write_account(&self, data: &[u8]) -> Result<(), String> {
            if take(&self.fail_account) {
                return Err("keychain write failed".into());
            }
            *self.account.lock().unwrap() = Some(data.to_vec());
            Ok(())
        }
        fn delete_account(&self) -> Result<(), String> {
            *self.account.lock().unwrap() = None;
            Ok(())
        }
        fn read_cookies(&self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.cookies.lock().unwrap().clone())
        }
        fn write_cookies(&self, data: &[u8]) -> Result<(), String> {
            if take(&self.fail_cookies) {
                return Err("disk full".into());
            }
            *self.cookies.lock().unwrap() = Some(data.to_vec());
            Ok(())
        }
        fn delete_cookies(&self) -> Result<(), String> {
            *self.cookies.lock().unwrap() = None;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{MemAuth, MemStore};
    use super::*;
    use crate::errors::AppError;
    use crate::ipatool::runner::{FakeRunner, RunResult};
    use std::sync::atomic::Ordering;

    const ME: &str = "me@example.com";
    const OTHER: &str = "other@example.com";

    struct Env {
        _t: tempfile::TempDir,
        store: Arc<MemStore>,
        auth: Arc<MemAuth>,
        s: Arc<Sessions>,
        index: PathBuf,
    }

    fn env(max: Option<usize>, chunk: usize) -> Env {
        let t = tempfile::tempdir().unwrap();
        let store = Arc::new(MemStore { max, ..Default::default() });
        let auth = Arc::new(MemAuth::default());
        let index = t.path().join("data/accounts.json");
        let s = Arc::new(Sessions::new(index.clone(), store.clone(), auth.clone(), chunk));
        Env { _t: t, store, auth, s, index }
    }

    fn raw(token: &[u8], cookies: Option<&[u8]>) -> Raw {
        Raw { account: Some(token.to_vec()), cookies: cookies.map(<[u8]>::to_vec) }
    }

    impl Env {
        /// ipatool вошёл в аккаунт: запись и cookies.
        fn ipatool_as(&self, token: &[u8], cookies: Option<&[u8]>) {
            *self.auth.account.lock().unwrap() = Some(token.to_vec());
            *self.auth.cookies.lock().unwrap() = cookies.map(<[u8]>::to_vec);
        }
        fn cookies(&self) -> Option<Vec<u8>> {
            self.auth.cookies.lock().unwrap().clone()
        }
        /// ME вошёл (ipatool), у OTHER есть снимок.
        fn me_now_other_saved(&self) {
            self.ipatool_as(b"token-other", Some(b"c-other"));
            self.s.save(OTHER).unwrap();
            self.ipatool_as(b"token-me", Some(b"c-me"));
        }
        fn has_blob(&self, email: &str) -> bool {
            self.store.items.lock().unwrap().contains_key(email)
        }
    }

    #[test]
    fn snapshot_roundtrip_per_account() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", Some(b"cookie-me"));
        e.s.save(ME).unwrap();
        e.ipatool_as(b"token-other", Some(b"cookie-other"));
        e.s.save("Other@Example.com").unwrap();
        assert_eq!(e.s.restore("ME@example.com "), Ok(true));
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"cookie-me")));
        assert_eq!(e.s.restore(OTHER), Ok(true));
        assert_eq!(e.auth.state(), raw(b"token-other", Some(b"cookie-other")));
        assert_eq!(e.s.known(), vec![(ME.to_string(), true), ("Other@Example.com".to_string(), true)]);
    }

    #[test]
    fn restore_without_snapshot_changes_nothing() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", Some(b"cookie-me"));
        assert_eq!(e.s.restore(OTHER), Ok(false));
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"cookie-me")));
    }

    #[test]
    fn snapshot_without_cookies_removes_foreign_cookies() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", None);
        e.s.save(ME).unwrap();
        e.ipatool_as(b"token-other", Some(b"cookie-other"));
        assert_eq!(e.s.restore(ME), Ok(true));
        assert!(e.cookies().is_none());
    }

    #[test]
    fn save_without_ipatool_account_is_error() {
        let e = env(None, CHUNK_MAC);
        assert!(e.s.save(ME).is_err());
        assert!(e.store.items.lock().unwrap().is_empty());
        assert!(e.s.known().is_empty());
    }

    #[test]
    fn real_auth_cookies_are_files_written_whole() {
        let t = tempfile::tempdir().unwrap();
        let a = RealAuth::new(t.path().join(".ipatool"));
        assert_eq!(a.read_cookies(), Ok(None));
        a.write_cookies(b"c1").unwrap();
        a.write_cookies(b"c2").unwrap();
        assert_eq!(a.read_cookies(), Ok(Some(b"c2".to_vec())));
        let left: Vec<_> = std::fs::read_dir(t.path().join(".ipatool")).unwrap().map(|x| x.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("cookies")], "без временных файлов");
        a.delete_cookies().unwrap();
        a.delete_cookies().unwrap();
        assert_eq!(a.read_cookies(), Ok(None));
    }

    #[test]
    fn big_snapshot_is_split_for_credential_manager() {
        let e = env(Some(2560), CHUNK_WINDOWS);
        let token: Vec<u8> = (0..6000u32).map(|i| (i % 251) as u8).collect();
        let cookies = vec![b'c'; 9000];
        e.ipatool_as(&token, Some(&cookies));
        e.s.save(ME).unwrap();
        assert!(e.store.items.lock().unwrap().len() > 2, "кусками");
        assert!(e.store.items.lock().unwrap().values().all(|v| v.len() <= 2560));
        e.ipatool_as(b"x", Some(b"y"));
        assert_eq!(e.s.restore(ME), Ok(true));
        assert_eq!(e.auth.state(), Raw { account: Some(token), cookies: Some(cookies) });
        e.ipatool_as(b"small", None);
        e.s.save(ME).unwrap();
        assert_eq!(e.store.items.lock().unwrap().len(), 2);
        e.s.forget(ME).unwrap();
        assert!(e.store.items.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_save_keeps_previous_snapshot() {
        for allowed in 0..4 {
            let e = env(Some(2560), CHUNK_WINDOWS);
            e.ipatool_as(b"old-token", Some(b"old-cookie"));
            e.s.save(ME).unwrap();
            let before: usize = e.store.items.lock().unwrap().len();
            // новый снимок в 3+ куска; запись обрывается на `allowed`-м
            e.ipatool_as(&vec![b't'; 3000], Some(&vec![b'c'; 3000]));
            *e.store.sets_left.lock().unwrap() = Some(allowed);
            assert!(e.s.save(ME).is_err(), "allowed={allowed}");
            *e.store.sets_left.lock().unwrap() = None;
            e.ipatool_as(b"x", None);
            assert_eq!(e.s.restore(ME), Ok(true), "allowed={allowed}");
            assert_eq!(e.auth.state(), raw(b"old-token", Some(b"old-cookie")), "allowed={allowed}");
            assert_eq!(e.s.known(), vec![(ME.to_string(), true)], "список не врёт");
            assert_eq!(e.store.items.lock().unwrap().len(), before, "недописанные куски убраны");
        }
    }

    #[test]
    fn broken_snapshot_is_error_not_panic() {
        let e = env(None, CHUNK_MAC);
        e.store.set(ME, b"v1:2").unwrap();
        e.store.set(&format!("{ME}#1"), b"{").unwrap();
        assert!(e.s.restore(ME).is_err(), "кусок пропал");
        e.store.set(&format!("{ME}#2"), b"garbage").unwrap();
        assert!(e.s.restore(ME).is_err());
        e.store.set(ME, b"nonsense").unwrap();
        assert!(e.s.restore(ME).is_err());
    }

    #[test]
    fn forget_removes_only_that_account_and_marks_it() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", Some(b"c1"));
        e.s.save(ME).unwrap();
        e.ipatool_as(b"token-other", Some(b"c2"));
        e.s.save(OTHER).unwrap();
        e.s.forget("Me@Example.com").unwrap();
        assert_eq!(e.s.restore(ME), Ok(false));
        assert_eq!(e.s.known(), vec![(OTHER.to_string(), true)]);
        assert_eq!(e.s.restore(OTHER), Ok(true));
        // «забыли» до явного входа
        assert!(e.s.is_forgotten(ME) && !e.s.is_forgotten(OTHER));
        let again = Sessions::new(e.index.clone(), e.store.clone(), e.auth.clone(), CHUNK_MAC);
        assert!(again.is_forgotten("ME@example.com"), "переживает перезапуск");
        e.s.remember(ME);
        assert!(!e.s.is_forgotten(ME));
    }

    #[test]
    fn index_holds_addresses_only() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"SECRET-TOKEN", Some(b"SECRET-COOKIE"));
        e.s.save(ME).unwrap();
        e.s.remember(OTHER);
        e.s.remember("OTHER@example.com");
        let text = std::fs::read_to_string(&e.index).unwrap();
        assert!(!text.contains("SECRET") && !text.contains(&STANDARD.encode("SECRET-TOKEN")), "{text}");
        assert_eq!(e.s.known(), vec![(ME.to_string(), true), (OTHER.to_string(), false)]);
        std::fs::write(&e.index, b"{oops").unwrap();
        assert!(e.s.known().is_empty());
        // старый файл без `forgotten` читается
        std::fs::write(&e.index, br#"{"accounts":[{"email":"me@example.com","saved":true}]}"#).unwrap();
        assert_eq!(e.s.known(), vec![(ME.to_string(), true)]);
    }

    #[test]
    fn account_list_merges_logins_folders_and_current() {
        let known = vec![(ME.to_string(), true), ("Old@Example.com".to_string(), false)];
        let counts = vec![(ME.to_string(), 15), ("old@example.com".to_string(), 2), ("folder@example.com".to_string(), 3)];
        let l = account_list(&known, &counts, Some("now@example.com"));
        let row = |e: &str| l.iter().find(|a| a.email == e).unwrap().clone();
        assert_eq!(l.len(), 4);
        assert_eq!(row(ME), AccountEntry { email: ME.into(), copies: 15, saved: true, current: false });
        assert_eq!(row("Old@Example.com"), AccountEntry { email: "Old@Example.com".into(), copies: 2, saved: false, current: false });
        assert_eq!(row("folder@example.com"), AccountEntry { email: "folder@example.com".into(), copies: 3, saved: false, current: false });
        assert_eq!(row("now@example.com"), AccountEntry { email: "now@example.com".into(), copies: 0, saved: true, current: true });
        let l = account_list(&known, &counts, Some("OLD@example.com"));
        assert!(l.iter().any(|a| a.email == "Old@Example.com" && a.current && a.saved));
        assert_eq!(l.len(), 3);
        assert!(account_list(&[], &[], None).is_empty());
    }

    fn info(email: &str) -> RunResult {
        RunResult {
            stdout: format!("{{\"level\":\"info\",\"name\":\"Test User\",\"email\":\"{email}\",\"success\":true}}\n"),
            exit_code: 0,
            stderr: String::new(),
        }
    }
    /// Ответ `list-purchases -l 1`: вход живой.
    fn token_ok() -> RunResult {
        RunResult { stdout: "{\"level\":\"info\",\"count\":0,\"totalCount\":0,\"page\":1,\"apps\":[]}\n".into(), exit_code: 0, stderr: String::new() }
    }
    fn logged_out() -> RunResult {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ipatool/");
        RunResult { stdout: std::fs::read_to_string(format!("{dir}purchases-logged-out.jsonl")).unwrap(), exit_code: 1, stderr: String::new() }
    }
    fn busy() -> RunResult {
        RunResult { stdout: "{\"level\":\"error\",\"error\":\"unexpected status code 503\",\"success\":false}\n".into(), exit_code: 1, stderr: String::new() }
    }

    fn session(r: &Arc<FakeRunner>) -> Session {
        Session::new(r.clone(), vec![]).with_delays(vec![std::time::Duration::ZERO; 3])
    }

    #[tokio::test]
    async fn switch_restores_checks_email_and_token() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        let r = Arc::new(FakeRunner::new(vec![info("Other@Example.com"), token_ok()]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(out.restored && out.error.is_none());
        assert_eq!(out.account.unwrap().email, "Other@Example.com");
        assert_eq!(e.auth.state(), raw(b"token-other", Some(b"c-other")));
        assert_eq!(r.calls()[1][0], "list-purchases", "токен проверен у Apple");
        // снимок ME успел сохраниться: можно вернуться
        let r = Arc::new(FakeRunner::new(vec![info(ME), token_ok()]));
        assert!(switch_to(&session(&r), &e.s, Some(OTHER), ME).await.restored);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
    }

    #[tokio::test]
    async fn switch_without_snapshot_needs_login() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", Some(b"c-me"));
        let r = Arc::new(FakeRunner::new(vec![]));
        let out = switch_to(&session(&r), &e.s, None, OTHER).await;
        assert!(!out.restored && out.kept && out.error.is_none());
        assert!(r.calls().is_empty(), "ipatool не звали");
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
    }

    #[tokio::test]
    async fn save_failure_aborts_before_touching_ipatool() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        e.store.fail_sets.store(true, Ordering::SeqCst);
        let r = Arc::new(FakeRunner::new(vec![]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert_eq!((out.restored, out.kept, out.error), (false, true, Some(AppError::SaveSessionFailed)));
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
        assert!(e.has_blob(OTHER), "снимок OTHER на месте");
        assert!(r.calls().is_empty());
    }

    #[tokio::test]
    async fn account_write_failure_rolls_back() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        e.auth.fail_account.store(1, Ordering::SeqCst);
        let r = Arc::new(FakeRunner::new(vec![]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(!out.restored && out.kept);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
    }

    #[tokio::test]
    async fn cookies_write_failure_rolls_back() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        e.auth.fail_cookies.store(1, Ordering::SeqCst);
        let r = Arc::new(FakeRunner::new(vec![]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(!out.restored && out.kept);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")), "запись account уже была — откатили");
    }

    #[tokio::test]
    async fn failed_rollback_leaves_ipatool_clean() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        e.auth.fail_account.store(2, Ordering::SeqCst);
        let r = Arc::new(FakeRunner::new(vec![]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(!out.restored && !out.kept);
        assert_eq!(e.auth.state(), Raw { account: None, cookies: None });
    }

    #[tokio::test]
    async fn email_mismatch_rolls_back_and_drops_snapshot() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        let r = Arc::new(FakeRunner::new(vec![info("someone@example.com")]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(!out.restored && out.kept);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
        assert!(!e.has_blob(OTHER), "чужой снимок убран");
        assert!(e.s.known().contains(&(OTHER.to_string(), false)), "адрес остался «нужен вход»");
    }

    #[tokio::test]
    async fn stale_token_online_rolls_back() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        let r = Arc::new(FakeRunner::new(vec![info(OTHER), logged_out()]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(!out.restored && out.kept);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
        assert!(e.s.known().contains(&(OTHER.to_string(), false)));
        // и «войдите» уже на auth info — тоже откат
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        let r = Arc::new(FakeRunner::new(vec![logged_out()]));
        assert!(!switch_to(&session(&r), &e.s, Some(ME), OTHER).await.restored);
        assert_eq!(e.auth.state(), raw(b"token-me", Some(b"c-me")));
    }

    #[tokio::test]
    async fn network_error_on_token_check_keeps_restore() {
        let e = env(None, CHUNK_MAC);
        e.me_now_other_saved();
        let r = Arc::new(FakeRunner::new(vec![info(OTHER), busy(), busy(), busy(), busy()]));
        let out = switch_to(&session(&r), &e.s, Some(ME), OTHER).await;
        assert!(out.restored);
        assert_eq!(e.auth.state(), raw(b"token-other", Some(b"c-other")));
    }

    #[tokio::test]
    async fn switch_to_current_account_is_instant() {
        let e = env(None, CHUNK_MAC);
        e.ipatool_as(b"token-me", None);
        let r = Arc::new(FakeRunner::new(vec![info(ME)]));
        let out = switch_to(&session(&r), &e.s, Some("ME@example.com"), ME).await;
        assert!(out.restored);
        assert!(e.store.items.lock().unwrap().is_empty(), "снимков не трогали");
        assert_eq!(r.calls().len(), 1);
    }
}
