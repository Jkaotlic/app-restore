//! Команды Tauri для интерфейса. Логика, которую стоит проверять, вынесена в `logic`;
//! здесь тонкая склейка: состояние, вызовы модулей, события.
//!
//! События: `job` {track_id, state}, `owned` {track_id, owned, error, available_version,
//! install_state, checked}, `phones` {phones, error}, `purchases_progress` {page, pages},
//! `purchases` (RowsDto).
//!
//! Apple ID покупателя с телефона — личные данные: в логи не пишется, интерфейс получает маску,
//! полный адрес — только как подстановку в поле входа (`switch_account`).
//!
//! Аккаунты (`list_accounts`, `use_account`, `forget_account`): снимки сессий ipatool в
//! Keychain / Credential Manager (`accounts`). Копии (`all_copies`, `pick_ipa`, `install_copy`,
//! `copy_account_match`): любая ipa из «Мои приложения» на любой телефон, без входа тоже.
//!
//! «С этого iPhone» строится по телефону-источнику — любому подключённому (`set_source_phone`),
//! цель установки та же. «Сообщить в каталог» (`report_to_catalog`): issue на GitHub, адрес
//! строит и проверяет Rust.
pub mod logic;

use crate::accounts::{Sessions, account_list, same_id};
use crate::device::{AccountMatch, DeviceHub, InstalledApp, PhoneInfo, account_match, idevice_impl::IdeviceHub};
use crate::errors::AppError;
use crate::ipatool::runner::SidecarRunner;
use crate::ipatool::session::{Account, Session};
use crate::library::Library;
use crate::lookup::{Fetch, HttpLookup, Lookup, ReqwestFetch, app_name};
use crate::queue::{FsSpace, JobState, Queue};
use crate::removed_catalog::{RemovedApp, bundled, fetch_catalog};
use logic::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::RwLock;

const PHONE_POLL: Duration = Duration::from_secs(2);

/// Пишет ошибку в журнал (`crate::log`), с подробностью `Unknown`. Секретов в ней нет:
/// ipatool не печатает свои аргументы, а пароль в `AppError` не попадает; журнал ещё и чистит.
fn log_err(context: &str, e: &AppError) {
    match error_ref(e) {
        Some(r) => crate::log::write(context, &format!("[{r}] {e}")),
        None => crate::log::write(context, &format!("{e}")),
    }
}

/// Подсказка про окно Связки ключей macOS — событие `keychain_note` (текст), видно на любом
/// экране. Ожидание при этом идёт дальше; `done` убирает подсказку, если её показал этот шаг.
fn keychain_hint<R: Runtime>(app: &AppHandle<R>) -> HintScope<'static, impl Fn(Option<String>) + Send + Sync + use<R>> {
    static ACTIVE: AtomicUsize = AtomicUsize::new(0);
    let app = app.clone();
    HintScope::new(&ACTIVE, move |note: Option<String>| {
        let _ = app.emit("keychain_note", note);
    })
}

fn ui(context: &str, e: AppError) -> UiError {
    log_err(context, &e);
    e.into()
}

/// Телефон-источник «С этого iPhone»: выбор человека и тот, по которому построена группа.
#[derive(Default)]
struct SourcePick {
    chosen: Option<String>,
    current: Option<String>,
}

#[derive(Default)]
struct Phones {
    /// Выбран человеком (`choose_phone`); главнее автоматического выбора.
    chosen: Option<String>,
    target: Option<String>,
    last: Vec<PhoneInfo>,
    /// Сбоев опроса подряд (см. `after_poll`).
    failures: u32,
}

/// Для экрана «Готово»: на тот же ли Apple ID вошли на телефоне и какой ID у покупателя.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunDto {
    pub same_apple_id: bool,
    pub email: Option<String>,
}

pub struct AppState {
    session: Arc<Session>,
    sessions: Arc<Sessions>,
    queue: Arc<Queue>,
    library: Arc<Library>,
    hub: Arc<dyn DeviceHub>,
    lookup: Arc<dyn Lookup>,
    fetch: Arc<dyn Fetch>,
    catalog_cache: PathBuf,
    catalog: RwLock<Vec<RemovedApp>>,
    /// Итоги `check_owned` по записям каталога: строки «Выбора» их подставляют.
    owned: Mutex<HashMap<i64, bool>>,
    /// Последние версии у Apple (`latest_version`) по trackId.
    latest: Mutex<HashMap<i64, String>>,
    /// Размеры из Lookup (`fileSizeBytes`) по trackId: для «≈ размер» на «Выборе».
    sizes: Mutex<HashMap<i64, u64>>,
    /// Что стоит на телефоне (udid, список) из `installed_apps`; `None` — без телефона.
    installed: Mutex<Option<(String, Vec<InstalledApp>)>>,
    /// Что стоит на каждом прочитанном телефоне (udid → список): источник группы «С этого iPhone»
    /// может быть любым подключённым, не только целью.
    phone_lists: Mutex<HashMap<String, Vec<InstalledApp>>>,
    /// Телефон-источник «С этого iPhone».
    source: Mutex<SourcePick>,
    /// trackId строк «Выбора» вне группы «С этого iPhone» после последней загрузки покупок:
    /// по ним `set_source_phone` отбирает строки другого источника. `None` — покупки не загружены.
    phone_taken: Mutex<Option<HashSet<i64>>>,
    /// Последний итог `phone_account` (udid, сравнение): для подстановки адреса в `switch_account`.
    phone_match: Mutex<Option<(String, AccountMatch)>>,
    /// Последнее состояние каждого задания: для «Повторить» и установки после подключения телефона.
    jobs: Mutex<HashMap<i64, JobState>>,
    names: Mutex<HashMap<i64, String>>,
    /// Версия сохранённой копии, поставленной вместо скачивания (`fallback_copy`).
    from_saved: Mutex<HashMap<i64, String>>,
    /// Задания «Все копии» (ключ `copy_job_id`): файл и телефон. «Повторить» ставит туда же.
    copy_paths: Mutex<HashMap<i64, (PathBuf, String)>>,
    /// trackId, которые сейчас скачиваются или ждут скачивания (`downloads_running`).
    downloads: Mutex<HashSet<i64>>,
    /// Задания «Только скачать»: заканчиваются на `Saved`, на телефон не ставятся.
    download_only: Mutex<HashSet<i64>>,
    account: Mutex<Option<Account>>,
    run: Mutex<Option<RunDto>>,
    phones: Mutex<Phones>,
    /// Новый `check_owned` останавливает предыдущий.
    owned_gen: AtomicU64,
    /// Новый `load_purchases` (и выход) отменяет итог предыдущего.
    purchases_gen: AtomicU64,
}

impl AppState {
    /// Нужен контекст tokio: очередь сразу запускает рабочую задачу.
    fn new<R: Runtime>(app: &AppHandle<R>) -> Self {
        let extra = match crate::passphrase::keychain_passphrase() {
            Ok(Some(p)) => vec!["--keychain-passphrase".to_string(), p],
            Ok(None) => vec![],
            Err(e) => {
                log_err("keychain passphrase", &e);
                vec![]
            }
        };
        let cookies = dirs::home_dir().map(|h| h.join(".ipatool").join("cookies"));
        let mut session = Session::new(Arc::new(SidecarRunner::new(app.clone())), extra);
        if let Some(c) = cookies {
            session = session.with_cookies(c);
        }
        let session = Arc::new(session);
        let data_dir = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("app-restore"));
        let home = dirs::home_dir().unwrap_or_else(|| data_dir.clone());
        let sessions = Arc::new(Sessions::real(data_dir.join("accounts.json"), &home));
        let root = Library::default_root().unwrap_or_else(|| data_dir.join("Мои приложения"));
        let library = Arc::new(Library::new(root));
        library.sweep_partials();
        // Копии, скачанные до папок по аккаунтам, — в папки их покупателей. Повтор безопасен.
        // Сотни мегабайт на файл при переносе между томами: не в потоке интерфейса.
        let lib = library.clone();
        tauri::async_runtime::spawn_blocking(move || lib.migrate());
        let hub: Arc<dyn DeviceHub> = Arc::new(IdeviceHub);
        let queue = Arc::new(Queue::new(session.clone(), library.clone(), Arc::new(FsSpace), hub.clone()));
        Self {
            session,
            sessions,
            queue,
            library,
            hub,
            lookup: Arc::new(HttpLookup::new()),
            fetch: Arc::new(ReqwestFetch::new()),
            catalog_cache: data_dir.join("catalog-cache.json"),
            catalog: RwLock::new(bundled()),
            owned: Mutex::new(HashMap::new()),
            latest: Mutex::new(HashMap::new()),
            sizes: Mutex::new(HashMap::new()),
            installed: Mutex::new(None),
            phone_lists: Mutex::new(HashMap::new()),
            source: Mutex::new(SourcePick::default()),
            phone_taken: Mutex::new(None),
            phone_match: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            from_saved: Mutex::new(HashMap::new()),
            copy_paths: Mutex::new(HashMap::new()),
            downloads: Mutex::new(HashSet::new()),
            download_only: Mutex::new(HashSet::new()),
            account: Mutex::new(None),
            run: Mutex::new(None),
            phones: Mutex::new(Phones::default()),
            owned_gen: AtomicU64::new(0),
            purchases_gen: AtomicU64::new(0),
        }
    }

    /// Снимок всего, из чего строятся строки «Выбора»; `f` получает его как `RowCtx`.
    async fn with_ctx<T>(&self, f: impl FnOnce(&RowCtx) -> T) -> T {
        let catalog = self.catalog.read().await.clone();
        let owned = self.owned.lock().unwrap().clone();
        let latest = self.latest.lock().unwrap().clone();
        let sizes = self.sizes.lock().unwrap().clone();
        let account = self.current_email();
        let installed = self.installed.lock().unwrap().clone().map(|(_, apps)| apps);
        // источник и его список — из одного снимка: им же помечаются строки группы
        let source_udid = self.source.lock().unwrap().current.clone();
        let source = source_udid.as_ref().and_then(|u| self.phone_lists.lock().unwrap().get(u).cloned());
        let saved = self.library.list();
        let unknown = self.library.list_unknown();
        let target_iphone = {
            let p = self.phones.lock().unwrap();
            p.target.as_deref().is_some_and(|t| p.last.iter().any(|d| d.udid == t && d.device_class == "iPhone"))
        };
        f(&RowCtx { catalog: &catalog, owned: &owned, latest: &latest, installed: installed.as_deref(), saved: &saved, unknown: &unknown, target_iphone, sizes: &sizes, account: account.as_deref(), source: source.as_deref(), source_udid: source_udid.as_deref() })
    }

    /// Что стоит на телефоне, с запоминанием. Во время установки телефон не трогаем — прошлый список.
    async fn installed_for(&self, udid: &str) -> Result<Vec<InstalledApp>, AppError> {
        if !should_probe(&self.jobs.lock().unwrap()) {
            return Ok(cached_installed(self.installed.lock().unwrap().as_ref(), udid));
        }
        let apps = self.hub.installed(udid).await?;
        *self.installed.lock().unwrap() = Some((udid.to_string(), apps.clone()));
        self.phone_lists.lock().unwrap().insert(udid.to_string(), apps.clone());
        Ok(apps)
    }

    /// Что стоит на любом телефоне (источник «С этого iPhone»): цель установки (`installed`) не
    /// меняется. Во время установки телефон не трогаем — прошлый список.
    async fn installed_any(&self, udid: &str) -> Result<Vec<InstalledApp>, AppError> {
        if !should_probe(&self.jobs.lock().unwrap()) {
            let cached = self.phone_lists.lock().unwrap().get(udid).cloned();
            return Ok(cached.unwrap_or_else(|| cached_installed(self.installed.lock().unwrap().as_ref(), udid)));
        }
        let apps = self.hub.installed(udid).await?;
        // источник — та же цель: «на iPhone» и группа из одного чтения
        refresh_target(&mut self.installed.lock().unwrap(), udid, &apps);
        self.phone_lists.lock().unwrap().insert(udid.to_string(), apps.clone());
        Ok(apps)
    }

    /// Списки приложений всех подключённых телефонов (порядок опроса). Не прочитался — без него.
    async fn connected_lists(&self) -> Vec<(String, Vec<InstalledApp>)> {
        let udids: Vec<String> = self.phones.lock().unwrap().last.iter().map(|p| p.udid.clone()).collect();
        let mut out = vec![];
        for u in udids {
            match self.installed_any(&u).await {
                Ok(apps) => out.push((u, apps)),
                Err(e) => log_err("source phone", &e),
            }
        }
        out
    }

    /// Выход из Apple ID: отменить идущие проверки и забыть права и версии прошлого аккаунта.
    async fn sign_out(&self) -> Result<(), AppError> {
        self.clear_account_caches();
        self.session.logout().await?;
        self.set_account(None);
        Ok(())
    }

    /// Права и версии прошлого аккаунта больше не верны: отменить проверки и забыть их.
    fn clear_account_caches(&self) {
        self.owned_gen.fetch_add(1, Ordering::SeqCst);
        self.purchases_gen.fetch_add(1, Ordering::SeqCst);
        self.owned.lock().unwrap().clear();
        self.latest.lock().unwrap().clear();
        *self.phone_taken.lock().unwrap() = None;
    }

    /// Смена аккаунта: убрать ждущие задания (`Saved` и `Queued`, которые очередь ещё не взяла;
    /// идущую установку не трогаем) и итог прошлого запуска. Возвращает убранные trackId.
    fn drop_waiting(&self) -> Vec<i64> {
        let gone = {
            // под замком заданий: `take_waiting` не переведёт `Saved` в очередь между шагами
            let mut jobs = self.jobs.lock().unwrap();
            let queue_dropped = self.queue.drop_pending();
            drop_pending(&mut jobs, &queue_dropped)
        };
        let (mut names, mut fs, mut dl) = (self.names.lock().unwrap(), self.from_saved.lock().unwrap(), self.downloads.lock().unwrap());
        let mut only = self.download_only.lock().unwrap();
        for id in &gone {
            names.remove(id);
            fs.remove(id);
            dl.remove(id);
            only.remove(id);
        }
        *self.run.lock().unwrap() = None;
        gone
    }

    /// Смена входа ipatool на `target` разрешена? Не во время скачивания по аккаунту.
    fn may_switch(&self, target: &str) -> Result<(), AppError> {
        let jobs = self.jobs.lock().unwrap();
        switch_guard(self.current_email().as_deref(), target, &jobs, &self.downloads.lock().unwrap())
    }

    /// Задания `ids` ставятся на скачивание.
    fn mark_downloads(&self, ids: impl IntoIterator<Item = i64>) {
        self.downloads.lock().unwrap().extend(ids);
    }

    fn current_email(&self) -> Option<String> {
        self.account.lock().unwrap().as_ref().map(|a| a.email.clone())
    }

    /// Выбранный человеком телефон (`choose_phone`); итог — куда теперь ставим.
    fn choose<R: Runtime>(&self, app: &AppHandle<R>, udid: String) -> Option<String> {
        let target = {
            let mut p = self.phones.lock().unwrap();
            p.chosen = Some(udid);
            resolve_target(&p.last, p.chosen.as_deref())
        };
        self.apply_target(app, target.clone());
        target
    }

    /// Аккаунт программы и его папка в «Мои приложения». Без папки копии выключены.
    fn set_account(&self, acc: Option<Account>) {
        if let Err(e) = self.library.set_account(acc.as_ref().map(|a| a.email.as_str())) {
            log_err("library account", &e);
        }
        *self.account.lock().unwrap() = acc;
    }

    fn emit_job<R: Runtime>(&self, app: &AppHandle<R>, track_id: i64, state: &JobState) {
        let from_saved = self.from_saved.lock().unwrap().get(&track_id).cloned();
        let _ = app.emit("job", JobEvent { track_id, state: job_dto(state), from_saved });
    }

    fn record<R: Runtime>(&self, app: &AppHandle<R>, track_id: i64, state: JobState) {
        track_download(&mut self.downloads.lock().unwrap(), track_id, &state);
        let only = self.download_only.lock().unwrap().contains(&track_id);
        if let JobState::Failed { error, saved: None } = &state {
            log_err(&format!("job {track_id}"), error);
            // «Только скачать» старую копию на телефон не ставит: пусть будет видна ошибка
            if let Some((path, version)) = fallback_copy(error, track_id, &self.library.list()).filter(|_| !only) {
                crate::log::write(&format!("job {track_id}"), &format!("скачать не вышло, ставим сохранённую копию {version}"));
                self.from_saved.lock().unwrap().insert(track_id, version);
                self.install_copy(app, track_id, path);
                return;
            }
        } else if let JobState::Failed { error, .. } = &state {
            log_err(&format!("job {track_id}"), error);
        }
        self.emit_job(app, track_id, &state);
        self.jobs.lock().unwrap().insert(track_id, state);
    }

    /// Ставит готовую ipa: сразу, если телефон выбран, иначе ждёт его в `Saved` (`apply_target`).
    fn install_copy<R: Runtime>(&self, app: &AppHandle<R>, track_id: i64, path: PathBuf) {
        if self.phones.lock().unwrap().target.is_some() {
            self.record(app, track_id, JobState::Queued);
            self.queue.install_saved(track_id, path);
        } else {
            self.record(app, track_id, JobState::Saved { path });
        }
    }

    /// Новая цель установки. Когда телефон появился, ставятся задания, ждущие в `Saved`.
    fn apply_target<R: Runtime>(&self, app: &AppHandle<R>, target: Option<String>) {
        let changed = {
            let mut p = self.phones.lock().unwrap();
            let changed = p.target != target;
            p.target = target.clone();
            changed
        };
        self.queue.set_target(target.clone());
        if changed && target.is_some() {
            // Выборка и пометка `Queued` под одним замком: параллельный вызов не поставит дважды.
            let only = self.download_only.lock().unwrap().clone();
            let waiting = take_waiting(&mut self.jobs.lock().unwrap(), &only);
            for (id, path) in waiting {
                self.emit_job(app, id, &JobState::Queued);
                self.queue.install_saved(id, path);
            }
        }
    }
}

/// Создаёт состояние и запускает фоновые задачи: пересылку событий очереди, опрос телефонов,
/// обновление каталога.
pub fn setup<R: Runtime>(app: &AppHandle<R>) {
    let state = tauri::async_runtime::block_on(async { AppState::new(app) });
    let mut rx = state.queue.subscribe();
    app.manage(state);

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok((id, st)) => h.state::<AppState>().record(&h, id, st),
                // Пропущены лишь промежуточные события прогресса; итог придёт следом.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let _ = poll_phones(&h).await;
            tokio::time::sleep(PHONE_POLL).await;
        }
    });

    let h = app.clone();
    tauri::async_runtime::spawn(async move { refresh(&h.state::<AppState>()).await });
}

async fn poll_phones<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<PhoneDto>, UiError> {
    let st = app.state::<AppState>();
    let probe = should_probe(&st.jobs.lock().unwrap());
    // Во время установки lockdown не трогаем: прошлый список остаётся в силе.
    let outcome = if probe { Some(st.hub.list().await) } else { None };
    let (phones, chosen, error) = {
        let mut p = st.phones.lock().unwrap();
        let mut error = None;
        if let Some(res) = outcome {
            let o = after_poll(&p.last, p.failures, res);
            (p.last, p.failures, error) = (o.phones, o.failures, o.error);
        }
        (p.last.clone(), p.chosen.clone(), error)
    };
    let target = resolve_target(&phones, chosen.as_deref());
    st.apply_target(app, target.clone());
    let dtos = phone_dtos(&phones, target.as_deref());
    let error = error.map(|e| ui("phones", e));
    let _ = app.emit("phones", PhonesEvent { phones: dtos.clone(), error: error.clone() });
    match error {
        Some(e) => Err(e),
        None => Ok(dtos),
    }
}

async fn refresh(st: &AppState) {
    let apps = fetch_catalog(st.fetch.as_ref(), &st.catalog_cache).await;
    *st.catalog.write().await = apps;
}

// Пароль приходит только сюда и уходит в ipatool один раз. Аргументы команд нигде не логируются.
// Шаги входа пишутся в журнал (адрес маской): следующее зависание будет видно, на каком шаге.
#[tauri::command]
pub async fn login(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    password: String,
    code: Option<String>,
    skip_save: Option<bool>,
) -> Result<LoginDto, UiError> {
    let email = email.trim().to_string();
    let who = crate::device::mask_email(&email);
    crate::log::write("login", &format!("{who}: start{}", if code.is_some() { " with code" } else { "" }));
    // Вход ipatool сейчас перезапишется: прежний аккаунт сохраняем снимком, чтобы вернуться без пароля.
    let before = state.current_email();
    state.may_switch(&email).map_err(|e| ui("login", e))?;
    if before.is_some() {
        crate::log::write("login", &format!("{who}: saving previous session"));
    }
    // окно разрешения macOS: подсказка на экране, ожидание идёт дальше
    let hint = keychain_hint(&app);
    let prepared = crate::accounts::prepare_login(&state.session, &state.sessions, before.as_deref(), &email, skip_save.unwrap_or(false), &|| hint.on_slow()).await;
    hint.done();
    prepared.map_err(|e| ui("login", e))?;
    crate::log::write("login", &format!("{who}: asking Apple"));
    // `password` уходит из памяти вместе с аргументами по выходу из функции (без затирания).
    let acc = state.session.login(&email, &password, normalize_code(code)).await.map_err(|e| ui("login", e))?;
    crate::log::write("login", &format!("{who}: signed in"));
    state.sessions.remember(&acc.email);
    // Снимок нового входа — в фоне: окно разрешения macOS не держит экран на «Входим…».
    // Первое чтение записи ipatool этой программой — тут: окно macOS вероятнее всего здесь,
    // поэтому подсказка та же, что перед входом (на «Выборе» видна в боковой панели).
    let (session, sessions, saved_email, h) = (state.session.clone(), state.sessions.clone(), acc.email.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        let hint = keychain_hint(&h);
        let r = crate::accounts::save_current_hinted(&session, &sessions, &saved_email, &|| hint.on_slow()).await;
        hint.done();
        if let Err(e) = r {
            log_err("login: save new session", &e);
        }
    });
    // другой Apple ID: как при `use_account` — права, версии и ждущие задания прошлого не годятся
    let dropped = if account_changed(before.as_deref(), Some(&acc.email)) {
        state.clear_account_caches();
        state.drop_waiting()
    } else {
        vec![]
    };
    let lib = state.library.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || lib.migrate()).await;
    state.set_account(Some(acc.clone()));
    Ok(LoginDto { account: acc, dropped })
}

/// Итог `login`: аккаунт (поля `email`, `name` в корне) и trackId убранных заданий.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoginDto {
    #[serde(flatten)]
    pub account: Account,
    pub dropped: Vec<i64>,
}

#[tauri::command]
pub async fn session_info(state: State<'_, AppState>) -> Result<Option<Account>, ()> {
    let acc = match state.session.info().await {
        Ok(a) => a,
        Err(e) => {
            log_err("session_info", &e);
            None
        }
    };
    // «Забытый» Apple ID молча не подхватываем: до явного входа программа без входа
    let acc = acc.filter(|a| !state.sessions.is_forgotten(&a.email));
    if let Some(a) = &acc {
        state.sessions.remember(&a.email);
    }
    state.set_account(acc.clone());
    Ok(acc)
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), UiError> {
    // выход посреди скачивания: следующие задания упали бы «Вы не вошли»
    if downloads_running(&state.jobs.lock().unwrap(), &state.downloads.lock().unwrap()) {
        return Err(ui("logout", AppError::DownloadsRunning));
    }
    state.sign_out().await.map_err(|e| ui("logout", e))
}

/// Итог `switch_account`: адрес телефона для поля входа (если он известен и другой) и trackId
/// убранных заданий — интерфейс убирает только их.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwitchDto {
    pub prefill: Option<String>,
    pub dropped: Vec<i64>,
}

/// «Войти в Apple ID этого iPhone»: выход, сброс ждущих заданий (`Saved` и `Queued`, которые
/// очередь ещё не взяла; идущую установку не трогаем), прав, версий и итога прошлого запуска.
/// Адрес не логируется.
#[tauri::command]
pub async fn switch_account(app: AppHandle, state: State<'_, AppState>) -> Result<SwitchDto, UiError> {
    // выход из ipatool посреди скачивания: скачивание осталось бы без входа
    if downloads_running(&state.jobs.lock().unwrap(), &state.downloads.lock().unwrap()) {
        return Err(ui("switch_account", AppError::DownloadsRunning));
    }
    let prefill = {
        let target = state.phones.lock().unwrap().target.clone();
        let m = state.phone_match.lock().unwrap();
        login_prefill(m.as_ref().filter(|(u, _)| Some(u) == target.as_ref()).map(|(_, m)| m))
    };
    let gone = state.drop_waiting();
    // Прежний вход остаётся в списке «Apple ID» снимком (не вышло — «нужен вход»).
    if let Some(cur) = state.current_email() {
        let hint = keychain_hint(&app);
        let r = crate::accounts::save_current_hinted(&state.session, &state.sessions, &cur, &|| hint.on_slow()).await;
        hint.done();
        if let Err(e) = r {
            log_err("switch_account: save session", &e);
            state.sessions.mark_unsaved(&cur);
        }
    }
    // Не вышло выйти — всё равно ведём на вход: новый вход заменит сессию.
    if let Err(e) = state.sign_out().await {
        log_err("switch_account", &e);
    }
    state.set_account(None);
    Ok(SwitchDto { prefill, dropped: gone })
}

/// Свежий каталог с GitHub (кэш, затем вшитая копия). Вызывается и сам при запуске.
#[tauri::command]
pub async fn refresh_catalog(state: State<'_, AppState>) -> Result<(), ()> {
    refresh(&state).await;
    Ok(())
}

/// Каталог и сохранённые копии — сразу, без обращения к Apple. Покупки догружает `load_purchases`.
#[tauri::command]
pub async fn load_catalog_rows(state: State<'_, AppState>) -> Result<RowsDto, ()> {
    Ok(state.with_ctx(catalog_rows).await)
}

/// Покупки в фоне: `purchases_progress` после каждой страницы, в конце `purchases` (весь список).
/// Если покупки не загрузились, в `purchases` каталог, копии и `purchases_error`.
#[tauri::command]
pub async fn load_purchases(app: AppHandle, state: State<'_, AppState>) -> Result<(), ()> {
    let generation = state.purchases_gen.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let current = || st.purchases_gen.load(Ordering::SeqCst) == generation;
        let progress = |page: u32, total: u32| {
            if current() {
                let _ = app.emit("purchases_progress", PurchasesProgress { page, pages: purchase_pages(total) });
            }
        };
        let result = st.session.all_purchases(&progress, &current).await;
        if !current() {
            return; // устаревшая цепочка: её итог никому не нужен
        }
        let rows = match result {
            Ok(purchases) => {
                // одним Lookup и покупки, и кандидаты «С этого iPhone» (нет в каталоге и покупках)
                let mut ids: Vec<i64> = purchases.iter().map(|a| a.id).collect();
                let taken: HashSet<i64> = st.catalog.read().await.iter().map(|r| r.track_id).chain(ids.iter().copied()).collect();
                // источник группы — любой подключённый телефон: кандидаты всех в том же Lookup
                let lists = st.connected_lists().await;
                let mut seen: HashSet<i64> = ids.iter().copied().collect();
                for (_, apps) in &lists {
                    ids.extend(phone_candidates(apps, &taken).into_iter().filter(|id| seen.insert(*id)));
                }
                let present = match st.lookup.present_in(&ids, "ru").await {
                    Ok(p) => {
                        st.sizes.lock().unwrap().extend(p.sizes.iter().map(|(k, v)| (*k, *v)));
                        Some(p)
                    }
                    Err(e) => {
                        log_err("lookup ru", &e);
                        None
                    }
                };
                let udids: Vec<String> = lists.iter().map(|(u, _)| u.clone()).collect();
                let target = st.phones.lock().unwrap().target.clone();
                // без Lookup группы нет, и считать строки по телефонам не по чему: цель или первый
                let default = match &present {
                    Some(p) => default_source(&udids, target.as_deref(), &source_counts(&lists, &taken, &p.ids)),
                    None => target.filter(|t| udids.contains(t)).or_else(|| udids.first().cloned()),
                };
                {
                    let mut s = st.source.lock().unwrap();
                    s.current = resolve_source(s.chosen.as_deref(), &udids, default);
                }
                // помечены источником, по которому построены (`with_ctx`): интерфейс сверяет его со своим выбором
                let rows = st.with_ctx(|ctx| purchase_rows(ctx, purchases, present.as_ref().map(|p| &p.ids))).await;
                *st.phone_taken.lock().unwrap() = Some(rows.rows.iter().filter(|r| r.source != "phone").map(|r| r.track_id).collect());
                rows
            }
            Err(e) => {
                // Без покупок список всё равно нужен: каталог и сохранённые копии ставятся и без входа.
                log_err("load_purchases", &e);
                st.with_ctx(|ctx| rows_without_purchases(ctx, e)).await
            }
        };
        if current() {
            let _ = app.emit("purchases", rows);
        }
    });
    Ok(())
}

/// Что стоит на телефоне. Запоминается: по нему строки получают «На iPhone» и состояние.
/// Во время установки телефон не трогаем — отдаём прошлый список.
#[tauri::command]
pub async fn installed_apps(state: State<'_, AppState>, udid: String) -> Result<Vec<InstalledDto>, UiError> {
    state.installed_for(&udid).await.map_err(|e| ui("installed_apps", e))
}

/// Чей Apple ID на телефоне против Apple ID программы (по покупателям установленных приложений).
/// Берёт тот же список, что `installed_apps`, и так же не трогает телефон во время установки.
/// Сбой чтения — `Unknown`: интерфейс тогда задаёт прежний вопрос.
#[tauri::command]
pub async fn phone_account(state: State<'_, AppState>, udid: String) -> Result<AccountMatchDto, ()> {
    let program = state.account.lock().unwrap().as_ref().map(|a| a.email.clone()).unwrap_or_default();
    let m = match state.installed_for(&udid).await {
        Ok(apps) => account_match(crate::device::phone_account(&apps).as_deref(), &program),
        Err(e) => {
            log_err("phone_account", &e);
            AccountMatch::Unknown
        }
    };
    let dto = account_match_dto(&m);
    *state.phone_match.lock().unwrap() = Some((udid, m));
    Ok(dto)
}

/// «Телефон-источник»: группа «С этого iPhone» по другому подключённому телефону. Цель установки
/// не меняется. Отдаёт новые строки группы (после загрузки покупок; до неё — ошибка).
#[tauri::command]
pub async fn set_source_phone(state: State<'_, AppState>, udid: String) -> Result<SourceRowsDto, UiError> {
    let udid = copy_device(&state.phones.lock().unwrap().last, &udid).map_err(|e| ui("set_source_phone", e))?;
    let taken = state.phone_taken.lock().unwrap().clone();
    let taken = taken.ok_or_else(|| ui("set_source_phone", AppError::Unknown { detail: "purchases not loaded".into() }))?;
    let apps = state.installed_any(&udid).await.map_err(|e| ui("set_source_phone", e))?;
    let present = state.lookup.present_in(&phone_candidates(&apps, &taken), "ru").await.map_err(|e| ui("set_source_phone", e))?;
    state.sizes.lock().unwrap().extend(present.sizes.iter().map(|(k, v)| (*k, *v)));
    {
        let mut s = state.source.lock().unwrap();
        (s.chosen, s.current) = (Some(udid.clone()), Some(udid));
    }
    Ok(state.with_ctx(|ctx| source_rows(ctx, &taken, &present.ids)).await)
}

/// «Сообщить в каталог»: заполненный issue на GitHub в браузере. Адрес строит и проверяет Rust
/// (`report_url`, `is_report_url`): разрешения opener у интерфейса не расширяются.
#[tauri::command]
pub fn report_to_catalog(app: AppHandle, track_id: i64, name: String, version: String, bundle_id: String) -> Result<(), UiError> {
    use tauri_plugin_opener::OpenerExt;
    let url = report_url(track_id, &name, &version, &bundle_id);
    if !is_report_url(&url) {
        return Err(ui("report_to_catalog", AppError::Unknown { detail: "report url rejected".into() }));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| ui("report_to_catalog", AppError::Unknown { detail: format!("open url: {e}") }))
}

/// «Продолжить без iPhone»: строки без колонки «На iPhone».
#[tauri::command]
pub fn clear_installed(state: State<'_, AppState>) {
    *state.installed.lock().unwrap() = None;
}

/// Права и последняя версия по одной записи (~2–4 с каждая) в фоне; итоги приходят событиями
/// `owned`. Пустой список просто останавливает идущую проверку.
#[tauri::command]
pub async fn check_owned(app: AppHandle, state: State<'_, AppState>, track_ids: Vec<i64>) -> Result<(), ()> {
    let generation = state.owned_gen.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        for id in track_ids {
            if st.owned_gen.load(Ordering::SeqCst) != generation {
                return;
            }
            let r = owned_result(st.session.latest_version(id).await);
            if let Ok((owned, latest)) = &r {
                st.owned.lock().unwrap().insert(id, *owned);
                if let Some(v) = latest {
                    st.latest.lock().unwrap().insert(id, v.clone());
                }
            }
            let stop = r.as_ref().err().is_some_and(stops_owned_check);
            if let Err(e) = &r {
                log_err(&format!("owns {id}"), e);
            }
            let ev = st.with_ctx(|ctx| owned_event(id, &r, ctx)).await;
            if st.owned_gen.load(Ordering::SeqCst) == generation {
                let _ = app.emit("owned", ev);
            }
            if stop {
                return;
            }
        }
    });
    Ok(())
}

/// «В App Store»: поиск по российскому магазину (Search API). Отдаёт строки, которых ещё нет на
/// «Выборе»; права проверяет `check_owned`. Скачивание — по trackId, покупки (`--purchase`) нет.
#[tauri::command]
pub async fn search_store(state: State<'_, AppState>, term: String) -> Result<Vec<RowDto>, UiError> {
    let found = crate::lookup::search(state.fetch.as_ref(), &term).await.map_err(|e| ui("search_store", e))?;
    let found = relevant(found, &term);
    state.sizes.lock().unwrap().extend(found.iter().filter_map(|a| Some((a.track_id, a.size?))));
    let screen = state.phone_taken.lock().unwrap().clone();
    Ok(state.with_ctx(|ctx| store_rows(found, &store_taken(ctx, screen.as_ref()), ctx)).await)
}

#[tauri::command]
pub async fn add_by_link(state: State<'_, AppState>, link: String) -> Result<RowDto, UiError> {
    let id = crate::app_link::parse_app_link(&link).ok_or_else(|| ui("add_by_link", AppError::BadLink))?;
    let latest = state.session.latest_version(id).await.map_err(|e| ui("add_by_link", e))?;
    state.owned.lock().unwrap().insert(id, true);
    if let Some(v) = latest {
        state.latest.lock().unwrap().insert(id, v);
    }
    let in_catalog = state.catalog.read().await.iter().any(|r| r.track_id == id);
    let looked_up = if in_catalog { None } else { app_name(state.fetch.as_ref(), id).await };
    Ok(state.with_ctx(|ctx| link_row(id, ctx, looked_up)).await)
}

/// Ставит выбранное в очередь. Копии из «Мои приложения» ставятся без скачивания.
/// `download_only` («Только скачать»): скачать в «Мои приложения» и не ставить, даже если телефон
/// подключён; готовые копии сразу «Сохранено».
#[tauri::command]
pub async fn start(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<(i64, String)>,
    same_apple_id: bool,
    download_only: Option<bool>,
) -> Result<RunDto, ()> {
    let only = download_only.unwrap_or(false);
    let email = state.account.lock().unwrap().as_ref().map(|a| a.email.clone());
    let run = RunDto { same_apple_id, email };
    *state.run.lock().unwrap() = Some(run.clone());
    state.names.lock().unwrap().extend(items.iter().cloned());
    {
        let mut fs = state.from_saved.lock().unwrap();
        for (id, _) in &items {
            fs.remove(id);
        }
    }
    let catalog_ids: HashSet<i64> = state.catalog.read().await.iter().map(|r| r.track_id).collect();
    {
        let mut set = state.download_only.lock().unwrap();
        for (id, _) in &items {
            if only {
                set.insert(*id);
            } else {
                set.remove(id);
            }
        }
    }
    let (download, install) = plan_start(items, &catalog_ids, &state.library.list(), &state.library.list_unknown());
    for (id, path) in install {
        if only {
            state.record(&app, id, JobState::Saved { path });
        } else {
            state.install_copy(&app, id, path);
        }
    }
    state.mark_downloads(download.iter().map(|(id, _)| *id).filter(|id| *id > 0));
    if only {
        state.queue.enqueue_download_only(download);
    } else {
        state.queue.enqueue(download);
    }
    Ok(run)
}

#[tauri::command]
pub fn run_info(state: State<'_, AppState>) -> Option<RunDto> {
    state.run.lock().unwrap().clone()
}

#[tauri::command]
pub async fn list_phones(app: AppHandle) -> Result<Vec<PhoneDto>, UiError> {
    poll_phones(&app).await
}

#[tauri::command]
pub fn choose_phone(app: AppHandle, state: State<'_, AppState>, udid: String) {
    state.choose(&app, udid);
}

#[tauri::command]
pub fn retry(app: AppHandle, state: State<'_, AppState>, track_id: i64) {
    let copy = state.copy_paths.lock().unwrap().get(&track_id).cloned();
    let action = {
        let jobs = state.jobs.lock().unwrap();
        match &copy {
            Some((p, _)) => copy_retry(jobs.get(&track_id), p),
            None => retry_action(jobs.get(&track_id)),
        }
    };
    match action {
        RetryAction::Install(path) => {
            state.record(&app, track_id, JobState::Queued);
            match copy {
                Some((_, udid)) => state.queue.install_on(track_id, path, udid),
                None => state.queue.install_saved(track_id, path),
            }
        }
        RetryAction::Download => {
            let name = job_name(&state.names.lock().unwrap(), track_id);
            state.from_saved.lock().unwrap().remove(&track_id);
            state.mark_downloads([track_id]);
            if state.download_only.lock().unwrap().contains(&track_id) {
                state.queue.enqueue_download_only(vec![(track_id, name)]);
            } else {
                state.queue.enqueue(vec![(track_id, name)]);
            }
        }
        RetryAction::Nothing => {}
    }
}

/// «Открыть папку с копиями»: «Мои приложения» в Finder / Проводнике. Папка создаётся, если её нет.
/// Открывает Rust, поэтому интерфейсу не нужно разрешение opener на пути.
#[tauri::command]
pub fn open_library(app: AppHandle, state: State<'_, AppState>) -> Result<(), UiError> {
    use tauri_plugin_opener::OpenerExt;
    let root = state.library.ensure_root().map_err(|e| ui("open_library", e))?;
    let path = root.to_string_lossy().into_owned();
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| ui("open_library", AppError::Unknown { detail: format!("open folder: {e}") }))
}

/// «Открыть журнал»: папка журнала (`crate::log`) в Finder / Проводнике. Как `open_library`,
/// открывает Rust: разрешения opener у интерфейса не расширяются.
#[tauri::command]
pub fn open_log(app: AppHandle) -> Result<(), UiError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::log::dir().ok_or_else(|| ui("open_log", AppError::Unknown { detail: "log disabled".into() }))?;
    let _ = std::fs::create_dir_all(&dir);
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| ui("open_log", AppError::Unknown { detail: format!("open log folder: {e}") }))
}

/// Тег новой версии программы на GitHub или `None` (в том числе при любой ошибке сети).
#[tauri::command]
pub async fn check_update(state: State<'_, AppState>) -> Result<Option<String>, ()> {
    Ok(crate::update_check::check(state.fetch.as_ref(), env!("CARGO_PKG_VERSION")).await)
}

/// Экран «Apple ID»: кто входил и чьи копии лежат в «Мои приложения». Без Keychain: признак
/// «вход сохранён» берётся из списка в папке данных.
#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Vec<crate::accounts::AccountEntry> {
    account_list(&state.sessions.known(), &state.library.copy_counts(), state.current_email().as_deref())
}

/// Выбор аккаунта: снимок его сессии на место ipatool и проверка `auth info`. Не подошёл —
/// `restored: false` и адрес для поля входа. Смена аккаунта убирает ждущие задания.
/// `skip_save`: «Перейти без сохранения» — прежний вход не сохранять (он станет «нужен вход»).
#[tauri::command]
pub async fn use_account(app: AppHandle, state: State<'_, AppState>, email: String, skip_save: Option<bool>) -> Result<UseAccountDto, UiError> {
    state.may_switch(&email).map_err(|e| ui("use_account", e))?;
    let before = state.current_email();
    let hint = keychain_hint(&app);
    let out = crate::accounts::switch_to_with(&state.session, &state.sessions, before.as_deref(), &email, skip_save.unwrap_or(false), &|| hint.on_slow()).await;
    hint.done();
    let error = out.error.clone().map(|e| ui("use_account", e));
    // Не вошли, но вход ipatool как был (`kept`) — программа остаётся в прежнем аккаунте.
    let now = if out.restored {
        out.account.clone()
    } else if out.kept {
        state.account.lock().unwrap().clone()
    } else {
        None
    };
    let dropped = if account_changed(before.as_deref(), now.as_ref().map(|a| a.email.as_str())) {
        state.clear_account_caches();
        state.drop_waiting()
    } else {
        vec![]
    };
    if out.restored {
        let lib = state.library.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || lib.migrate()).await;
    }
    state.set_account(now.clone());
    if out.unsettled {
        reconcile_account(&app);
    }
    let mut dto = use_account_dto(out, &email);
    dto.account = now;
    dto.dropped = dropped;
    dto.error = error;
    Ok(dto)
}

/// Запись входа ещё доделывается в фоне (окно macOS): когда допишется, `auth info` (он ждёт
/// своей очереди) скажет, в каком входе ipatool, — программа берёт его и шлёт `account`.
/// До этого программа «без входа»: честнее, чем показывать вход, которого может не быть.
fn reconcile_account<R: Runtime>(app: &AppHandle<R>) {
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        let st = h.state::<AppState>();
        let acc = crate::accounts::settled_login(&st.session, &st.sessions).await;
        if acc.is_some() {
            // как после входа: копии старого вида — в папку покупателя (повтор безопасен)
            let lib = st.library.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || lib.migrate()).await;
        }
        // проверка и смена без `await` между ними
        if st.current_email().is_some() {
            return; // человек уже вошёл заново — его вход главнее
        }
        crate::log::write("reconcile account", if acc.is_some() { "ipatool signed in after the keychain step" } else { "no live login" });
        if let Some(a) = &acc {
            st.sessions.remember(&a.email);
        }
        st.set_account(acc.clone());
        let _ = h.emit("account", acc);
    });
}

/// «Забыть»: удалить сохранённую сессию и адрес из списка. Копии остаются. Нынешний вход —
/// ещё и выход из ipatool. Ошибка до выхода — `Err` (ничего не меняли); после — в `error`.
#[tauri::command]
pub async fn forget_account(state: State<'_, AppState>, email: String) -> Result<ForgetDto, UiError> {
    let mut dropped = vec![];
    let mut signed_out = false;
    if state.current_email().as_deref().is_some_and(|c| same_id(c, &email)) {
        if downloads_running(&state.jobs.lock().unwrap(), &state.downloads.lock().unwrap()) {
            return Err(ui("forget_account", AppError::DownloadsRunning));
        }
        // выйти не вышло — ничего не забываем: вход остался бы в ipatool
        state.sign_out().await.map_err(|e| ui("forget_account", e))?;
        dropped = state.drop_waiting();
        signed_out = true;
    }
    let s = state.sessions.clone();
    // не забыли (очередь к ipatool, связка ключей) — сказать, а не делать вид, что вышло;
    // выход и убранные задания при этом уже случились — их интерфейс получает тоже
    let forgot = match state.session.exclusive(move || s.forget(&email)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(AppError::Unknown { detail: format!("forget: {e}") }),
        Err(e) => Err(e),
    };
    if let Err(e) = &forgot {
        log_err("forget_account", e);
    }
    Ok(forget_dto(dropped, signed_out, forgot))
}

/// «Все копии»: каждая ipa из «Мои приложения» и папок аккаунтов. Без входа тоже.
#[tauri::command]
pub async fn all_copies(state: State<'_, AppState>) -> Result<Vec<CopyDto>, ()> {
    let lib = state.library.clone();
    let copies = tauri::async_runtime::spawn_blocking(move || lib.all_copies()).await.unwrap_or_default();
    Ok(copies.iter().map(copy_dto).collect())
}

/// «Выбрать файл .ipa…»: системный диалог из Rust (интерфейсу разрешения не нужны). Файл
/// проверяется и копируется в папку своего покупателя. Отмена — `null`.
#[tauri::command]
pub async fn pick_ipa(app: AppHandle, state: State<'_, AppState>) -> Result<Option<CopyDto>, UiError> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter("Приложение iPhone", &["ipa"]).pick_file(move |f| {
        let _ = tx.send(f);
    });
    let Some(picked) = rx.await.ok().flatten() else { return Ok(None) };
    let path = picked.into_path().map_err(|e| ui("pick_ipa", AppError::Unknown { detail: format!("picked path: {e}") }))?;
    let lib = state.library.clone();
    let res = tauri::async_runtime::spawn_blocking(move || lib.import_ipa(&path))
        .await
        .unwrap_or_else(|e| Err(AppError::Unknown { detail: format!("import: {e}") }));
    res.map(|c| Some(copy_dto(&c))).map_err(|e| ui("pick_ipa", e))
}

/// Поставить копию на телефон `udid`. Путь принимается, только если он внутри «Мои приложения»
/// (после разворота ссылок). Ставит очередь (`install_on`) на этот телефон, не меняя выбранный
/// для главного потока; прогресс — события `job` с возвращённым ключом.
#[tauri::command]
pub async fn install_copy(app: AppHandle, state: State<'_, AppState>, path: String, udid: String) -> Result<i64, UiError> {
    let lib = state.library.clone();
    let p = PathBuf::from(path);
    let (real, info) = tauri::async_runtime::spawn_blocking(move || (lib.resolve_copy(&p), lib.copy_at(&p)))
        .await
        .map_err(|e| ui("install_copy", AppError::Unknown { detail: format!("resolve: {e}") }))?;
    let real = real.map_err(|e| ui("install_copy", e))?;
    let info = info.ok_or_else(|| ui("install_copy", AppError::NotIpa))?;
    // Свой телефон у задания: выбор главного потока и его ждущие задания не трогаем.
    let device = copy_device(&state.phones.lock().unwrap().last, &udid).map_err(|e| ui("install_copy", e))?;
    let id = copy_job_id(&real);
    state.copy_paths.lock().unwrap().insert(id, (real.clone(), device.clone()));
    state.names.lock().unwrap().insert(id, info.name);
    state.from_saved.lock().unwrap().remove(&id);
    state.record(&app, id, JobState::Queued);
    state.queue.install_on(id, real, device);
    Ok(id)
}

/// Покупатель копии против Apple ID телефона: «При первом запуске iPhone попросит войти в …».
#[tauri::command]
pub async fn copy_account_match(state: State<'_, AppState>, path: String, udid: String) -> Result<AccountMatchDto, ()> {
    let lib = state.library.clone();
    let p = PathBuf::from(path);
    let purchaser = tauri::async_runtime::spawn_blocking(move || lib.copy_at(&p).and_then(|c| c.purchaser)).await.ok().flatten();
    let phone = match state.installed_for(&udid).await {
        Ok(apps) => crate::device::phone_account(&apps),
        Err(e) => {
            log_err("copy_account_match", &e);
            None
        }
    };
    Ok(account_match_dto(&copy_match(phone.as_deref(), purchaser.as_deref())))
}
