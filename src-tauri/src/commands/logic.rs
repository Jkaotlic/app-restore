//! Чистая часть команд: что отдать интерфейсу и какое действие выбрать. Без Tauri, проверяется тестами.
use crate::catalog::{AppRow, InstallState, RowSource, Rows, SavedIpa, apply_versions, default_checked};
use crate::device::{AccountMatch, InstalledApp, PhoneInfo, mask_email, pick_default};
use crate::ipatool::output::OwnedApp;
use crate::errors::AppError;
use crate::queue::JobState;
use crate::removed_catalog::RemovedApp;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Ошибка для интерфейса: `code` для ветвления, `text` показать человеку как есть.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiError {
    pub code: String,
    pub text: String,
    pub retryable: bool,
    /// Только у `unknown`: короткий код (`error_ref`) — его можно продиктовать; та же строка
    /// журнала содержит его и подробность. Сама подробность в интерфейс не идёт.
    pub ref_code: Option<String>,
}

fn code(e: &AppError) -> &'static str {
    match e {
        AppError::AuthCodeRequired => "auth_code_required",
        AppError::WrongCredentials => "wrong_credentials",
        AppError::CodeRejected => "code_rejected",
        AppError::NotPurchased => "not_purchased",
        AppError::AppleUnavailable { .. } => "apple_unavailable",
        AppError::AppleChangedLogin => "apple_changed_login",
        AppError::NotLoggedIn => "not_logged_in",
        AppError::AccountDisabled => "account_disabled",
        AppError::DeviceNotFound { drivers_missing: true } => "drivers_missing",
        AppError::DeviceNotFound { drivers_missing: false } => "device_not_found",
        AppError::DeviceNotTrusted => "device_not_trusted",
        AppError::DeviceLocked => "device_locked",
        AppError::DeviceRejected { .. } => "device_rejected",
        AppError::NoDiskSpace { .. } => "no_disk_space",
        AppError::BadLink => "bad_link",
        AppError::BadAccountFolder => "bad_account_folder",
        AppError::NotIpa => "not_ipa",
        AppError::SaveSessionFailed => "save_session_failed",
        AppError::DownloadsRunning => "downloads_running",
        AppError::Busy => "busy",
        AppError::Timeout => "timeout",
        AppError::KeychainWaiting => "keychain_waiting",
        AppError::Unknown { .. } => "unknown",
    }
}

/// Код `unknown` для человека и журнала: 6 знаков FNV-1a от подробности. Один и тот же сбой
/// даёт один код, по нему строка находится в журнале.
pub fn error_ref(e: &AppError) -> Option<String> {
    let AppError::Unknown { detail } = e else { return None };
    let h = detail.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    Some(format!("{:06X}", h & 0xFF_FFFF))
}

impl From<AppError> for UiError {
    fn from(e: AppError) -> Self {
        Self { code: code(&e).into(), text: e.user_text(), retryable: e.retryable(), ref_code: error_ref(&e) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowDto {
    pub track_id: i64,
    /// Пустой у записей каталога, пока приложение не скачано.
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    /// Подсказка «Нет в российском App Store».
    pub removed_from_ru: bool,
    /// Есть копия в «Мои приложения»: `start` поставит её без скачивания.
    pub has_saved_copy: bool,
    /// `null` — ещё не проверено (`check_owned` пришлёт событие `owned`).
    pub owned: Option<bool>,
    /// `"removed"` (каталог), `"purchase"`, `"link"` (добавлено по ссылке), `"saved"` (только копия:
    /// покупки не загрузились).
    pub source: &'static str,
    pub letter: Option<String>,
    pub color: Option<String>,
    /// «На iPhone: X»; `null`, если не стоит или телефон не выбран.
    pub installed_version: Option<String>,
    /// «Доступно: Y»; `null`, пока неизвестно.
    pub available_version: Option<String>,
    pub install_state: InstallState,
    /// Отметить сразу (`catalog::default_checked`).
    pub checked: bool,
    /// «Только для iPad»: при установке на iPhone строку не выбрать.
    pub ipad_only: bool,
    /// Копия лежит в «Мои приложения» без папки аккаунта и без покупателя: «Неизвестный Apple ID».
    pub unknown_account: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowsDto {
    pub rows: Vec<RowDto>,
    pub ru_check_failed: bool,
    /// Покупки не загрузились (например, сломан вход): в списке каталог и сохранённые копии.
    pub purchases_error: Option<UiError>,
}

/// `target_iphone`: ставим на iPhone — строки «только для iPad» не отмечаются.
pub fn row_dto(r: AppRow, target_iphone: bool) -> RowDto {
    let checked = default_checked(r.source, r.owned, r.saved_copy.is_some(), r.install_state, r.ipad_only && target_iphone);
    RowDto {
        ipad_only: r.ipad_only,
        installed_version: r.installed_version,
        available_version: r.available_version,
        install_state: r.install_state,
        checked,
        track_id: r.app.id,
        bundle_id: r.app.bundle_id,
        name: r.app.name,
        version: r.app.version,
        removed_from_ru: r.removed_from_ru,
        has_saved_copy: r.saved_copy.is_some(),
        owned: r.owned,
        unknown_account: false,
        source: match r.source {
            RowSource::Removed => "removed",
            RowSource::Purchase => "purchase",
        },
        letter: r.letter,
        color: r.color,
    }
}

pub fn rows_dto(r: Rows, target_iphone: bool) -> RowsDto {
    RowsDto { rows: r.rows.into_iter().map(|x| row_dto(x, target_iphone)).collect(), ru_check_failed: r.ru_check_failed, purchases_error: None }
}

/// Приложение на телефоне для интерфейса: {bundle_id, name, version, item_id}.
pub type InstalledDto = InstalledApp;

/// Всё, из чего строятся строки «Выбора», кроме покупок.
pub struct RowCtx<'a> {
    pub catalog: &'a [RemovedApp],
    /// Итоги `check_owned` по записям каталога.
    pub owned: &'a HashMap<i64, bool>,
    /// Последние версии у Apple (`latest_version`) по trackId.
    pub latest: &'a HashMap<i64, String>,
    /// `None` — телефон не выбран: колонки «На iPhone» нет.
    pub installed: Option<&'a [InstalledApp]>,
    pub saved: &'a [SavedIpa],
    /// Копии без известного покупателя (`Library::list_unknown`).
    pub unknown: &'a [SavedIpa],
    /// Установка пойдёт на iPhone (не на iPad и не «без телефона»).
    pub target_iphone: bool,
}

/// Список, который показывается сразу: каталог плюс строки сохранённых копий (с trackId),
/// чтобы их можно было поставить и без покупок. Копии без trackId (старое имя) по id не поставить.
pub fn catalog_rows(ctx: &RowCtx) -> RowsDto {
    let catalog_ids: HashSet<i64> = ctx.catalog.iter().map(|r| r.track_id).collect();
    let mut seen = HashSet::new();
    let copies: Vec<OwnedApp> = ctx
        .saved
        .iter()
        .filter_map(|s| s.track_id)
        .filter(|id| !catalog_ids.contains(id) && seen.insert(*id))
        .filter_map(|id| {
            // самая свежая копия этого trackId, как в `build_rows`
            let path = crate::catalog::find_saved(ctx.saved, id, "")?;
            let s = ctx.saved.iter().find(|s| s.path == path)?;
            Some(OwnedApp { id, bundle_id: s.bundle_id.clone(), name: s.name.clone().unwrap_or_else(|| s.bundle_id.clone()), version: s.version.clone(), platforms: vec![] })
        })
        .collect();
    let mut rows = crate::catalog::build_rows(ctx.catalog, ctx.owned, copies, Some(&HashSet::new()), ctx.saved);
    apply_versions(&mut rows.rows, ctx.installed, ctx.latest, ctx.saved);
    let rows = rows
        .rows
        .into_iter()
        .map(|r| {
            let copy = r.source == RowSource::Purchase;
            let mut d = row_dto(r, ctx.target_iphone);
            if copy {
                (d.source, d.removed_from_ru) = ("saved", false);
            }
            d
        })
        .collect();
    RowsDto { rows: with_unknown(rows, ctx), ru_check_failed: false, purchases_error: None }
}

/// Дописывает строки копий неизвестного Apple ID: только с trackId, которого нет ни в строках,
/// ни среди своих копий. Источник `saved`, не отмечаются сами.
fn with_unknown(mut rows: Vec<RowDto>, ctx: &RowCtx) -> Vec<RowDto> {
    let mut taken: HashSet<i64> = rows.iter().map(|r| r.track_id).chain(ctx.saved.iter().filter_map(|s| s.track_id)).collect();
    let copies: Vec<OwnedApp> = ctx
        .unknown
        .iter()
        .filter_map(|s| s.track_id)
        .filter(|id| taken.insert(*id))
        .filter_map(|id| {
            let path = crate::catalog::find_saved(ctx.unknown, id, "")?;
            let s = ctx.unknown.iter().find(|s| s.path == path)?;
            Some(OwnedApp { id, bundle_id: s.bundle_id.clone(), name: s.name.clone().unwrap_or_else(|| s.bundle_id.clone()), version: s.version.clone(), platforms: vec![] })
        })
        .collect();
    if copies.is_empty() {
        return rows;
    }
    let mut built = crate::catalog::build_rows(&[], ctx.owned, copies, Some(&HashSet::new()), ctx.unknown);
    apply_versions(&mut built.rows, ctx.installed, ctx.latest, ctx.unknown);
    rows.extend(built.rows.into_iter().map(|r| {
        let mut d = row_dto(r, ctx.target_iphone);
        (d.source, d.removed_from_ru, d.checked, d.unknown_account) = ("saved", false, false, true);
        d
    }));
    rows
}

/// Список, когда `all_purchases` упал: то же, что `catalog_rows`, и ошибка для баннера.
pub fn rows_without_purchases(ctx: &RowCtx, error: AppError) -> RowsDto {
    RowsDto { purchases_error: Some(error.into()), ..catalog_rows(ctx) }
}

/// Полный список после загрузки покупок.
pub fn purchase_rows(ctx: &RowCtx, purchases: Vec<OwnedApp>, present_ru: Option<&HashSet<i64>>) -> RowsDto {
    let mut rows = crate::catalog::build_rows(ctx.catalog, ctx.owned, purchases, present_ru, ctx.saved);
    apply_versions(&mut rows.rows, ctx.installed, ctx.latest, ctx.saved);
    let d = rows_dto(rows, ctx.target_iphone);
    RowsDto { rows: with_unknown(d.rows, ctx), ..d }
}

/// Строка каталога с версиями и состоянием по `ctx`.
fn catalog_row(track_id: i64, ctx: &RowCtx) -> Option<AppRow> {
    let r = ctx.catalog.iter().find(|r| r.track_id == track_id)?;
    let mut rows = crate::catalog::build_rows(std::slice::from_ref(r), ctx.owned, vec![], Some(&HashSet::new()), ctx.saved);
    apply_versions(&mut rows.rows, ctx.installed, ctx.latest, ctx.saved);
    rows.rows.into_iter().next()
}

/// Строка «добавлено по ссылке». Запись каталога с тем же trackId важнее (у неё буква и цвет).
/// Имя из Lookup, иначе «Приложение {id}». Права уже проверены — `owned = true` (вызывающий
/// кладёт их в `ctx.owned`, версию — в `ctx.latest`).
pub fn link_row(track_id: i64, ctx: &RowCtx, looked_up: Option<(String, String)>) -> RowDto {
    if let Some(row) = catalog_row(track_id, ctx) {
        return row_dto(row, ctx.target_iphone);
    }
    let (name, bundle_id) = looked_up.unwrap_or_else(|| (format!("Приложение {track_id}"), String::new()));
    let available_version = ctx.latest.get(&track_id).cloned();
    let installed_version = ctx
        .installed
        .and_then(|list| crate::catalog::find_installed(list, track_id, &bundle_id))
        .map(|a| a.version.clone());
    RowDto {
        track_id,
        has_saved_copy: crate::catalog::find_saved(ctx.saved, track_id, &bundle_id).is_some(),
        bundle_id,
        name,
        version: String::new(),
        removed_from_ru: false,
        owned: Some(true),
        source: "link",
        letter: None,
        color: None,
        install_state: crate::catalog::install_state(installed_version.as_deref(), available_version.as_deref()),
        installed_version,
        available_version,
        // добавил сам — значит, хочет поставить
        checked: true,
        ipad_only: false,
        unknown_account: false,
    }
}

/// Ответ `latest_version` -> (права, версия). Нет прав — не ошибка.
pub fn owned_result(r: Result<Option<String>, AppError>) -> Result<(bool, Option<String>), AppError> {
    match r {
        Ok(v) => Ok((true, v)),
        Err(AppError::NotPurchased) => Ok((false, None)),
        Err(e) => Err(e),
    }
}

/// Событие `owned`: `owned = null` и `error`, если проверить не вышло. Версии и отметка
/// посчитаны по строке каталога из `ctx` (кэш уже обновлён этим итогом).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnedEvent {
    pub track_id: i64,
    pub owned: Option<bool>,
    pub error: Option<UiError>,
    pub available_version: Option<String>,
    pub install_state: InstallState,
    pub checked: bool,
}

pub fn owned_event(track_id: i64, r: &Result<(bool, Option<String>), AppError>, ctx: &RowCtx) -> OwnedEvent {
    let row = catalog_row(track_id, ctx);
    let (owned, error) = match r {
        Ok((o, _)) => (Some(*o), None),
        Err(e) => (None, Some(e.clone().into())),
    };
    let available_version = row.as_ref().and_then(|x| x.available_version.clone()).or_else(|| r.as_ref().ok().and_then(|x| x.1.clone()));
    let install_state = row.as_ref().map_or(InstallState::NotInstalled, |x| x.install_state);
    let has_copy = row.as_ref().is_some_and(|x| x.saved_copy.is_some());
    let blocked = ctx.target_iphone && row.as_ref().is_some_and(|x| x.ipad_only);
    let checked = default_checked(RowSource::Removed, owned, has_copy, install_state, blocked);
    OwnedEvent { track_id, owned, error, available_version, install_state, checked }
}

/// Кэш «что стоит на телефоне» по udid: другой телефон — пустой список, а не чужие приложения.
pub fn cached_installed(cache: Option<&(String, Vec<InstalledApp>)>, udid: &str) -> Vec<InstalledApp> {
    cache.filter(|(u, _)| u == udid).map(|(_, apps)| apps.clone()).unwrap_or_default()
}

/// Сколько страниц по 100 у `list-purchases` при `total` покупках (минимум одна).
pub fn purchase_pages(total: u32) -> u32 {
    total.div_ceil(100).max(1)
}

/// Событие `purchases_progress`: «Загружаем покупки: страница N из M».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PurchasesProgress {
    pub page: u32,
    pub pages: u32,
}

/// Ошибки, после которых проверять остальные записи бессмысленно: ответ будет тот же.
pub fn stops_owned_check(e: &AppError) -> bool {
    matches!(e, AppError::NotLoggedIn | AppError::AccountDisabled | AppError::AppleChangedLogin)
}

/// Состояние задания для интерфейса. `kind`: queued, downloading, saved, installing, installed, failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobDto {
    Queued,
    Downloading { bytes: u64 },
    Saved,
    /// 0–100 сквозным счётом (загрузка в телефон 0–50, установка 50–100).
    Installing { percent: u8 },
    Installed,
    /// `saved`: копия уже скачана — «Повторить» поставит её без скачивания.
    Failed { error: UiError, saved: bool },
}

pub fn job_dto(s: &JobState) -> JobDto {
    match s {
        JobState::Queued => JobDto::Queued,
        JobState::Downloading { bytes } => JobDto::Downloading { bytes: *bytes },
        JobState::Saved { .. } => JobDto::Saved,
        JobState::Installing { percent } => JobDto::Installing { percent: *percent },
        JobState::Installed => JobDto::Installed,
        JobState::Failed { error, saved } => JobDto::Failed { error: error.clone().into(), saved: saved.is_some() },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RetryAction {
    Install(PathBuf),
    Download,
    /// Задание идёт или уже удалось: второй раз не ставим.
    Nothing,
}

pub fn retry_action(last: Option<&JobState>) -> RetryAction {
    match last {
        Some(JobState::Failed { saved: Some(p), .. }) => RetryAction::Install(p.clone()),
        Some(JobState::Failed { saved: None, .. }) | None => RetryAction::Download,
        Some(_) => RetryAction::Nothing,
    }
}

/// Итог `plan_start`: (что скачать — trackId и имя, что поставить готовым — trackId и файл).
pub type StartPlan = (Vec<(i64, String)>, Vec<(i64, PathBuf)>);

/// Что делать по «Вернуть». Записи каталога с копией в «Мои приложения» ставятся без скачивания:
/// Apple всё равно держит только последнюю версию до удаления. Покупки и ссылки скачиваются
/// всегда (свежая версия); копия для них — запасной путь, см. `fallback_copy`.
/// Копия неизвестного Apple ID (`unknown`) ставится как есть, если строка была только ею: не
/// каталог и своей копии нет — скачать её текущим входом всё равно нельзя.
pub fn plan_start(
    items: Vec<(i64, String)>,
    catalog_ids: &HashSet<i64>,
    saved: &[SavedIpa],
    unknown: &[SavedIpa],
) -> StartPlan {
    let mut download = vec![];
    let mut install = vec![];
    for (id, name) in items {
        let own = crate::catalog::find_saved(saved, id, "");
        let in_catalog = catalog_ids.contains(&id);
        match own {
            Some(path) if in_catalog => install.push((id, path)),
            None if !in_catalog => match crate::catalog::find_saved(unknown, id, "") {
                Some(path) => install.push((id, path)),
                None => download.push((id, name)),
            },
            _ => download.push((id, name)),
        }
    }
    (download, install)
}

/// Куда ставить. Выбранный человеком телефон главнее; пока его нет в списке — никуда.
/// Без выбора — единственный iPhone (`pick_default`).
pub fn resolve_target(phones: &[PhoneInfo], chosen: Option<&str>) -> Option<String> {
    match chosen {
        Some(u) => phones.iter().any(|p| p.udid == u).then(|| u.to_string()),
        None => pick_default(phones).map(|p| p.udid.clone()),
    }
}

/// Телефон для установки из «Все копии»: тот, что выбран в диалоге, если он подключён.
/// Выбор главного потока (`resolve_target`) не участвует и не меняется.
pub fn copy_device(phones: &[PhoneInfo], udid: &str) -> Result<String, AppError> {
    phones
        .iter()
        .any(|p| p.udid == udid)
        .then(|| udid.to_string())
        .ok_or(AppError::DeviceNotFound { drivers_missing: false })
}

/// Ошибки скачивания, при которых вместо свежей версии ставится сохранённая копия.
fn copy_helps(e: &AppError) -> bool {
    matches!(e, AppError::AppleChangedLogin | AppError::AppleUnavailable { .. } | AppError::NotLoggedIn | AppError::Timeout)
}

/// Запасной путь для покупки: скачать не вышло из-за входа или серверов Apple, а копия есть.
/// Возвращает путь и версию копии (версию показывает интерфейс).
pub fn fallback_copy(error: &AppError, track_id: i64, saved: &[SavedIpa]) -> Option<(PathBuf, String)> {
    if !copy_helps(error) {
        return None;
    }
    let path = crate::catalog::find_saved(saved, track_id, "")?;
    let version = saved.iter().find(|s| s.path == path)?.version.clone();
    Some((path, version))
}

/// Событие `job`. `from_saved`: версия сохранённой копии, если ставится она вместо скачанной
/// («Поставлена сохранённая копия {version}»).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobEvent {
    pub track_id: i64,
    pub state: JobDto,
    pub from_saved: Option<String>,
}

/// Задания, которые скачаны и ждут телефона. Под тем же замком помечаются `Queued`, чтобы
/// второй вызов (следующая смена телефона) не поставил их ещё раз.
pub fn take_waiting(jobs: &mut HashMap<i64, JobState>) -> Vec<(i64, PathBuf)> {
    let mut out = vec![];
    for (id, s) in jobs.iter_mut() {
        if let JobState::Saved { path } = s {
            out.push((*id, std::mem::take(path)));
            *s = JobState::Queued;
        }
    }
    out
}

/// Смена Apple ID: убрать скачанные и ждущие телефона (`Saved`) и те `Queued`, что очередь
/// подтвердила отброшенными (`queue_dropped`); `Queued`, уже взятые очередью в работу, остаются.
/// Идущие и завершённые тоже. Возвращает убранные trackId по возрастанию.
pub fn drop_pending(jobs: &mut HashMap<i64, JobState>, queue_dropped: &[i64]) -> Vec<i64> {
    let mut gone: Vec<i64> = jobs
        .iter()
        // копии (отрицательный ключ) от аккаунта не зависят
        .filter(|(id, _)| **id > 0)
        .filter(|(id, s)| match s {
            JobState::Saved { .. } => true,
            JobState::Queued => queue_dropped.contains(id),
            _ => false,
        })
        .map(|(id, _)| *id)
        .collect();
    gone.sort();
    for id in &gone {
        jobs.remove(id);
    }
    gone
}

/// Итог `phone_account` для интерфейса. Адрес телефона только маской (`vf***@example.com`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountMatchDto {
    Same,
    Different { phone_masked: String },
    Unknown,
}

pub fn account_match_dto(m: &AccountMatch) -> AccountMatchDto {
    match m {
        AccountMatch::Same => AccountMatchDto::Same,
        AccountMatch::Different { phone } => AccountMatchDto::Different { phone_masked: mask_email(phone) },
        AccountMatch::Unknown => AccountMatchDto::Unknown,
    }
}

/// Полный адрес для поля входа после «Войти в Apple ID этого iPhone»: только когда он другой.
pub fn login_prefill(m: Option<&AccountMatch>) -> Option<String> {
    match m {
        Some(AccountMatch::Different { phone }) => Some(phone.clone()),
        _ => None,
    }
}

/// Опрашивать ли телефоны: во время установки lockdown не трогаем, берём прошлый список.
pub fn should_probe(jobs: &HashMap<i64, JobState>) -> bool {
    !jobs.values().any(|s| matches!(s, JobState::Installing { .. }))
}

/// Сколько сбоев подряд терпеть, прежде чем убрать телефон из списка.
pub const POLL_TOLERANCE: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct PollOutcome {
    pub phones: Vec<PhoneInfo>,
    pub failures: u32,
    /// Что показать; `None`, пока сбой считается случайным.
    pub error: Option<AppError>,
}

/// Итог опроса с памятью. Явный ответ usbmuxd (в том числе пустой) верен сразу. «Доверять» —
/// сразу: это действие человека. Прочий сбой: прошлый список держится до `POLL_TOLERANCE` подряд.
pub fn after_poll(prev: &[PhoneInfo], failures: u32, res: Result<Vec<PhoneInfo>, AppError>) -> PollOutcome {
    match res {
        Ok(phones) => PollOutcome { phones, failures: 0, error: None },
        Err(AppError::DeviceNotTrusted) => PollOutcome { phones: vec![], failures: 0, error: Some(AppError::DeviceNotTrusted) },
        Err(e) => {
            let failures = failures.saturating_add(1);
            if failures >= POLL_TOLERANCE {
                PollOutcome { phones: vec![], failures, error: Some(e) }
            } else {
                PollOutcome { phones: prev.to_vec(), failures, error: None }
            }
        }
    }
}

/// Имя для повторной постановки; если его нет — «Приложение {id}».
pub fn job_name(names: &HashMap<i64, String>, track_id: i64) -> String {
    names.get(&track_id).filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("Приложение {track_id}"))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhoneDto {
    pub udid: String,
    pub name: String,
    pub ios_version: String,
    pub device_class: String,
    pub product_type: String,
    /// На этот телефон пойдёт установка.
    pub target: bool,
}

/// Событие `phones` (раз в 2 с): список и ошибка опроса, если была.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhonesEvent {
    pub phones: Vec<PhoneDto>,
    pub error: Option<UiError>,
}

pub fn phone_dtos(phones: &[PhoneInfo], target: Option<&str>) -> Vec<PhoneDto> {
    phones
        .iter()
        .map(|p| PhoneDto {
            udid: p.udid.clone(),
            name: p.name.clone(),
            ios_version: p.ios_version.clone(),
            device_class: p.device_class.clone(),
            product_type: p.product_type.clone(),
            target: target == Some(p.udid.as_str()),
        })
        .collect()
}

/// Код 2FA: пустая строка из поля ввода значит «кода нет».
pub fn normalize_code(code: Option<String>) -> Option<String> {
    code.map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
}

/// Копия для экрана «Все копии». Покупатель только маской; `path` — ключ для `install_copy`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CopyDto {
    pub path: String,
    pub track_id: Option<i64>,
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub ipad_only: bool,
    pub purchaser_masked: Option<String>,
}

pub fn copy_dto(c: &crate::library::CopyInfo) -> CopyDto {
    CopyDto {
        path: c.path.to_string_lossy().into_owned(),
        track_id: c.track_id,
        bundle_id: c.bundle_id.clone(),
        name: c.name.clone(),
        version: c.version.clone(),
        ipad_only: crate::catalog::is_ipad_only(&c.device_family),
        purchaser_masked: c.purchaser.as_deref().map(mask_email),
    }
}

/// Ключ задания для установки копии: отрицательное число от пути, по модулю не больше 2^53 − 1.
/// Свои ключи у копий: с trackId (они положительные) не пересекаются, одинаковы для одного файла.
pub fn copy_job_id(path: &std::path::Path) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    // 53 бита: число JS (f64) передаёт его без потерь
    -((h.finish() >> 11) as i64).max(1)
}

/// «Повторить» у копии — всегда установка этого файла (скачивать нечего); идущую не повторяем.
pub fn copy_retry(last: Option<&JobState>, path: &std::path::Path) -> RetryAction {
    match last {
        Some(JobState::Queued | JobState::Downloading { .. } | JobState::Installing { .. }) => RetryAction::Nothing,
        _ => RetryAction::Install(path.to_path_buf()),
    }
}

/// Сменился ли вход: был один Apple ID, стал другой (или пропал). Вход «с нуля» — не смена:
/// без входа ждут только установки копий, их не трогаем.
pub fn account_changed(before: Option<&str>, after: Option<&str>) -> bool {
    match (before, after) {
        (Some(b), Some(a)) => !crate::accounts::same_id(b, a),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Идёт ли скачивание по аккаунту: задание из `downloads` в `Queued` или `Downloading`.
/// Установки (копии, сохранённые копии каталога) ipatool не трогают и смене входа не мешают.
pub fn downloads_running(jobs: &HashMap<i64, JobState>, downloads: &HashSet<i64>) -> bool {
    downloads.iter().any(|id| matches!(jobs.get(id), Some(JobState::Queued | JobState::Downloading { .. })))
}

/// Скачивание кончилось (скачано, ошибка, дальше установка) — ключ больше не скачивание.
pub fn track_download(downloads: &mut HashSet<i64>, id: i64, state: &JobState) {
    if !matches!(state, JobState::Queued | JobState::Downloading { .. }) {
        downloads.remove(&id);
    }
}

/// Можно ли менять вход ipatool на `target`: тот же аккаунт — да; другой — только когда не идёт
/// скачивание по аккаунту (`downloads_running`).
pub fn switch_guard(current: Option<&str>, target: &str, jobs: &HashMap<i64, JobState>, downloads: &HashSet<i64>) -> Result<(), AppError> {
    let same = current.is_some_and(|c| crate::accounts::same_id(c, target));
    if !same && downloads_running(jobs, downloads) {
        return Err(AppError::DownloadsRunning);
    }
    Ok(())
}

/// Покупатель копии против Apple ID телефона. Кого-то из них не знаем — `Unknown`.
pub fn copy_match(phone: Option<&str>, purchaser: Option<&str>) -> AccountMatch {
    match purchaser {
        Some(p) => crate::device::account_match(phone, p),
        None => AccountMatch::Unknown,
    }
}

/// Итог `use_account`: вошли снимком (`account`) или нужен вход с адресом `prefill`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UseAccountDto {
    pub restored: bool,
    pub account: Option<crate::ipatool::session::Account>,
    pub prefill: Option<String>,
    /// trackId заданий, убранных при смене аккаунта (как у `switch_account`).
    pub dropped: Vec<i64>,
    /// Ничего не меняли: «Не получилось сохранить вход текущего Apple ID».
    pub error: Option<UiError>,
    /// Ошибка на шаге сохранения нынешнего входа: можно «Перейти без сохранения».
    pub can_skip_save: bool,
}

pub fn use_account_dto(out: crate::accounts::UseOutcome, email: &str) -> UseAccountDto {
    let prefill = (!out.restored && out.error.is_none()).then(|| email.trim().to_string());
    UseAccountDto { restored: out.restored, account: out.account, prefill, dropped: vec![], error: None, can_skip_save: out.save_failed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_error_text_and_code() {
        let e = UiError::from(AppError::WrongCredentials);
        assert_eq!(e.text, "Apple не принял пароль или код. Проверьте и попробуйте снова.");
        assert_eq!((e.code.as_str(), e.retryable), ("wrong_credentials", false));
        let e = UiError::from(AppError::AuthCodeRequired);
        assert_eq!(e.code, "auth_code_required");
        let e = UiError::from(AppError::AppleUnavailable { status: Some(503) });
        assert_eq!((e.code.as_str(), e.retryable), ("apple_unavailable", true));
        assert_eq!(UiError::from(AppError::DeviceNotFound { drivers_missing: true }).code, "drivers_missing");
        assert_eq!(UiError::from(AppError::DeviceNotFound { drivers_missing: false }).code, "device_not_found");
        assert_eq!(UiError::from(AppError::BadLink).code, "bad_link");
        assert_eq!(UiError::from(AppError::BadAccountFolder).code, "bad_account_folder");
        assert_eq!(UiError::from(AppError::BadLink).ref_code, None);
    }

    /// Подробность в интерфейс не идёт — только короткий код, тот же, что в строке журнала.
    #[test]
    fn unknown_error_has_short_ref_code_matching_the_log() {
        let a = UiError::from(AppError::Unknown { detail: "partial: denied".into() });
        let code = a.ref_code.clone().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()), "{code}");
        assert_eq!(UiError::from(AppError::Unknown { detail: "partial: denied".into() }).ref_code, Some(code.clone()));
        assert_ne!(UiError::from(AppError::Unknown { detail: "other".into() }).ref_code, Some(code.clone()));
        assert_eq!(error_ref(&AppError::Unknown { detail: "partial: denied".into() }), Some(code));
    }

    #[test]
    fn unknown_detail_never_reaches_ui() {
        let e = UiError::from(AppError::Unknown { detail: "secret-ish {\"x\":1}".into() });
        assert_eq!((e.code.as_str(), e.text.as_str()), ("unknown", "Что-то пошло не так. Попробуйте ещё раз."));
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("secret-ish"), "{json}");
    }

    #[test]
    fn every_error_has_distinct_code() {
        let all = [
            AppError::AuthCodeRequired,
            AppError::WrongCredentials,
            AppError::CodeRejected,
            AppError::NotPurchased,
            AppError::AppleUnavailable { status: None },
            AppError::AppleChangedLogin,
            AppError::NotLoggedIn,
            AppError::AccountDisabled,
            AppError::DeviceNotFound { drivers_missing: false },
            AppError::DeviceNotFound { drivers_missing: true },
            AppError::DeviceNotTrusted,
            AppError::DeviceLocked,
            AppError::DeviceRejected { reason: "r".into() },
            AppError::NoDiskSpace { need_mb: 1 },
            AppError::BadLink,
            AppError::Busy,
            AppError::Timeout,
            AppError::KeychainWaiting,
            AppError::Unknown { detail: String::new() },
        ];
        let mut codes: Vec<String> = all.into_iter().map(|e| UiError::from(e).code).collect();
        let n = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), n);
    }

    fn removed(id: i64, name: &str) -> RemovedApp {
        RemovedApp {
            track_id: id,
            name: name.into(),
            aliases: vec![],
            letter: "С".into(),
            color: "#21A038".into(),
            source: "s".into(),
            status: None,
            seller: None,
            ipad_only: false,
        }
    }
    fn app(id: i64, bundle: &str, name: &str) -> OwnedApp {
        OwnedApp { id, bundle_id: bundle.into(), name: name.into(), version: "1.0".into(), platforms: vec![] }
    }
    fn saved(id: i64, path: &str) -> SavedIpa {
        SavedIpa { track_id: Some(id), bundle_id: "b".into(), name: None, version: "1".into(), device_family: vec![], path: path.into() }
    }

    #[test]
    fn rows_dto_from_build_rows() {
        let rows = crate::catalog::build_rows(
            &[removed(900, "Сбер")],
            &HashMap::new(),
            vec![app(1, "com.example.a", "Alpha")],
            None,
            &[saved(900, "/x/900_b_1.ipa")],
        );
        let d = rows_dto(rows, true);
        assert!(d.ru_check_failed);
        assert_eq!(
            d.rows[0],
            RowDto {
                track_id: 900,
                bundle_id: String::new(),
                name: "Сбер".into(),
                version: String::new(),
                removed_from_ru: true,
                has_saved_copy: true,
                owned: None,
                source: "removed",
                letter: Some("С".into()),
                color: Some("#21A038".into()),
                installed_version: None,
                available_version: Some("1".into()),
                install_state: InstallState::NotInstalled,
                // права ещё не проверены, но копия на компьютере есть — её и поставим
                checked: true,
                ipad_only: false,
                unknown_account: false,
            }
        );
        assert_eq!(
            (d.rows[1].source, d.rows[1].owned, d.rows[1].has_saved_copy, d.rows[1].bundle_id.as_str()),
            ("purchase", Some(true), false, "com.example.a")
        );
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["rows"][0]["owned"], serde_json::Value::Null);
        assert_eq!(json["rows"][1]["track_id"], 1);
    }

    struct Ctx {
        catalog: Vec<RemovedApp>,
        owned: HashMap<i64, bool>,
        latest: HashMap<i64, String>,
        installed: Option<Vec<InstalledApp>>,
        saved: Vec<SavedIpa>,
        unknown: Vec<SavedIpa>,
        target_iphone: bool,
    }
    impl Ctx {
        fn new(catalog: Vec<RemovedApp>) -> Self {
            Self { catalog, owned: HashMap::new(), latest: HashMap::new(), installed: None, saved: vec![], unknown: vec![], target_iphone: false }
        }
        fn get(&self) -> RowCtx<'_> {
            RowCtx {
                catalog: &self.catalog,
                owned: &self.owned,
                latest: &self.latest,
                installed: self.installed.as_deref(),
                saved: &self.saved,
                unknown: &self.unknown,
                target_iphone: self.target_iphone,
            }
        }
    }
    fn inst(bundle: &str, version: &str, item: Option<i64>) -> InstalledApp {
        InstalledApp { bundle_id: bundle.into(), name: bundle.into(), version: version.into(), item_id: item, purchaser: None }
    }

    #[test]
    fn link_row_prefers_catalog_then_lookup_then_placeholder() {
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.owned = [(900, true), (5, true), (77, true)].into();
        c.latest = [(900, "12.15.0".to_string()), (5, "12.1".to_string())].into();
        c.saved = vec![saved(5, "/x/5.ipa")];
        c.installed = Some(vec![inst("ru.sberbank.onlineiphone", "12.14.0", Some(900)), inst("ph.telegra.Telegraph", "12.1", None)]);
        let r = link_row(900, &c.get(), Some(("Другое".into(), "x".into())));
        assert_eq!((r.name.as_str(), r.source, r.owned, r.letter.as_deref()), ("Сбер", "removed", Some(true), Some("С")));
        assert_eq!((r.install_state, r.checked), (InstallState::UpdateAvailable, true));
        let r = link_row(5, &c.get(), Some(("Telegram".into(), "ph.telegra.Telegraph".into())));
        assert_eq!((r.name.as_str(), r.bundle_id.as_str(), r.source), ("Telegram", "ph.telegra.Telegraph", "link"));
        assert!(r.owned == Some(true) && r.has_saved_copy && !r.removed_from_ru);
        // ссылку человек добавил сам: строка отмечена, даже если уже стоит
        assert_eq!(
            (r.installed_version.as_deref(), r.available_version.as_deref(), r.install_state, r.checked),
            (Some("12.1"), Some("12.1"), InstallState::Installed, true)
        );
        let r = link_row(77, &c.get(), None);
        assert_eq!(r.name, "Приложение 77");
        assert_eq!((r.letter, r.color, r.available_version), (None, None, None));
    }

    #[test]
    fn owned_result_mapping() {
        assert_eq!(owned_result(Ok(Some("1.0".into()))), Ok((true, Some("1.0".into()))));
        assert_eq!(owned_result(Ok(None)), Ok((true, None)));
        assert_eq!(owned_result(Err(AppError::NotPurchased)), Ok((false, None)));
        assert_eq!(owned_result(Err(AppError::NotLoggedIn)), Err(AppError::NotLoggedIn));
    }

    #[test]
    fn owned_event_mapping() {
        let mut c = Ctx::new(vec![removed(1, "A"), removed(2, "B"), removed(3, "C"), removed(4, "D")]);
        c.owned = [(1, true), (2, false), (4, true)].into();
        c.latest = [(1, "12.15.0".to_string()), (4, "2.0".to_string())].into();
        c.installed = Some(vec![inst("x", "12.14.0", Some(1)), inst("y", "2.0", Some(4))]);
        let e = owned_event(1, &Ok((true, Some("12.15.0".into()))), &c.get());
        assert_eq!(
            e,
            OwnedEvent {
                track_id: 1,
                owned: Some(true),
                error: None,
                available_version: Some("12.15.0".into()),
                install_state: InstallState::UpdateAvailable,
                checked: true
            }
        );
        let e = owned_event(4, &Ok((true, Some("2.0".into()))), &c.get());
        assert_eq!((e.install_state, e.checked), (InstallState::Installed, false), "уже стоит — не отмечаем");
        let e = owned_event(2, &Ok((false, None)), &c.get());
        assert_eq!((e.owned, e.checked, e.error), (Some(false), false, None));
        let e = owned_event(3, &Err(AppError::AppleUnavailable { status: Some(503) }), &c.get());
        assert_eq!((e.owned, e.checked), (None, false));
        assert_eq!(e.error.unwrap().code, "apple_unavailable");
        // без телефона
        c.installed = None;
        let json = serde_json::to_value(owned_event(4, &Ok((true, Some("2.0".into()))), &c.get())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"track_id": 4, "owned": true, "error": null, "available_version": "2.0", "install_state": "not_installed", "checked": true})
        );
    }

    #[test]
    fn ipad_only_not_checked_for_iphone() {
        let mut c = Ctx::new(vec![RemovedApp { ipad_only: true, ..removed(656485245, "ВКонтакте для iPad") }, removed(2, "Обычное")]);
        c.owned = [(656485245, true), (2, true)].into();
        c.target_iphone = true;
        let d = catalog_rows(&c.get());
        assert_eq!(d.rows.iter().map(|r| (r.ipad_only, r.checked)).collect::<Vec<_>>(), [(true, false), (false, true)]);
        let e = owned_event(656485245, &Ok((true, None)), &c.get());
        assert!(!e.checked);
        // на iPad или без телефона — как обычно
        c.target_iphone = false;
        assert!(catalog_rows(&c.get()).rows[0].checked);
        assert!(owned_event(656485245, &Ok((true, None)), &c.get()).checked);
    }

    #[test]
    fn installed_cache_is_per_udid() {
        let cache = ("a".to_string(), vec![inst("com.example.x", "1", None)]);
        assert_eq!(cached_installed(Some(&cache), "a").len(), 1);
        assert!(cached_installed(Some(&cache), "b").is_empty(), "другой телефон");
        assert!(cached_installed(None, "a").is_empty());
    }

    #[test]
    fn purchase_pages_count() {
        assert_eq!(purchase_pages(0), 1);
        assert_eq!(purchase_pages(1), 1);
        assert_eq!(purchase_pages(100), 1);
        assert_eq!(purchase_pages(101), 2);
        assert_eq!(purchase_pages(741), 8);
    }

    #[test]
    fn purchase_rows_apply_versions() {
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.installed = Some(vec![inst("com.example.a", "1.0", None)]);
        let buy = vec![OwnedApp { version: "1.1".into(), ..app(1, "com.example.a", "A") }, app(2, "com.example.b", "B")];
        let d = purchase_rows(&c.get(), buy, Some(&[1, 2].into()));
        assert_eq!(d.rows.iter().map(|r| r.track_id).collect::<Vec<_>>(), [900, 1, 2]);
        assert_eq!((d.rows[1].install_state, d.rows[1].checked), (InstallState::UpdateAvailable, false), "покупки не отмечаем сами");
        assert_eq!(d.rows[2].install_state, InstallState::NotInstalled);
        assert!(!d.ru_check_failed && d.purchases_error.is_none());
        assert!(purchase_rows(&c.get(), vec![], None).ru_check_failed);
    }

    #[test]
    fn owned_check_stops_on_account_errors_only() {
        assert!(stops_owned_check(&AppError::NotLoggedIn));
        assert!(stops_owned_check(&AppError::AccountDisabled));
        assert!(stops_owned_check(&AppError::AppleChangedLogin));
        assert!(!stops_owned_check(&AppError::AppleUnavailable { status: None }));
        assert!(!stops_owned_check(&AppError::Unknown { detail: String::new() }));
    }

    #[test]
    fn job_dto_mapping_and_json() {
        assert_eq!(job_dto(&JobState::Queued), JobDto::Queued);
        assert_eq!(job_dto(&JobState::Downloading { bytes: 7 }), JobDto::Downloading { bytes: 7 });
        assert_eq!(job_dto(&JobState::Saved { path: "/x".into() }), JobDto::Saved);
        assert_eq!(job_dto(&JobState::Installing { percent: 60 }), JobDto::Installing { percent: 60 });
        assert_eq!(job_dto(&JobState::Installed), JobDto::Installed);
        let f = job_dto(&JobState::Failed { error: AppError::DeviceLocked, saved: Some("/x".into()) });
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["kind"], "failed");
        assert_eq!(json["saved"], true);
        assert_eq!(json["error"]["code"], "device_locked");
        assert_eq!(serde_json::to_value(JobDto::Installing { percent: 5 }).unwrap(), serde_json::json!({"kind":"installing","percent":5}));
    }

    #[test]
    fn retry_decision() {
        let failed = |saved: Option<&str>| JobState::Failed { error: AppError::DeviceLocked, saved: saved.map(PathBuf::from) };
        assert_eq!(retry_action(Some(&failed(Some("/x/1.ipa")))), RetryAction::Install("/x/1.ipa".into()));
        assert_eq!(retry_action(Some(&failed(None))), RetryAction::Download);
        assert_eq!(retry_action(None), RetryAction::Download);
        for busy in [
            JobState::Queued,
            JobState::Downloading { bytes: 1 },
            JobState::Saved { path: "/x".into() },
            JobState::Installing { percent: 1 },
            JobState::Installed,
        ] {
            assert_eq!(retry_action(Some(&busy)), RetryAction::Nothing, "{busy:?}");
        }
    }

    #[test]
    fn start_installs_copy_only_for_catalog_rows() {
        let lib = [saved(2, "/x/2_old.ipa"), saved(2, "/x/2_new.ipa"), saved(5, "/x/5.ipa"), SavedIpa { track_id: None, ..saved(3, "/x/b_1.ipa") }];
        let catalog: HashSet<i64> = [2, 3].into();
        let items = vec![(1, "A".into()), (2, "B".into()), (3, "C".into()), (5, "D".into())];
        let (down, inst) = plan_start(items, &catalog, &lib, &[]);
        // 5 — покупка с копией: всё равно скачивается свежая версия
        assert_eq!(down, vec![(1, "A".to_string()), (3, "C".to_string()), (5, "D".to_string())]);
        assert_eq!(inst.iter().map(|x| x.0).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn start_installs_unknown_account_copy_directly() {
        let unknown = [saved(8, "/x/8_u.ipa"), saved(2, "/x/2_u.ipa"), saved(9, "/x/9_u.ipa")];
        let lib = [saved(9, "/x/me/9.ipa")];
        let catalog: HashSet<i64> = [2].into();
        let items = vec![(8, "U".into()), (2, "C".into()), (9, "M".into())];
        let (down, inst) = plan_start(items, &catalog, &lib, &unknown);
        // 8 — только копия неизвестного Apple ID: скачать её нашим входом нельзя, ставим как есть.
        // 2 — запись каталога, 9 — есть своя копия: как раньше.
        assert_eq!(down, vec![(2, "C".to_string()), (9, "M".to_string())]);
        assert_eq!(inst, vec![(8, PathBuf::from("/x/8_u.ipa"))]);
    }

    /// Покупки не загрузились: строки копий называются по имени из ipa, не по bundle id.
    #[test]
    fn saved_rows_use_the_copy_name() {
        let mut c = Ctx::new(vec![]);
        c.saved = vec![SavedIpa { name: Some("СберБанк Онлайн".into()), ..saved(7, "/x/7_b_1.ipa") }, saved(8, "/x/8_b_1.ipa")];
        c.unknown = vec![SavedIpa { name: Some("Чужое".into()), ..saved(9, "/x/9_u_1.ipa") }];
        let d = rows_without_purchases(&c.get(), AppError::NotLoggedIn);
        let mut names: Vec<_> = d.rows.iter().map(|r| (r.track_id, r.name.as_str())).collect();
        names.sort();
        assert_eq!(names, [(7, "СберБанк Онлайн"), (8, "b"), (9, "Чужое")]);
    }

    #[test]
    fn unknown_account_copies_are_saved_rows_with_hint() {
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.saved = vec![saved(7, "/x/me/7_b_1.ipa")];
        c.unknown = vec![
            SavedIpa { bundle_id: "com.example.u".into(), ..saved(5, "/x/5_com.example.u_1.ipa") },
            saved(900, "/x/900_u.ipa"), // уже есть строка каталога
            saved(7, "/x/7_u.ipa"),     // уже есть своя копия
            SavedIpa { track_id: None, ..saved(0, "/x/old_1.ipa") },
        ];
        let d = catalog_rows(&c.get());
        assert_eq!(d.rows.iter().map(|r| (r.track_id, r.unknown_account)).collect::<Vec<_>>(), [(900, false), (7, false), (5, true)]);
        let u = &d.rows[2];
        assert_eq!((u.source, u.checked, u.has_saved_copy, u.bundle_id.as_str()), ("saved", false, true, "com.example.u"));
        // после покупок: та же строка, если такой покупки нет
        let p = purchase_rows(&c.get(), vec![app(1, "com.example.a", "Alpha")], Some(&HashSet::new()));
        assert_eq!(p.rows.iter().map(|r| (r.track_id, r.unknown_account)).collect::<Vec<_>>(), [(900, false), (1, false), (5, true)]);
        let p = purchase_rows(&c.get(), vec![app(5, "com.example.u", "U")], Some(&HashSet::new()));
        assert_eq!(p.rows.iter().filter(|r| r.track_id == 5).map(|r| r.unknown_account).collect::<Vec<_>>(), [false], "покупка главнее");
    }

    #[test]
    fn fallback_copy_only_for_login_and_server_errors() {
        let lib = [SavedIpa { version: "9.1".into(), ..saved(5, "/x/5_b_9.1.ipa") }];
        for e in [AppError::AppleChangedLogin, AppError::AppleUnavailable { status: Some(503) }, AppError::NotLoggedIn, AppError::Timeout] {
            assert_eq!(fallback_copy(&e, 5, &lib), Some((PathBuf::from("/x/5_b_9.1.ipa"), "9.1".into())), "{e:?}");
        }
        assert_eq!(fallback_copy(&AppError::NotLoggedIn, 6, &lib), None, "копии нет");
        for e in [AppError::NotPurchased, AppError::NoDiskSpace { need_mb: 1 }, AppError::Unknown { detail: String::new() }] {
            assert_eq!(fallback_copy(&e, 5, &lib), None, "{e:?}");
        }
    }

    #[test]
    fn job_event_json_has_from_saved() {
        let ev = JobEvent { track_id: 5, state: JobDto::Queued, from_saved: Some("9.1".into()) };
        assert_eq!(serde_json::to_value(ev).unwrap(), serde_json::json!({"track_id":5,"state":{"kind":"queued"},"from_saved":"9.1"}));
    }

    #[test]
    fn rows_without_purchases_keep_catalog_and_copies() {
        let lib = [
            saved(900, "/x/900_b_1.ipa"),
            SavedIpa { bundle_id: "com.example.k".into(), version: "2.0".into(), ..saved(7, "/x/7_com.example.k_2.0.ipa") },
            SavedIpa { track_id: None, ..saved(0, "/x/old_1.ipa") },
        ];
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.saved = lib.to_vec();
        c.installed = Some(vec![inst("com.example.k", "1.0", None)]);
        let full = catalog_rows(&c.get());
        assert_eq!(full.purchases_error, None);
        assert_eq!(full.rows.iter().map(|r| r.track_id).collect::<Vec<_>>(), [900, 7]);
        assert_eq!(full.rows[1].install_state, InstallState::UpdateAvailable, "у копии известен bundle id");
        let d = rows_without_purchases(&c.get(), AppError::AppleChangedLogin);
        assert_eq!(d.purchases_error.as_ref().map(|e| e.code.as_str()), Some("apple_changed_login"));
        assert!(!d.ru_check_failed, "сверку не делали — подсказку о сбое не показываем");
        assert_eq!(d.rows.iter().map(|r| r.track_id).collect::<Vec<_>>(), [900, 7]);
        assert_eq!((d.rows[0].source, d.rows[0].has_saved_copy), ("removed", true));
        let k = &d.rows[1];
        assert_eq!((k.source, k.name.as_str(), k.version.as_str(), k.has_saved_copy), ("saved", "com.example.k", "2.0", true));
        assert_eq!(rows_dto(crate::catalog::build_rows(&[], &HashMap::new(), vec![], None, &[]), false).purchases_error, None);
    }

    #[test]
    fn copy_device_needs_connected_phone_and_ignores_choice() {
        let phones = [ph("iPhone", "A"), ph("iPhone", "B")];
        assert_eq!(copy_device(&phones, "B"), Ok("B".to_string()));
        assert_eq!(copy_device(&phones, "C"), Err(AppError::DeviceNotFound { drivers_missing: false }));
        assert_eq!(copy_device(&[], "A"), Err(AppError::DeviceNotFound { drivers_missing: false }));
    }

    fn ph(class: &str, udid: &str) -> PhoneInfo {
        PhoneInfo { udid: udid.into(), name: "n".into(), ios_version: "17".into(), device_class: class.into(), product_type: "p".into() }
    }

    #[test]
    fn target_resolution() {
        let (a, b, pad) = (ph("iPhone", "a"), ph("iPhone", "b"), ph("iPad", "p"));
        assert_eq!(resolve_target(&[a.clone(), pad.clone()], None), Some("a".into()));
        assert_eq!(resolve_target(&[a.clone(), b.clone()], None), None);
        assert_eq!(resolve_target(&[a.clone(), b.clone()], Some("b")), Some("b".into()));
        assert_eq!(resolve_target(&[a.clone(), pad.clone()], Some("p")), Some("p".into()), "iPad можно выбрать руками");
        assert_eq!(resolve_target(&[a], Some("b")), None, "выбранный отключён — не ставить на другой");
        assert_eq!(resolve_target(&[], None), None);
    }

    #[test]
    fn take_waiting_marks_queued_once() {
        let mut jobs: HashMap<i64, JobState> = [
            (1, JobState::Saved { path: "/x/1.ipa".into() }),
            (2, JobState::Installed),
            (3, JobState::Failed { error: AppError::DeviceLocked, saved: Some("/x/3.ipa".into()) }),
            (4, JobState::Saved { path: "/x/4.ipa".into() }),
        ]
        .into();
        let mut w = take_waiting(&mut jobs);
        w.sort();
        assert_eq!(w, vec![(1, PathBuf::from("/x/1.ipa")), (4, PathBuf::from("/x/4.ipa"))]);
        assert_eq!((&jobs[&1], &jobs[&4]), (&JobState::Queued, &JobState::Queued));
        assert!(take_waiting(&mut jobs).is_empty(), "второй вызов ничего не ставит");
        assert!(matches!(jobs[&3], JobState::Failed { .. }));
    }

    #[test]
    fn drop_pending_keeps_copy_installs() {
        let mut jobs = HashMap::new();
        jobs.insert(-3, JobState::Saved { path: "/c.ipa".into() });
        jobs.insert(-4, JobState::Queued);
        jobs.insert(9, JobState::Saved { path: "/d.ipa".into() });
        assert_eq!(drop_pending(&mut jobs, &[-4]), vec![9], "копии от аккаунта не зависят");
        assert!(jobs.contains_key(&-3) && jobs.contains_key(&-4));
    }

    #[test]
    fn drop_pending_removes_only_queued_and_saved() {
        let mut jobs: HashMap<i64, JobState> = [
            (1, JobState::Queued),
            (2, JobState::Saved { path: "/x/2.ipa".into() }),
            (3, JobState::Installing { percent: 40 }),
            (4, JobState::Downloading { bytes: 1 }),
            (5, JobState::Installed),
            (6, JobState::Failed { error: AppError::DeviceLocked, saved: None }),
        ]
        .into();
        jobs.insert(7, JobState::Queued);
        // 7 очередь уже взяла в работу (нет в её списке отброшенных): остаётся
        assert_eq!(drop_pending(&mut jobs, &[1, 99]), vec![1, 2]);
        let mut left: Vec<i64> = jobs.keys().copied().collect();
        left.sort();
        assert_eq!(left, vec![3, 4, 5, 6, 7], "идущую установку и итоги не трогаем");
        assert!(drop_pending(&mut jobs, &[]).is_empty());
    }

    #[test]
    fn account_match_dto_masks_phone() {
        use crate::device::AccountMatch;
        assert_eq!(account_match_dto(&AccountMatch::Same), AccountMatchDto::Same);
        assert_eq!(account_match_dto(&AccountMatch::Unknown), AccountMatchDto::Unknown);
        let d = account_match_dto(&AccountMatch::Different { phone: "owner@example.com".into() });
        assert_eq!(d, AccountMatchDto::Different { phone_masked: "ow***@example.com".into() });
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, r#"{"kind":"different","phone_masked":"ow***@example.com"}"#);
        assert_eq!(serde_json::to_string(&AccountMatchDto::Same).unwrap(), r#"{"kind":"same"}"#);
        assert_eq!(serde_json::to_string(&AccountMatchDto::Unknown).unwrap(), r#"{"kind":"unknown"}"#);
    }

    #[test]
    fn installed_dto_never_carries_purchaser() {
        let mut a = inst("com.example.a", "1.0", Some(1));
        a.purchaser = Some("owner@example.com".into());
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("owner@") && !json.contains("purchaser"), "{json}");
    }

    #[test]
    fn prefill_only_for_different() {
        use crate::device::AccountMatch;
        let d = AccountMatch::Different { phone: "owner@example.com".into() };
        assert_eq!(login_prefill(Some(&d)), Some("owner@example.com".into()));
        assert_eq!(login_prefill(Some(&AccountMatch::Same)), None);
        assert_eq!(login_prefill(Some(&AccountMatch::Unknown)), None);
        assert_eq!(login_prefill(None), None);
    }

    #[test]
    fn no_probe_during_install() {
        let mut jobs: HashMap<i64, JobState> = [(1, JobState::Downloading { bytes: 1 }), (2, JobState::Installed)].into();
        assert!(should_probe(&jobs));
        jobs.insert(3, JobState::Installing { percent: 40 });
        assert!(!should_probe(&jobs));
        assert!(should_probe(&HashMap::new()));
    }

    #[test]
    fn poll_keeps_list_through_transient_errors() {
        let a = vec![ph("iPhone", "a")];
        let blip = || Err(AppError::DeviceNotFound { drivers_missing: false });
        let o = after_poll(&a, 0, blip());
        assert_eq!((o.phones.clone(), o.failures, o.error.clone()), (a.clone(), 1, None));
        let o = after_poll(&a, 1, blip());
        assert_eq!((o.phones.clone(), o.failures, o.error.clone()), (a.clone(), 2, None));
        let o = after_poll(&a, 2, blip());
        assert_eq!((o.phones, o.failures, o.error), (vec![], 3, Some(AppError::DeviceNotFound { drivers_missing: false })));
        // явный пустой ответ usbmuxd — телефон вынули
        assert_eq!(after_poll(&a, 1, Ok(vec![])), PollOutcome { phones: vec![], failures: 0, error: None });
        assert_eq!(after_poll(&[], 2, Ok(a.clone())), PollOutcome { phones: a.clone(), failures: 0, error: None });
        // «Доверять» показываем сразу
        assert_eq!(
            after_poll(&a, 0, Err(AppError::DeviceNotTrusted)),
            PollOutcome { phones: vec![], failures: 0, error: Some(AppError::DeviceNotTrusted) }
        );
    }

    #[test]
    fn job_name_falls_back_to_id() {
        let names: HashMap<i64, String> = [(1, "Сбер".to_string()), (2, String::new())].into();
        assert_eq!(job_name(&names, 1), "Сбер");
        assert_eq!(job_name(&names, 2), "Приложение 2");
        assert_eq!(job_name(&names, 3), "Приложение 3");
    }

    #[test]
    fn phone_dtos_mark_target() {
        let d = phone_dtos(&[ph("iPhone", "a"), ph("iPad", "p")], Some("p"));
        assert_eq!(d.iter().map(|p| p.target).collect::<Vec<_>>(), [false, true]);
        assert!(phone_dtos(&[ph("iPhone", "a")], None).iter().all(|p| !p.target));
    }

    #[test]
    fn code_normalized() {
        assert_eq!(normalize_code(None), None);
        assert_eq!(normalize_code(Some("  ".into())), None);
        assert_eq!(normalize_code(Some(" 123456 ".into())), Some("123456".into()));
    }
    fn copy_info(purchaser: Option<&str>, family: &[u8]) -> crate::library::CopyInfo {
        crate::library::CopyInfo {
            path: "/lib/me@example.com/1_com.example.a_1.0.ipa".into(),
            track_id: Some(1),
            bundle_id: "com.example.a".into(),
            name: "Банк".into(),
            version: "1.0".into(),
            device_family: family.to_vec(),
            purchaser: purchaser.map(String::from),
            account_folder: Some("me@example.com".into()),
        }
    }

    #[test]
    fn copy_dto_masks_purchaser() {
        let d = copy_dto(&copy_info(Some("vasya.pupkin@example.com"), &[2]));
        assert_eq!(d.purchaser_masked.as_deref(), Some("va***@example.com"));
        assert!(d.ipad_only);
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("vasya.pupkin@"), "{json}");
        assert!(json.contains("\"path\":\"/lib/me@example.com/1_com.example.a_1.0.ipa\""), "{json}");
        let d = copy_dto(&copy_info(None, &[1, 2]));
        assert_eq!((d.purchaser_masked, d.ipad_only), (None, false));
    }

    #[test]
    fn copy_job_ids_fit_in_a_js_number() {
        for i in 0..2000 {
            let id = copy_job_id(std::path::Path::new(&format!("/lib/{i}.ipa")));
            assert!(id < 0 && id.unsigned_abs() < (1u64 << 53), "{id}");
            // через JSON как число JS (f64) — без потерь
            let back: f64 = serde_json::from_str(&serde_json::to_string(&id).unwrap()).unwrap();
            assert_eq!(back as i64, id);
        }
    }

    #[test]
    fn downloads_running_sees_only_account_downloads() {
        let mut jobs = HashMap::new();
        let mut downloads = HashSet::new();
        // установка копии и сохранённой копии каталога — не скачивание
        jobs.insert(-7, JobState::Queued);
        jobs.insert(5, JobState::Installing { percent: 10 });
        assert!(!downloads_running(&jobs, &downloads));
        // скачивание в очереди и идущее
        for (id, st) in [(1, JobState::Queued), (2, JobState::Downloading { bytes: 9 })] {
            downloads.insert(id);
            jobs.insert(id, st);
        }
        assert!(downloads_running(&jobs, &downloads));
        // скачалось — дальше установка, уже не скачивание
        for (id, st) in [(1, JobState::Saved { path: "/a".into() }), (2, JobState::Failed { error: AppError::NotPurchased, saved: None })] {
            track_download(&mut downloads, id, &st);
            jobs.insert(id, st);
        }
        assert!(downloads.is_empty());
        jobs.insert(1, JobState::Queued); // установка скачанного
        assert!(!downloads_running(&jobs, &downloads));
        // промежуточные события скачивания ключ не снимают
        downloads.insert(3);
        track_download(&mut downloads, 3, &JobState::Downloading { bytes: 1 });
        assert!(downloads.contains(&3));
    }

    #[test]
    fn switch_guard_refuses_while_account_downloads_run() {
        let mut jobs = HashMap::new();
        let mut downloads = HashSet::new();
        jobs.insert(1, JobState::Downloading { bytes: 5 });
        downloads.insert(1);
        jobs.insert(-2, JobState::Installing { percent: 50 });
        let err = Err(AppError::DownloadsRunning);
        assert_eq!(switch_guard(Some("me@example.com"), "other@example.com", &jobs, &downloads), err);
        assert_eq!(switch_guard(None, "other@example.com", &jobs, &downloads), err, "вход ipatool всё равно сменится");
        assert_eq!(switch_guard(Some("me@example.com"), " ME@example.com", &jobs, &downloads), Ok(()), "тот же аккаунт");
        downloads.clear();
        assert_eq!(switch_guard(Some("me@example.com"), "other@example.com", &jobs, &downloads), Ok(()), "установки не мешают");
        assert_eq!(UiError::from(AppError::DownloadsRunning).text, "Дождитесь окончания скачивания");
        assert_eq!(UiError::from(AppError::DownloadsRunning).code, "downloads_running");
    }

    #[test]
    fn copy_job_ids_never_collide_with_track_ids() {
        let p = std::path::Path::new("/lib/me@example.com/42_com.example.a_1.0.ipa");
        let a = copy_job_id(p);
        assert!(a < 0, "trackId положительные, копии — отрицательные");
        assert_eq!(a, copy_job_id(p));
        assert_ne!(a, copy_job_id(std::path::Path::new("/lib/y.ipa")));
    }

    #[test]
    fn copy_jobs_always_retry_as_install() {
        let p = PathBuf::from("/lib/x.ipa");
        let failed_no_copy = JobState::Failed { error: AppError::DeviceLocked, saved: None };
        assert_eq!(copy_retry(Some(&failed_no_copy), &p), RetryAction::Install(p.clone()), "никогда не скачивание");
        assert_eq!(copy_retry(None, &p), RetryAction::Install(p.clone()));
        assert_eq!(copy_retry(Some(&JobState::Installed), &p), RetryAction::Install(p.clone()));
        assert_eq!(copy_retry(Some(&JobState::Installing { percent: 3 }), &p), RetryAction::Nothing);
        assert_eq!(copy_retry(Some(&JobState::Queued), &p), RetryAction::Nothing);
    }

    #[test]
    fn account_changed_only_from_one_login_to_another() {
        assert!(account_changed(Some("me@example.com"), Some("other@example.com")));
        assert!(!account_changed(Some("me@example.com"), Some(" ME@example.com")));
        assert!(!account_changed(None, Some("me@example.com")), "без входа ждут только копии — их не трогаем");
        assert!(account_changed(Some("me@example.com"), None), "вход пропал");
        assert!(!account_changed(None, None));
    }

    #[test]
    fn copy_match_compares_purchaser_with_phone() {
        assert_eq!(copy_match(Some("me@example.com"), Some("ME@example.com")), AccountMatch::Same);
        assert_eq!(
            copy_match(Some("phone@example.com"), Some("me@example.com")),
            AccountMatch::Different { phone: "phone@example.com".into() }
        );
        assert_eq!(copy_match(None, Some("me@example.com")), AccountMatch::Unknown);
        assert_eq!(copy_match(Some("me@example.com"), None), AccountMatch::Unknown);
    }

    #[test]
    fn use_account_dto_prefills_only_when_not_restored() {
        use crate::accounts::UseOutcome;
        use crate::ipatool::session::Account;
        let acc = Account { email: "me@example.com".into(), name: "Me".into() };
        let ok = use_account_dto(UseOutcome { restored: true, account: Some(acc.clone()), ..Default::default() }, "me@example.com");
        assert_eq!(ok, UseAccountDto { restored: true, account: Some(acc), prefill: None, dropped: vec![], error: None, can_skip_save: false });
        let no = use_account_dto(UseOutcome::default(), " other@example.com ");
        assert_eq!(no, UseAccountDto { restored: false, account: None, prefill: Some("other@example.com".into()), dropped: vec![], error: None, can_skip_save: false });
        // не сохранили нынешний вход — остаёмся, без подстановки
        let err = use_account_dto(UseOutcome { kept: true, error: Some(AppError::SaveSessionFailed), save_failed: true, ..Default::default() }, "other@example.com");
        assert_eq!(err.prefill, None);
        assert!(err.can_skip_save);
        let waiting = use_account_dto(UseOutcome { error: Some(AppError::KeychainWaiting), unsettled: true, ..Default::default() }, "other@example.com");
        assert!(!waiting.can_skip_save, "повисла запись снимка, не сохранение — «без сохранения» не поможет");
    }
}
