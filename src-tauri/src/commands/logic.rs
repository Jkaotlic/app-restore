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
        AppError::VersionMetaBroken { .. } => "version_meta_broken",
        AppError::WrongCredentials => "wrong_credentials",
        AppError::CodeRejected => "code_rejected",
        AppError::NotPurchased => "not_purchased",
        AppError::AppGone => "app_gone",
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
        AppError::DownloadStalled => "download_stalled",
        AppError::KeychainWaiting => "keychain_waiting",
        AppError::IosTooOld { .. } => "ios_too_old",
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
    /// покупки не загрузились), `"phone"` («С этого iPhone»), `"store"` («В App Store», поиск).
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
    /// Другие названия записи каталога — по ним тоже ищет поиск.
    pub aliases: Vec<String>,
    /// Фильтр «Банки» и т. п.: `bank`, `social`, `travel`, `other` у записей каталога, `null` у прочих.
    pub category: Option<&'static str>,
    /// Примерный размер в байтах: `fileSizeBytes` из Lookup или размер сохранённой копии; `null` —
    /// неизвестно.
    pub size_bytes: Option<u64>,
    /// Строка «С этого iPhone», купленная на другом Apple ID: не выбрать, «Куплено на другом Apple ID».
    /// Самого адреса в строке нет.
    pub other_account: bool,
    /// Маска покупателя (`ma***@example.com`) у строки `other_account`; полного адреса нигде нет.
    pub purchaser_masked: Option<String>,
    /// Минимальная iOS того, что поставим (последняя версия, `minimumOsVersion` из Lookup);
    /// `null` — неизвестно (в том числе у записей каталога, которые ставятся из копии).
    pub min_ios: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowsDto {
    pub rows: Vec<RowDto>,
    pub ru_check_failed: bool,
    /// Покупки не загрузились (например, сломан вход): в списке каталог и сохранённые копии.
    pub purchases_error: Option<UiError>,
    /// udid телефона-источника группы «С этого iPhone»; `null` — источника нет (или ещё не выбран).
    pub source_udid: Option<String>,
}

/// `target_iphone`: ставим на iPhone — строки «только для iPad» не отмечаются.
pub fn row_dto(r: AppRow, target_iphone: bool) -> RowDto {
    let checked = default_checked(r.source, r.owned, r.saved_copy.is_some(), r.install_state, r.ipad_only && target_iphone);
    RowDto {
        size_bytes: r.saved_copy.as_deref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()),
        aliases: r.aliases,
        category: r.category,
        other_account: false,
        purchaser_masked: None,
        min_ios: None,
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
    RowsDto { rows: r.rows.into_iter().map(|x| row_dto(x, target_iphone)).collect(), ru_check_failed: r.ru_check_failed, purchases_error: None, source_udid: None }
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
    /// Размеры из Lookup (`fileSizeBytes`) по trackId.
    pub sizes: &'a HashMap<i64, u64>,
    /// Apple ID программы: с ним сравнивается покупатель строк «С этого iPhone».
    pub account: Option<&'a str>,
    /// Что стоит на телефоне-источнике группы «С этого iPhone» (любой подключённый, не только цель);
    /// `None` — группы нет. Версия «на iPhone» и состояние строк группы — по цели (`installed`).
    pub source: Option<&'a [InstalledApp]>,
    /// udid этого источника: им помечаются строки группы (`RowsDto.source_udid`, `SourceRowsDto`).
    pub source_udid: Option<&'a str>,
    /// Минимальная iOS последней версии из Lookup (`minimumOsVersion`) по trackId.
    pub min_os: &'a HashMap<i64, String>,
}

/// Размер для строки: у записей каталога главнее копия (её и поставим), у прочих — Lookup.
/// Минимальная iOS из Lookup — о последней версии: строке каталога с копией (поставим копию) не идёт.
fn with_sizes(mut rows: Vec<RowDto>, ctx: &RowCtx) -> Vec<RowDto> {
    for r in &mut rows {
        let store = ctx.sizes.get(&r.track_id).copied();
        r.size_bytes = if r.source == "removed" { r.size_bytes.or(store) } else { store.or(r.size_bytes) };
        if !(r.source == "removed" && r.has_saved_copy) {
            r.min_ios = r.min_ios.take().or_else(|| ctx.min_os.get(&r.track_id).cloned());
        }
    }
    rows
}

/// Сколько версий спрашивать у Apple за один раз («Другая версия…», «Ещё»).
pub const VERSIONS_PAGE: usize = 10;

/// Версии для списка: от новой к старой, после `before` (его external id; `None` — с самой новой),
/// не больше `n`. `ids` — от старой к новой, как отдаёт `list-versions`. Второе — есть ли ещё старше.
/// Неизвестный `before` — пусто.
pub fn versions_page(ids: &[String], before: Option<&str>, n: usize) -> (Vec<String>, bool) {
    let end = match before {
        None => ids.len(),
        Some(b) => match ids.iter().position(|x| x == b) {
            Some(i) => i,
            None => return (vec![], false),
        },
    };
    let start = end.saturating_sub(n);
    (ids[start..end].iter().rev().cloned().collect(), start > 0)
}

/// Ошибка списка версий для человека: 429 — «Apple просит подождать», а не «серверы не отвечают».
/// `wait` — сколько секунд ещё пауза (`MetaGate`).
pub fn versions_error(e: AppError, wait: Option<u64>) -> UiError {
    let mut u = UiError::from(e.clone());
    if e == (AppError::AppleUnavailable { status: Some(429) }) {
        u.text = match wait {
            Some(s) => format!("Apple просит подождать: слишком много запросов о версиях. Можно снова через {s} с."),
            None => "Apple просит подождать: слишком много запросов о версиях. Попробуйте через несколько минут.".into(),
        };
    }
    u
}

/// Пауза метаданных версий после 429: столько к Apple о версиях не ходим.
pub const META_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// Общая для списка и поиска пауза после 429 («Apple просит подождать»).
#[derive(Debug, Default)]
pub struct MetaGate {
    until: Option<std::time::Instant>,
}

impl MetaGate {
    /// Сколько секунд ещё ждать (вверх); `None` — можно спрашивать.
    pub fn wait_left(&self, now: std::time::Instant) -> Option<u64> {
        let left = self.until?.checked_duration_since(now)?;
        (!left.is_zero()).then(|| left.as_secs() + u64::from(left.subsec_nanos() > 0))
    }

    /// Ответ Apple: 429 — пауза `META_COOLDOWN` с этого момента.
    pub fn note(&mut self, e: &AppError, now: std::time::Instant) {
        if *e == (AppError::AppleUnavailable { status: Some(429) }) {
            self.until = Some(now + META_COOLDOWN);
        }
    }
}

/// iOS телефона-цели для «Найти подходящую»: только iPhone.
pub fn target_ios(phones: &[PhoneInfo], target: Option<&str>) -> Result<String, AppError> {
    let p = target.and_then(|t| phones.iter().find(|d| d.udid == t)).ok_or(AppError::DeviceNotFound { drivers_missing: false })?;
    if p.device_class != "iPhone" {
        return Err(AppError::DeviceRejected { reason: "Подходящую версию ищем только для iPhone".into() });
    }
    Ok(p.ios_version.clone())
}

/// Версия в списке «Другая версия…». `display` — `null`, если Apple не успел ответить.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionDto {
    pub id: String,
    pub display: Option<String>,
    /// Дата выхода «ГГГГ-ММ-ДД».
    pub released: Option<String>,
    /// Apple о ней спросили; `false` — не успели (пауза после 429), можно спросить позже.
    pub checked: bool,
}

/// Метаданные версии: нечитаемые (`VersionMetaBroken`) — версия без сведений (и с датой, если
/// она нашлась в ошибке), а не ошибка списка. Прочие ошибки — как есть.
pub fn meta_or_blank(r: Result<crate::ios_compat::VersionMeta, AppError>) -> Result<crate::ios_compat::VersionMeta, AppError> {
    match r {
        Err(AppError::VersionMetaBroken { released }) => Ok(crate::ios_compat::VersionMeta { display: String::new(), released }),
        other => other,
    }
}

pub fn version_dto(id: &str, meta: Option<&crate::ios_compat::VersionMeta>) -> VersionDto {
    VersionDto {
        id: id.to_string(),
        display: meta.map(|m| m.display.clone()).filter(|d| !d.trim().is_empty()),
        released: meta.and_then(|m| m.released.as_deref()).map(|d| d.get(..10).unwrap_or(d).to_string()),
        checked: meta.is_some(),
    }
}

/// Итог `app_versions`: страница версий от новой к старой. `more` — есть старше («Ещё»).
/// `error` — Apple перестал отвечать посреди страницы: показано, что успели.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionsDto {
    pub versions: Vec<VersionDto>,
    pub more: bool,
    pub error: Option<UiError>,
    /// Пауза после 429: через сколько секунд можно снова.
    pub wait_s: Option<u64>,
}

/// Итог `find_version`: самая новая версия, вышедшая до следующей iOS после телефона.
/// `version` — `null`, если не нашли (`error` — почему, если из-за Apple). `next_ios` — номер
/// той iOS. `none_older` — все версии вышли позже: подходящей, скорее всего, нет.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FoundVersionDto {
    pub version: Option<VersionDto>,
    pub next_ios: Option<u64>,
    pub none_older: bool,
    /// Запросы кончились, подходящей среди проверенных нет.
    pub exhausted: bool,
    pub error: Option<UiError>,
    pub wait_s: Option<u64>,
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
    RowsDto { rows: with_sizes(with_unknown(rows, ctx), ctx), ru_check_failed: false, purchases_error: None, source_udid: None }
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

/// Полный список после загрузки покупок. Если Lookup ответил, в конце — группа «С этого iPhone»
/// (`phone_rows`); `present_ru` должен покрывать и `phone_candidates`.
pub fn purchase_rows(ctx: &RowCtx, purchases: Vec<OwnedApp>, present_ru: Option<&HashSet<i64>>) -> RowsDto {
    let mut rows = crate::catalog::build_rows(ctx.catalog, ctx.owned, purchases, present_ru, ctx.saved);
    apply_versions(&mut rows.rows, ctx.installed, ctx.latest, ctx.saved);
    let d = rows_dto(rows, ctx.target_iphone);
    let mut list = with_unknown(d.rows, ctx);
    if let Some(present) = present_ru {
        let taken: HashSet<i64> = list.iter().map(|r| r.track_id).collect();
        list.extend(phone_rows(ctx, &taken, present));
    }
    RowsDto { rows: with_sizes(list, ctx), source_udid: present_ru.and(ctx.source_udid.map(String::from)), ..d }
}

/// Кандидаты в «С этого iPhone»: установленные приложения с App Store id, которых нет в `taken`
/// (каталог и покупки). Порядок телефона, без повторов. Их проверяют в магазине RU вместе с покупками.
pub fn phone_candidates(installed: &[InstalledApp], taken: &HashSet<i64>) -> Vec<i64> {
    let mut seen = HashSet::new();
    installed.iter().filter_map(|a| a.item_id).filter(|id| !taken.contains(id) && seen.insert(*id)).collect()
}

/// Группа «С этого iPhone»: кандидаты, которых нет в магазине RU (`present_ru`), по имени.
/// Покупатель известен и это не Apple ID программы — `other_account`: строку не выбрать.
/// Иначе права проверяет `check_owned`, а ставится она скачиванием по trackId, как каталог.
/// Сами не отмечаются: приложение уже стоит.
pub fn phone_rows(ctx: &RowCtx, taken: &HashSet<i64>, present_ru: &HashSet<i64>) -> Vec<RowDto> {
    let Some(installed) = ctx.source else { return vec![] };
    let mut rows: Vec<RowDto> = phone_candidates(installed, taken)
        .into_iter()
        .filter(|id| !present_ru.contains(id))
        .filter_map(|id| installed.iter().find(|a| a.item_id == Some(id)).map(|a| phone_row(id, a, ctx)))
        .collect();
    rows.sort_by_cached_key(|r| r.name.to_lowercase());
    rows
}

/// Группа «С этого iPhone» для выбранного источника (`set_source_phone`): `phone_rows` с размерами.
pub fn source_rows(ctx: &RowCtx, taken: &HashSet<i64>, present_ru: &HashSet<i64>) -> SourceRowsDto {
    SourceRowsDto { source_udid: ctx.source_udid.map(String::from), rows: with_sizes(phone_rows(ctx, taken, present_ru), ctx) }
}

/// Итог `set_source_phone`: строки группы и источник, по которому они построены. Интерфейс
/// берёт только ответ для источника, который выбран последним.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceRowsDto {
    pub source_udid: Option<String>,
    pub rows: Vec<RowDto>,
}

/// Один прочитанный список телефона обновляет и кэш цели (`installed`), если это она: группа и
/// «на iPhone» тогда из одного чтения. Другой телефон и «без телефона» цель не трогают.
pub fn refresh_target(cache: &mut Option<(String, Vec<InstalledApp>)>, udid: &str, apps: &[InstalledApp]) {
    if let Some((u, list)) = cache
        && u == udid
    {
        *list = apps.to_vec();
    }
}

/// Версия приложения строки «С этого iPhone» на цели установки; `None` — не стоит или цели нет.
fn target_version(ctx: &RowCtx, track_id: i64, bundle_id: &str) -> Option<String> {
    ctx.installed.and_then(|l| crate::catalog::find_installed(l, track_id, bundle_id)).map(|a| a.version.clone())
}

/// Покупатель приложения с телефона — не Apple ID программы: его маска. Сравнение без регистра и
/// пробелов по краям; покупатель неизвестен (или пуст) или программа без входа — `None`.
pub fn other_purchaser(purchaser: Option<&str>, account: Option<&str>) -> Option<String> {
    let p = purchaser?.trim();
    let me = account?;
    (!p.is_empty() && !crate::accounts::same_id(p, me)).then(|| mask_email(p))
}

fn phone_row(track_id: i64, a: &InstalledApp, ctx: &RowCtx) -> RowDto {
    let purchaser_masked = other_purchaser(a.purchaser.as_deref(), ctx.account);
    let other_account = purchaser_masked.is_some();
    let available_version = ctx.latest.get(&track_id).cloned();
    let installed_version = target_version(ctx, track_id, &a.bundle_id);
    let name = if a.name.trim().is_empty() { a.bundle_id.clone() } else { a.name.clone() };
    RowDto {
        track_id,
        has_saved_copy: crate::catalog::find_saved(ctx.saved, track_id, &a.bundle_id).is_some(),
        bundle_id: a.bundle_id.clone(),
        name,
        version: a.version.clone(),
        removed_from_ru: true,
        owned: ctx.owned.get(&track_id).copied(),
        source: "phone",
        letter: None,
        color: None,
        install_state: crate::catalog::install_state(installed_version.as_deref(), available_version.as_deref()),
        installed_version,
        available_version,
        checked: false,
        ipad_only: false,
        unknown_account: false,
        aliases: vec![],
        category: None,
        size_bytes: None,
        other_account,
        purchaser_masked,
        min_ios: None,
    }
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
        aliases: vec![],
        category: None,
        size_bytes: ctx.sizes.get(&track_id).copied(),
        other_account: false,
        purchaser_masked: None,
        min_ios: ctx.min_os.get(&track_id).cloned(),
    }
}

/// Как `fold` в rows.ts: без регистра, ё = е, пробелы схлопнуты.
fn fold(s: &str) -> String {
    s.to_lowercase().replace('ё', "е").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Search API нечёткий («сбер» → Госуслуги, Telegram…): оставляем только то, где запрос есть в
/// названии или в имени продавца.
pub fn relevant(found: Vec<crate::lookup::StoreApp>, term: &str) -> Vec<crate::lookup::StoreApp> {
    let t = fold(term);
    found.into_iter().filter(|a| fold(&a.name).contains(&t) || fold(&a.seller).contains(&t)).collect()
}

/// Что уже есть на «Выборе» и в группу «В App Store» не идёт: каталог, копии, строки после
/// загрузки покупок (`phone_taken`; `None` — ещё не загружены) и приложения телефона-источника.
pub fn store_taken(ctx: &RowCtx, screen: Option<&HashSet<i64>>) -> HashSet<i64> {
    let mut out: HashSet<i64> = ctx.catalog.iter().map(|r| r.track_id).collect();
    out.extend(ctx.saved.iter().filter_map(|s| s.track_id));
    out.extend(ctx.source.unwrap_or_default().iter().filter_map(|a| a.item_id));
    out.extend(screen.into_iter().flatten().copied());
    out
}

/// Группа «В App Store»: найденное Search API, кроме того, что уже есть (`taken`). Права — из
/// `check_owned` (`null` — ещё не проверены); сама строка не отмечается.
pub fn store_rows(found: Vec<crate::lookup::StoreApp>, taken: &HashSet<i64>, ctx: &RowCtx) -> Vec<RowDto> {
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|a| !taken.contains(&a.track_id) && seen.insert(a.track_id))
        .map(|a| {
            let available_version = ctx.latest.get(&a.track_id).cloned().or_else(|| (!a.version.is_empty()).then(|| a.version.clone()));
            let installed_version = ctx
                .installed
                .and_then(|l| crate::catalog::find_installed(l, a.track_id, &a.bundle_id))
                .map(|x| x.version.clone());
            RowDto {
                track_id: a.track_id,
                has_saved_copy: false,
                name: a.name,
                version: a.version,
                removed_from_ru: false,
                owned: ctx.owned.get(&a.track_id).copied(),
                source: "store",
                letter: None,
                color: None,
                install_state: crate::catalog::install_state(installed_version.as_deref(), available_version.as_deref()),
                installed_version,
                available_version,
                checked: false,
                ipad_only: a.ipad_only,
                unknown_account: false,
                aliases: vec![],
                category: None,
                size_bytes: ctx.sizes.get(&a.track_id).copied().or(a.size),
                other_account: false,
                purchaser_masked: None,
                min_ios: a.min_os.or_else(|| ctx.min_os.get(&a.track_id).cloned()),
                bundle_id: a.bundle_id,
            }
        })
        .collect()
}

/// trackId строк «Выбора»: каталог, строки после загрузки покупок (`taken`), приложения
/// прочитанных телефонов и последний ответ «В App Store». По ним выбираются устаревшие ответы
/// кэша прав для фоновой перепроверки.
/// `extra` — строки «Из истории покупок» и добавленные по ссылке.
pub fn screen_ids(catalog: &[i64], taken: Option<&HashSet<i64>>, phones: &HashMap<String, Vec<InstalledApp>>, store: &[i64], extra: &HashSet<i64>) -> HashSet<i64> {
    let mut out: HashSet<i64> = catalog.iter().copied().collect();
    out.extend(taken.into_iter().flatten().copied());
    out.extend(phones.values().flatten().filter_map(|a| a.item_id));
    out.extend(store.iter().copied());
    out.extend(extra.iter().copied());
    out
}

/// Ответ `latest_version` -> (права, версия). Нет прав — не ошибка.
pub fn owned_result(r: Result<Option<String>, AppError>) -> Result<(bool, Option<String>), AppError> {
    match r {
        Ok(v) => Ok((true, v)),
        Err(AppError::NotPurchased) => Ok((false, None)),
        Err(e) => Err(e),
    }
}

/// Событие `owned`: `owned = null` и `error`, если проверить не вышло; у пропавшего у Apple
/// (`app_gone`) — `owned = false` и `error` с причиной. Версии и отметка
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
        Err(e @ AppError::AppGone) => (Some(false), Some(e.clone().into())),
        Err(e) => (None, Some(e.clone().into())),
    };
    // строка «С этого iPhone»: уже стоит, сама не отмечается
    if row.is_none()
        && let Some(a) = ctx.source.and_then(|l| l.iter().find(|a| a.item_id == Some(track_id)))
    {
        let available_version = ctx.latest.get(&track_id).cloned().or_else(|| r.as_ref().ok().and_then(|x| x.1.clone()));
        let install_state = crate::catalog::install_state(target_version(ctx, track_id, &a.bundle_id).as_deref(), available_version.as_deref());
        return OwnedEvent { track_id, owned, error, available_version, install_state, checked: false };
    }
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

/// Состояние задания для интерфейса. `kind`: queued, downloading, saved, installing, installed,
/// failed, waiting, paused.
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
    /// Apple просит подождать (429/503): повтор скачивания через `seconds`.
    Waiting { seconds: u64 },
    /// «Пауза» оборвала скачивание; после «Продолжить» начнётся заново.
    Paused,
}

pub fn job_dto(s: &JobState) -> JobDto {
    match s {
        JobState::Queued => JobDto::Queued,
        JobState::Downloading { bytes } => JobDto::Downloading { bytes: *bytes },
        JobState::Saved { .. } => JobDto::Saved,
        JobState::Installing { percent } => JobDto::Installing { percent: *percent },
        JobState::Installed => JobDto::Installed,
        JobState::Failed { error, saved } => JobDto::Failed { error: error.clone().into(), saved: saved.is_some() },
        JobState::Waiting { seconds } => JobDto::Waiting { seconds: *seconds },
        JobState::Paused => JobDto::Paused,
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
/// `chosen` — выбрана другая версия («Другая версия…»): скачивается она, копия не в счёт.
pub fn plan_start(
    items: Vec<(i64, String)>,
    catalog_ids: &HashSet<i64>,
    saved: &[SavedIpa],
    unknown: &[SavedIpa],
    chosen: &HashSet<i64>,
) -> StartPlan {
    let mut download = vec![];
    let mut install = vec![];
    for (id, name) in items {
        if chosen.contains(&id) {
            download.push((id, name));
            continue;
        }
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
    matches!(e, AppError::AppleChangedLogin | AppError::AppleUnavailable { .. } | AppError::NotLoggedIn | AppError::Timeout | AppError::DownloadStalled)
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
/// второй вызов (следующая смена телефона) не поставил их ещё раз. Задания «Только скачать»
/// (`download_only`) остаются `Saved`: их на телефон не ставим. Задания со своим телефоном
/// (`pinned`, «Продолжить» прошлый раз) ставятся только на него: другая цель их не берёт.
pub fn take_waiting(jobs: &mut HashMap<i64, JobState>, download_only: &HashSet<i64>, pinned: &HashMap<i64, String>, target: Option<&str>) -> Vec<(i64, PathBuf)> {
    let mut out = vec![];
    let fits = |id: &i64| pinned.get(id).is_none_or(|u| Some(u.as_str()) == target);
    for (id, s) in jobs.iter_mut().filter(|(id, _)| !download_only.contains(id) && fits(id)) {
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
            JobState::Queued | JobState::Paused => queue_dropped.contains(id),
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

/// Итог `phone_account` для интерфейса. Адрес телефона только маской (`ow***@example.com`).
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

/// «Проверить весь каталог» (команда `check_catalog`, событие `catalog_check`): «проверено N из M».
/// `running: false` — проверка кончилась или остановлена.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SweepDto {
    pub checked: usize,
    pub total: usize,
    pub running: bool,
}

/// Итог перетаскивания в окно (событие `dropped`): принятые копии, номера для «Добавить по
/// ссылке» и ошибки. `no_files` — перетащили не файл (текст или ссылку из браузера): событие
/// Tauri приносит только пути файлов.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DropDto {
    pub copies: Vec<CopyDto>,
    pub ids: Vec<i64>,
    pub errors: Vec<UiError>,
    pub no_files: bool,
}

pub fn drop_dto(out: crate::dropped::Outcome, no_files: bool) -> DropDto {
    DropDto { copies: out.copies.iter().map(copy_dto).collect(), ids: out.ids, errors: out.errors.into_iter().map(UiError::from).collect(), no_files }
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
        Some(JobState::Queued | JobState::Downloading { .. } | JobState::Waiting { .. } | JobState::Paused | JobState::Installing { .. }) => RetryAction::Nothing,
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

/// Идёт ли скачивание по аккаунту: задание из `downloads` в `Queued`, `Downloading`, `Waiting`
/// или `Paused` (ждёт «Продолжить»).
/// Установки (копии, сохранённые копии каталога) ipatool не трогают и смене входа не мешают.
pub fn downloads_running(jobs: &HashMap<i64, JobState>, downloads: &HashSet<i64>) -> bool {
    downloads.iter().any(|id| matches!(jobs.get(id), Some(JobState::Queued | JobState::Downloading { .. } | JobState::Waiting { .. } | JobState::Paused)))
}

/// Скачивание кончилось (скачано, ошибка, дальше установка) — ключ больше не скачивание.
pub fn track_download(downloads: &mut HashSet<i64>, id: i64, state: &JobState) {
    if !matches!(state, JobState::Queued | JobState::Downloading { .. } | JobState::Waiting { .. } | JobState::Paused) {
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

/// Сколько строк «С этого iPhone» дал бы каждый телефон: кандидаты, которых нет в магазине RU.
pub fn source_counts(lists: &[(String, Vec<InstalledApp>)], taken: &HashSet<i64>, present_ru: &HashSet<i64>) -> HashMap<String, usize> {
    lists.iter().map(|(udid, apps)| (udid.clone(), phone_candidates(apps, taken).iter().filter(|id| !present_ru.contains(id)).count())).collect()
}

/// Телефон-источник по умолчанию из подключённых (`phones`, порядок опроса): цель, если у неё есть
/// строки группы; иначе первый другой с ними, иначе первый другой; цели нет — первый подключённый.
pub fn default_source(phones: &[String], target: Option<&str>, counts: &HashMap<String, usize>) -> Option<String> {
    let has = |u: &str| counts.get(u).is_some_and(|n| *n > 0);
    let Some(t) = target.filter(|t| phones.iter().any(|p| p == t)) else { return phones.first().cloned() };
    if has(t) {
        return Some(t.to_string());
    }
    let others = || phones.iter().filter(|p| p.as_str() != t);
    others().find(|p| has(p)).or_else(|| others().next()).cloned().or_else(|| Some(t.to_string()))
}

/// Выбор человека (`set_source_phone`) главнее, пока этот телефон подключён.
pub fn resolve_source(chosen: Option<&str>, phones: &[String], default: Option<String>) -> Option<String> {
    chosen.filter(|c| phones.iter().any(|p| p == c)).map(String::from).or(default)
}

/// Начало адреса «Сообщить в каталог». Открывается только адрес, который начинается ровно с него.
pub const REPORT_PREFIX: &str = "https://github.com/Jkaotlic/app-restore/issues/new?";

/// Текст с телефона для issue: без управляющих символов, пробелы сжаты, не длиннее `max` символов.
fn issue_text(s: &str, max: usize) -> String {
    let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// «Сообщить в каталог»: заполненный issue на GitHub. Только номер, имя, версия и bundle id
/// приложения — ни Apple ID, ни имени человека, ни udid, ни путей.
pub fn report_url(track_id: i64, name: &str, version: &str, bundle_id: &str) -> String {
    let name = issue_text(name, 120);
    let (version, bundle_id) = (issue_text(version, 40), issue_text(bundle_id, 160));
    let title = format!("Каталог: {name} (id{track_id})");
    let body = format!(
        "Приложение стоит на телефоне, но его нет в каталоге и в российском App Store.\n\n\
         - Номер (trackId): {track_id}\n\
         - Название: {name}\n\
         - Версия на телефоне: {version}\n\
         - Bundle ID: {bundle_id}\n\
         - Ссылка: https://apps.apple.com/ru/app/id{track_id}\n"
    );
    format!("{REPORT_PREFIX}title={}&body={}", crate::lookup::url_encode(&title), crate::lookup::url_encode(&body))
}

/// Можно ли открыть `url` для «Сообщить в каталог»: ровно наш префикс, дальше только алфавит
/// `url_encode` и разделители параметров (`A-Z a-z 0-9 - _ . ~ % = &`): ни `#`, ни `@`, ни `\`.
pub fn is_report_url(url: &str) -> bool {
    url.strip_prefix(REPORT_PREFIX)
        .is_some_and(|q| !q.is_empty() && q.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~%=&".contains(c)))
}

/// «Открыть в App Store»: страница приложения в российском магазине. Открывает Rust, адрес
/// проверяет `is_store_url` — разрешения opener у интерфейса не расширяются.
pub const STORE_PREFIX: &str = "https://apps.apple.com/ru/app/id";

pub fn store_url(track_id: i64) -> String {
    format!("{STORE_PREFIX}{track_id}")
}

/// Ровно `STORE_PREFIX` и номер из цифр (без знака, не ноль, до 19 цифр).
pub fn is_store_url(url: &str) -> bool {
    url.strip_prefix(STORE_PREFIX)
        .is_some_and(|n| (1..=19).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) && n.parse::<i64>().is_ok_and(|id| id > 0))
}

/// Итог «Забыть»: убранные задания и вышли ли из нынешнего входа — даже если снимок убрать
/// не вышло (`error`): тогда адрес остаётся в списке.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ForgetDto {
    pub dropped: Vec<i64>,
    pub signed_out: bool,
    pub error: Option<UiError>,
}

pub fn forget_dto(dropped: Vec<i64>, signed_out: bool, forgot: Result<(), AppError>) -> ForgetDto {
    ForgetDto { dropped, signed_out, error: forgot.err().map(UiError::from) }
}

/// Подсказка про окно Связки ключей на время одного шага. Показывает `on_slow` (если шаг
/// затянулся), убирает `done` — только если этот шаг её показал и других показанных нет.
/// `active` — общий счётчик показанных подсказок.
pub struct HintScope<'a, F: Fn(Option<String>)> {
    emit: F,
    active: &'a std::sync::atomic::AtomicUsize,
    fired: std::sync::atomic::AtomicBool,
}

impl<'a, F: Fn(Option<String>)> HintScope<'a, F> {
    pub fn new(active: &'a std::sync::atomic::AtomicUsize, emit: F) -> Self {
        Self { emit, active, fired: std::sync::atomic::AtomicBool::new(false) }
    }

    pub fn on_slow(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        if !self.fired.swap(true, SeqCst) {
            self.active.fetch_add(1, SeqCst);
        }
        (self.emit)(Some(AppError::KeychainWaiting.user_text()));
    }

    pub fn done(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.fired.swap(false, SeqCst) && self.active.fetch_sub(1, SeqCst) == 1 {
            (self.emit)(None);
        }
    }
}

/// Шаг кончился как угодно (и паникой) — его подсказка не остаётся висеть.
impl<F: Fn(Option<String>)> Drop for HintScope<'_, F> {
    fn drop(&mut self) {
        self.done();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn broken_metadata_is_a_version_without_details() {
        let m = meta_or_blank(Err(AppError::VersionMetaBroken { released: Some("2022-04-04T11:59:04".into()) })).unwrap();
        assert_eq!(version_dto("7", Some(&m)), VersionDto { id: "7".into(), display: None, released: Some("2022-04-04".into()), checked: true });
        let m = meta_or_blank(Err(AppError::VersionMetaBroken { released: None })).unwrap();
        assert_eq!(version_dto("7", Some(&m)), VersionDto { id: "7".into(), display: None, released: None, checked: true });
        assert_eq!(meta_or_blank(Err(AppError::AppleUnavailable { status: Some(429) })), Err(AppError::AppleUnavailable { status: Some(429) }));
    }

    use super::*;
    use std::time::Duration;

    #[test]
    fn store_url_only_for_app_ids() {
        assert_eq!(store_url(1631810724), "https://apps.apple.com/ru/app/id1631810724");
        assert!(is_store_url(&store_url(1631810724)));
        for bad in [
            "",
            "https://apps.apple.com/ru/app/id",
            "https://apps.apple.com/ru/app/id0",
            "https://apps.apple.com/ru/app/id-5",
            "https://apps.apple.com/ru/app/id12a",
            "https://apps.apple.com/ru/app/id123?x=1",
            "https://apps.apple.com/ru/app/id123/../../evil",
            "https://apps.apple.com/ru/app/id123 ",
            "https://apps.apple.com/us/app/id123",
            "http://apps.apple.com/ru/app/id123",
            "https://apps.apple.com.evil.example/ru/app/id123",
            "https://apps.apple.com/ru/app/id99999999999999999999",
        ] {
            assert!(!is_store_url(bad), "{bad}");
        }
        assert!(!is_store_url(&store_url(0)) && !is_store_url(&store_url(-7)));
    }

    #[test]
    fn drop_dto_masks_purchaser_and_maps_errors() {
        let copy = crate::library::CopyInfo {
            path: "/lib/a.ipa".into(),
            track_id: Some(5),
            bundle_id: "com.example.a".into(),
            name: "А".into(),
            version: "1.0".into(),
            device_family: vec![1],
            purchaser: Some("person@example.com".into()),
            account_folder: None,
        };
        let out = crate::dropped::Outcome { copies: vec![copy], ids: vec![492224193], errors: vec![AppError::NotIpa] };
        let dto = drop_dto(out, false);
        assert_eq!(dto.copies[0].path, "/lib/a.ipa");
        assert!(!serde_json::to_string(&dto).unwrap().contains("person@example.com"));
        assert_eq!(dto.ids, vec![492224193]);
        assert_eq!(dto.errors[0].code, "not_ipa");
        assert!(!dto.no_files);
        assert!(drop_dto(crate::dropped::Outcome::default(), true).no_files);
    }

    /// Вышли, а снимок убрать не вышло: убранные задания и выход не теряются вместе с ошибкой.
    #[test]
    fn forget_keeps_dropped_jobs_with_the_error() {
        let ok = forget_dto(vec![3], true, Ok(()));
        assert_eq!((ok.dropped.clone(), ok.signed_out, ok.error), (vec![3], true, None));
        let half = forget_dto(vec![3, 4], true, Err(AppError::Busy));
        assert_eq!((half.dropped, half.signed_out), (vec![3, 4], true));
        assert_eq!(half.error.unwrap().code, "busy");
        let other = forget_dto(vec![], false, Err(AppError::KeychainWaiting));
        assert!(!other.signed_out && other.error.is_some());
    }

    #[test]
    fn hint_is_cleared_only_by_the_step_that_showed_it() {
        let active = std::sync::atomic::AtomicUsize::new(0);
        let log = std::sync::Mutex::new(Vec::<Option<String>>::new());
        let emit = |v: Option<String>| log.lock().unwrap().push(v.map(|_| "hint".into()));
        let slow = HintScope::new(&active, emit);
        let quick = HintScope::new(&active, emit);
        slow.on_slow();
        quick.done(); // быстрый шаг (или Busy) подсказку не показывал — и не убирает
        assert_eq!(*log.lock().unwrap(), [Some("hint".to_string())]);
        let other = HintScope::new(&active, emit);
        other.on_slow();
        slow.done(); // второй ещё ждёт окна
        assert_eq!(log.lock().unwrap().len(), 2);
        other.done();
        assert_eq!(log.lock().unwrap().last().unwrap(), &None);
        other.done(); // повтор ничего не шлёт
        assert_eq!(log.lock().unwrap().len(), 3);
    }

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
            AppError::AppGone,
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
            AppError::DownloadStalled,
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
            category: None,
            disguised: false,
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
                aliases: vec![],
                category: Some("other"),
                size_bytes: None,
                other_account: false,
                purchaser_masked: None,
                min_ios: None,
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
        sizes: HashMap<i64, u64>,
        account: Option<String>,
        /// Телефон-источник «С этого iPhone»; `None` — тот же, что цель (`installed`).
        source: Option<Vec<InstalledApp>>,
        source_udid: Option<String>,
        min_os: HashMap<i64, String>,
    }
    impl Ctx {
        fn new(catalog: Vec<RemovedApp>) -> Self {
            Self {
                catalog,
                owned: HashMap::new(),
                latest: HashMap::new(),
                installed: None,
                saved: vec![],
                unknown: vec![],
                target_iphone: false,
                sizes: HashMap::new(),
                account: None,
                source: None,
                source_udid: None,
                min_os: HashMap::new(),
            }
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
                sizes: &self.sizes,
                account: self.account.as_deref(),
                source: self.source.as_deref().or(self.installed.as_deref()),
                source_udid: self.source_udid.as_deref(),
                min_os: &self.min_os,
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
    fn rows_carry_category_aliases_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("900_b_1.ipa");
        std::fs::write(&copy, vec![0u8; 1234]).unwrap();
        let sber = RemovedApp { aliases: vec!["СБОЛ".into()], category: Some("bank".into()), ..removed(900, "Сбер") };
        let mut c = Ctx::new(vec![sber, removed(901, "Без категории")]);
        c.saved = vec![SavedIpa { path: copy, ..saved(900, "") }];
        c.sizes = [(1, 5_000_000)].into();
        let d = purchase_rows(&c.get(), vec![app(1, "com.example.a", "A"), app(2, "com.example.b", "B")], Some(&[1, 2].into()));
        let got: Vec<_> = d.rows.iter().map(|r| (r.track_id, r.category, r.aliases.clone(), r.size_bytes)).collect();
        assert_eq!(
            got,
            [
                (900, Some("bank"), vec!["СБОЛ".to_string()], Some(1234)),
                // нет в файле — «другое»
                (901, Some("other"), vec![], None),
                // у покупок категории нет; размер из Lookup, если Apple его дал
                (1, None, vec![], Some(5_000_000)),
                (2, None, vec![], None),
            ]
        );
        let json = serde_json::to_value(&d.rows[3]).unwrap();
        assert_eq!((&json["category"], &json["size_bytes"], &json["other_account"]), (&serde_json::Value::Null, &serde_json::Value::Null, &serde_json::json!(false)));
    }

    fn bought_by(bundle: &str, version: &str, item: i64, purchaser: Option<&str>) -> InstalledApp {
        InstalledApp { name: format!("Имя {bundle}"), purchaser: purchaser.map(String::from), ..inst(bundle, version, Some(item)) }
    }

    #[test]
    fn phone_candidates_skip_catalog_purchases_and_apps_without_item_id() {
        let installed = vec![
            inst("com.example.catalog", "1", Some(900)),
            inst("com.example.bought", "1", Some(1)),
            inst("com.example.enterprise", "1", None),
            inst("com.example.gone", "1", Some(50)),
            inst("com.example.gone2", "1", Some(50)),
            inst("com.example.other", "1", Some(60)),
        ];
        assert_eq!(phone_candidates(&installed, &[900, 1].into()), [50, 60]);
        assert!(phone_candidates(&[], &HashSet::new()).is_empty());
    }

    #[test]
    fn phone_group_has_only_apps_missing_from_ru_store() {
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.account = Some("User@Example.com".into());
        c.installed = Some(vec![
            inst("com.example.catalog", "1", Some(900)),
            inst("com.example.a", "1.0", Some(1)),
            bought_by("com.example.zeta", "2.0", 70, Some("user@example.com")),
            bought_by("com.example.alpha", "14.2", 60, None),
            bought_by("com.example.live", "1.0", 80, Some("user@example.com")),
            bought_by("com.example.foreign", "3.0", 90, Some("other@example.com")),
        ]);
        c.owned = [(70, true)].into();
        c.latest = [(70, "2.1".to_string())].into();
        // 80 есть в магазине RU — обычное приложение, в группу не идёт
        let d = purchase_rows(&c.get(), vec![app(1, "com.example.a", "A")], Some(&[1, 80].into()));
        let phone: Vec<&RowDto> = d.rows.iter().filter(|r| r.source == "phone").collect();
        assert_eq!(phone.iter().map(|r| r.track_id).collect::<Vec<_>>(), [60, 90, 70], "по имени");
        let alpha = phone[0];
        assert_eq!((alpha.name.as_str(), alpha.bundle_id.as_str(), alpha.installed_version.as_deref()), ("Имя com.example.alpha", "com.example.alpha", Some("14.2")));
        assert_eq!((alpha.owned, alpha.other_account, alpha.checked, alpha.removed_from_ru), (None, false, false, true));
        let foreign = phone[1];
        assert!(foreign.other_account && !foreign.checked);
        let json = serde_json::to_string(foreign).unwrap();
        assert!(!json.contains("other@"), "адрес покупателя не уходит в интерфейс: {json}");
        let zeta = phone[2];
        assert_eq!((zeta.owned, zeta.available_version.as_deref(), zeta.install_state, zeta.checked), (Some(true), Some("2.1"), InstallState::UpdateAvailable, false), "уже стоит — не отмечаем сами");
        // Lookup упал — в магазине ли приложение, неизвестно: группы нет
        assert!(purchase_rows(&c.get(), vec![], None).rows.iter().all(|r| r.source != "phone"));
        // без телефона группы нет
        c.installed = None;
        assert!(purchase_rows(&c.get(), vec![], Some(&HashSet::new())).rows.iter().all(|r| r.source != "phone"));
    }

    #[test]
    fn phone_rows_owned_event_never_checks_itself() {
        let mut c = Ctx::new(vec![]);
        c.installed = Some(vec![bought_by("com.example.gone", "1.0", 60, None)]);
        c.owned = [(60, true)].into();
        c.latest = [(60, "1.1".to_string())].into();
        let e = owned_event(60, &Ok((true, Some("1.1".into()))), &c.get());
        assert_eq!((e.owned, e.available_version.as_deref(), e.install_state, e.checked), (Some(true), Some("1.1"), InstallState::UpdateAvailable, false));
    }

    #[test]
    fn phone_row_downloads_by_track_id_like_catalog() {
        // не каталог, своей копии нет — скачивание по trackId
        let (download, install) = plan_start(vec![(60, "Онлайн".into())], &HashSet::new(), &[], &[], &HashSet::new());
        assert_eq!((download, install), (vec![(60, "Онлайн".to_string())], vec![]));
    }

    #[test]
    fn screen_ids_cover_catalog_purchases_phones_store_history_and_links() {
        let phones: HashMap<String, Vec<InstalledApp>> = [("u1".to_string(), vec![inst("com.example.a", "1", Some(30)), inst("com.example.b", "1", None)])].into();
        let taken: HashSet<i64> = [20].into();
        let got = screen_ids(&[10], Some(&taken), &phones, &[40, 10], &[50].into());
        assert_eq!(got, [10, 20, 30, 40, 50].into());
        assert_eq!(screen_ids(&[], None, &HashMap::new(), &[40], &HashSet::new()), [40].into());
        // строки «Из истории покупок» и добавленные по ссылке — тоже на экране
        assert_eq!(screen_ids(&[], None, &HashMap::new(), &[], &[60, 70].into()), [60, 70].into());
    }

    #[test]
    fn owned_result_mapping() {
        assert_eq!(owned_result(Ok(Some("1.0".into()))), Ok((true, Some("1.0".into()))));
        assert_eq!(owned_result(Ok(None)), Ok((true, None)));
        assert_eq!(owned_result(Err(AppError::NotPurchased)), Ok((false, None)));
        assert_eq!(owned_result(Err(AppError::NotLoggedIn)), Err(AppError::NotLoggedIn));
        // пропавшее у Apple — отдельный ответ, не «нет прав» и не сбой
        assert_eq!(owned_result(Err(AppError::AppGone)), Err(AppError::AppGone));
    }

    #[test]
    fn owned_event_app_gone_is_not_owned_with_reason() {
        let c = Ctx::new(vec![removed(1, "A")]);
        let e = owned_event(1, &Err(AppError::AppGone), &c.get());
        assert_eq!((e.owned, e.checked), (Some(false), false));
        let err = e.error.unwrap();
        assert_eq!((err.code.as_str(), err.retryable), ("app_gone", false));
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
        assert_eq!(serde_json::to_value(job_dto(&JobState::Waiting { seconds: 5 })).unwrap(), serde_json::json!({"kind":"waiting","seconds":5}));
    }

    /// Пауза повтора после 429/503 и «Пауза» очереди — скачивание ещё не кончилось: вход не
    /// сменить, «Повторить» не нужен (паузу снимает «Продолжить»).
    #[test]
    fn waiting_and_paused_are_still_downloads() {
        for st in [JobState::Waiting { seconds: 10 }, JobState::Paused] {
            let mut downloads = HashSet::from([1]);
            track_download(&mut downloads, 1, &st);
            assert!(downloads_running(&HashMap::from([(1, st.clone())]), &downloads), "{st:?}");
            assert_eq!(retry_action(Some(&st)), RetryAction::Nothing);
            assert_eq!(copy_retry(Some(&st), std::path::Path::new("/x.ipa")), RetryAction::Nothing);
        }
        assert_eq!(serde_json::to_value(job_dto(&JobState::Paused)).unwrap(), serde_json::json!({"kind":"paused"}));
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
        let (down, inst) = plan_start(items, &catalog, &lib, &[], &HashSet::new());
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
        let (down, inst) = plan_start(items, &catalog, &lib, &unknown, &HashSet::new());
        // 8 — только копия неизвестного Apple ID: скачать её нашим входом нельзя, ставим как есть.
        // 2 — запись каталога, 9 — есть своя копия: как раньше.
        assert_eq!(down, vec![(2, "C".to_string()), (9, "M".to_string())]);
        assert_eq!(inst, vec![(8, PathBuf::from("/x/8_u.ipa"))]);
    }

    /// Выбрана другая версия: скачивается она, даже если копия лежит (копия — другой версии).
    #[test]
    fn start_downloads_chosen_version_even_with_a_copy() {
        let lib = [saved(2, "/x/2.ipa")];
        let unknown = [saved(8, "/x/8_u.ipa")];
        let items = vec![(2, "C".into()), (8, "U".into())];
        let (down, inst) = plan_start(items, &[2].into(), &lib, &unknown, &[2, 8].into());
        assert_eq!(down, vec![(2, "C".to_string()), (8, "U".to_string())]);
        assert!(inst.is_empty());
    }

    #[test]
    fn rows_carry_minimum_ios_of_what_will_be_installed() {
        let mut c = Ctx::new(vec![removed(900, "Сбер"), removed(901, "ВТБ")]);
        c.saved = vec![saved(900, "/x/900_b_1.ipa")];
        c.min_os = [(1, "16.0".to_string()), (900, "15.0".to_string()), (901, "14.0".to_string())].into();
        let d = purchase_rows(&c.get(), vec![app(1, "com.example.a", "A"), app(2, "com.example.b", "B")], Some(&[1, 2].into()));
        let got: Vec<_> = d.rows.iter().map(|r| (r.track_id, r.min_ios.clone())).collect();
        // 900 ставится из копии: минимальная iOS последней версии к ней не относится
        assert_eq!(got, [(900, None), (901, Some("14.0".into())), (1, Some("16.0".into())), (2, None)]);
        let found = vec![crate::lookup::StoreApp { min_os: Some("17.2".into()), ..store_app(14, "Новое") }];
        assert_eq!(store_rows(found, &HashSet::new(), &c.get())[0].min_ios.as_deref(), Some("17.2"));
        c.owned = [(1, true)].into();
        assert_eq!(link_row(1, &c.get(), None).min_ios.as_deref(), Some("16.0"));
        assert_eq!(serde_json::to_value(&d.rows[3]).unwrap()["min_ios"], serde_json::Value::Null);
    }

    #[test]
    fn versions_page_newest_first() {
        let ids: Vec<String> = (0..12).map(|i| format!("v{i}")).collect();
        let (page, more) = versions_page(&ids, None, 10);
        assert_eq!(page, (2..12).rev().map(|i| format!("v{i}")).collect::<Vec<_>>());
        assert!(more);
        let (page, more) = versions_page(&ids, Some("v2"), 10);
        assert_eq!(page, ["v1", "v0"]);
        assert!(!more);
        assert_eq!(versions_page(&ids, Some("нет"), 10), (vec![], false));
        assert_eq!(versions_page(&[], None, 10), (vec![], false));
    }

    #[test]
    fn meta_gate_waits_a_minute_after_429() {
        let t0 = std::time::Instant::now();
        let mut g = MetaGate::default();
        assert_eq!(g.wait_left(t0), None);
        g.note(&AppError::NotPurchased, t0);
        assert_eq!(g.wait_left(t0), None, "только 429");
        g.note(&AppError::AppleUnavailable { status: Some(429) }, t0);
        assert_eq!(g.wait_left(t0), Some(60));
        assert_eq!(g.wait_left(t0 + Duration::from_millis(30_500)), Some(30), "секунды вверх");
        assert_eq!(g.wait_left(t0 + Duration::from_secs(60)), None);
        let e = versions_error(AppError::AppleUnavailable { status: Some(429) }, Some(42));
        assert_eq!(e.text, "Apple просит подождать: слишком много запросов о версиях. Можно снова через 42 с.");
    }

    #[test]
    fn find_version_only_for_an_iphone_target() {
        let phone = |udid: &str, class: &str| PhoneInfo { udid: udid.into(), name: "n".into(), ios_version: "15.8".into(), device_class: class.into(), product_type: "p".into() };
        let list = [phone("A", "iPhone"), phone("B", "iPad")];
        assert_eq!(target_ios(&list, Some("A")), Ok("15.8".to_string()));
        assert_eq!(target_ios(&list, Some("B")), Err(AppError::DeviceRejected { reason: "Подходящую версию ищем только для iPhone".into() }));
        assert_eq!(target_ios(&list, None), Err(AppError::DeviceNotFound { drivers_missing: false }));
        assert_eq!(target_ios(&list, Some("C")), Err(AppError::DeviceNotFound { drivers_missing: false }));
    }

    #[test]
    fn unasked_version_is_marked() {
        assert!(!version_dto("1", None).checked);
        let m = crate::ios_compat::VersionMeta { display: "1.0".into(), released: None };
        assert!(version_dto("1", Some(&m)).checked);
    }

    #[test]
    fn versions_error_explains_429() {
        let e = versions_error(AppError::AppleUnavailable { status: Some(429) }, None);
        assert_eq!((e.code.as_str(), e.text.as_str()), ("apple_unavailable", "Apple просит подождать: слишком много запросов о версиях. Попробуйте через несколько минут."));
        assert_eq!(versions_error(AppError::NotPurchased, Some(5)).text, AppError::NotPurchased.user_text());
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
        for e in [AppError::AppleChangedLogin, AppError::AppleUnavailable { status: Some(503) }, AppError::NotLoggedIn, AppError::Timeout, AppError::DownloadStalled] {
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
        let mut w = take_waiting(&mut jobs, &HashSet::new(), &HashMap::new(), Some("U1"));
        w.sort();
        assert_eq!(w, vec![(1, PathBuf::from("/x/1.ipa")), (4, PathBuf::from("/x/4.ipa"))]);
        assert_eq!((&jobs[&1], &jobs[&4]), (&JobState::Queued, &JobState::Queued));
        assert!(take_waiting(&mut jobs, &HashSet::new(), &HashMap::new(), Some("U1")).is_empty(), "второй вызов ничего не ставит");
        assert!(matches!(jobs[&3], JobState::Failed { .. }));
    }

    #[test]
    fn take_waiting_skips_download_only() {
        let mut jobs: HashMap<i64, JobState> =
            [(1, JobState::Saved { path: "/x/1.ipa".into() }), (2, JobState::Saved { path: "/x/2.ipa".into() })].into();
        assert_eq!(take_waiting(&mut jobs, &[2].into(), &HashMap::new(), Some("U1")), vec![(1, PathBuf::from("/x/1.ipa"))]);
        // «Только скачать»: остаётся «Сохранено», телефон его не ставит
        assert_eq!(jobs[&2], JobState::Saved { path: "/x/2.ipa".into() });
    }

    /// Продолженное задание со своим телефоном: на другой телефон не ставится, на свой — да.
    #[test]
    fn take_waiting_keeps_pinned_jobs_for_their_own_phone() {
        let mut jobs: HashMap<i64, JobState> =
            [(1, JobState::Saved { path: "/x/1.ipa".into() }), (2, JobState::Saved { path: "/x/2.ipa".into() })].into();
        let pinned = HashMap::from([(2, "MINE".to_string())]);
        assert_eq!(take_waiting(&mut jobs, &HashSet::new(), &pinned, Some("OTHER")), vec![(1, PathBuf::from("/x/1.ipa"))]);
        assert_eq!(jobs[&2], JobState::Saved { path: "/x/2.ipa".into() }, "ждёт свой телефон");
        assert_eq!(take_waiting(&mut jobs, &HashSet::new(), &pinned, Some("MINE")), vec![(2, PathBuf::from("/x/2.ipa"))]);
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

    /// Прерванное «Паузой» ждёт в очереди, как `Queued`: отброшено очередью — убирается.
    #[test]
    fn drop_pending_removes_paused_the_queue_dropped() {
        let mut jobs: HashMap<i64, JobState> = [(1, JobState::Paused), (2, JobState::Paused)].into();
        assert_eq!(drop_pending(&mut jobs, &[1]), vec![1]);
        assert!(jobs.contains_key(&2));
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

    #[test]
    fn other_purchaser_is_case_insensitive_trimmed_and_masked() {
        assert_eq!(other_purchaser(Some(" User@Example.COM "), Some("user@example.com")), None);
        assert_eq!(other_purchaser(Some("other@example.com"), Some("user@example.com")).as_deref(), Some("ot***@example.com"));
        assert_eq!(other_purchaser(Some("  "), Some("user@example.com")), None, "пустой покупатель — неизвестен");
        assert_eq!(other_purchaser(None, Some("user@example.com")), None);
        assert_eq!(other_purchaser(Some("other@example.com"), None), None, "без входа сравнивать не с чем");
    }

    #[test]
    fn foreign_phone_row_carries_only_the_mask() {
        let mut c = Ctx::new(vec![]);
        c.account = Some("user@example.com".into());
        c.installed = Some(vec![bought_by("com.example.foreign", "3.0", 90, Some("other@example.com")), bought_by("com.example.mine", "1.0", 91, Some(" USER@example.com "))]);
        let d = purchase_rows(&c.get(), vec![], Some(&HashSet::new()));
        let foreign = d.rows.iter().find(|r| r.track_id == 90).unwrap();
        assert!(foreign.other_account);
        assert_eq!(foreign.purchaser_masked.as_deref(), Some("ot***@example.com"));
        let json = serde_json::to_string(foreign).unwrap();
        assert!(!json.contains("other@"), "{json}");
        let mine = d.rows.iter().find(|r| r.track_id == 91).unwrap();
        assert_eq!((mine.other_account, mine.purchaser_masked.as_deref()), (false, None));
    }

    #[test]
    fn source_phone_rows_show_target_state() {
        // источник — старый телефон, цель — новый: версия «на iPhone» и состояние по цели
        let mut c = Ctx::new(vec![]);
        c.installed = Some(vec![inst("com.example.both", "2.0", Some(61))]);
        c.source = Some(vec![bought_by("com.example.gone", "1.0", 60, None), bought_by("com.example.both", "1.5", 61, None)]);
        c.latest = [(60, "1.1".to_string())].into();
        let d = purchase_rows(&c.get(), vec![], Some(&HashSet::new()));
        let phone: Vec<&RowDto> = d.rows.iter().filter(|r| r.source == "phone").collect();
        assert_eq!(phone.len(), 2);
        let gone = phone.iter().find(|r| r.track_id == 60).unwrap();
        assert_eq!((gone.version.as_str(), gone.installed_version.as_deref(), gone.install_state), ("1.0", None, InstallState::NotInstalled));
        let both = phone.iter().find(|r| r.track_id == 61).unwrap();
        assert_eq!((both.version.as_str(), both.installed_version.as_deref()), ("1.5", Some("2.0")));
        // без цели (без iPhone) группа строится по источнику
        c.installed = None;
        let d = purchase_rows(&c.get(), vec![], Some(&HashSet::new()));
        assert_eq!(d.rows.iter().filter(|r| r.source == "phone").count(), 2);
        // событие `owned` строки источника: не отмечается, состояние по цели
        c.installed = Some(vec![]);
        let e = owned_event(60, &Ok((true, Some("1.1".into()))), &c.get());
        assert_eq!((e.install_state, e.checked), (InstallState::NotInstalled, false));
    }

    #[test]
    fn source_rows_apply_sizes() {
        let mut c = Ctx::new(vec![]);
        c.source = Some(vec![bought_by("com.example.gone", "1.0", 60, None), bought_by("com.example.live", "1.0", 61, None)]);
        c.sizes = [(60, 5_000_000)].into();
        let rows = source_rows(&c.get(), &HashSet::new(), &[61].into());
        assert_eq!(rows.rows.iter().map(|r| (r.track_id, r.size_bytes)).collect::<Vec<_>>(), [(60, Some(5_000_000))]);
    }

    #[test]
    fn source_counts_per_phone() {
        let lists = vec![
            ("NEW".to_string(), vec![inst("a", "1", Some(1)), inst("b", "1", Some(2))]),
            ("OLD".to_string(), vec![inst("c", "1", Some(3)), inst("d", "1", Some(4)), inst("e", "1", None)]),
        ];
        // 1 — в каталоге, 2 и 4 — есть в магазине RU
        let counts = source_counts(&lists, &[1].into(), &[2, 4].into());
        assert_eq!(counts, [("NEW".to_string(), 0), ("OLD".to_string(), 1)].into());
    }

    #[test]
    fn source_default_target_first_if_it_has_candidates() {
        let phones = ["NEW".to_string(), "OLD".to_string(), "PAD".to_string()];
        let counts = |n: usize, o: usize, p: usize| -> HashMap<String, usize> { [("NEW".into(), n), ("OLD".into(), o), ("PAD".into(), p)].into() };
        assert_eq!(default_source(&phones, Some("NEW"), &counts(2, 5, 0)).as_deref(), Some("NEW"));
        assert_eq!(default_source(&phones, Some("NEW"), &counts(0, 5, 0)).as_deref(), Some("OLD"));
        // у цели пусто — первый другой с кандидатами, а нет таких — просто первый другой
        assert_eq!(default_source(&phones, Some("NEW"), &counts(0, 0, 3)).as_deref(), Some("PAD"));
        assert_eq!(default_source(&phones, Some("NEW"), &counts(0, 0, 0)).as_deref(), Some("OLD"));
        assert_eq!(default_source(&phones[..1], Some("NEW"), &counts(0, 0, 0)).as_deref(), Some("NEW"));
        // цель не выбрана или отключена — первый подключённый
        assert_eq!(default_source(&phones, None, &counts(0, 0, 0)).as_deref(), Some("NEW"));
        assert_eq!(default_source(&phones[1..], Some("NEW"), &HashMap::new()).as_deref(), Some("OLD"));
        assert_eq!(default_source(&[], Some("NEW"), &HashMap::new()), None);
    }

    #[test]
    fn source_choice_wins_while_connected() {
        let phones = ["NEW".to_string(), "OLD".to_string()];
        assert_eq!(resolve_source(Some("OLD"), &phones, Some("NEW".into())).as_deref(), Some("OLD"));
        assert_eq!(resolve_source(Some("GONE"), &phones, Some("NEW".into())).as_deref(), Some("NEW"));
        assert_eq!(resolve_source(None, &phones, Some("NEW".into())).as_deref(), Some("NEW"));
    }

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn report_url_has_only_app_fields() {
        let url = report_url(1234567890, "Сбер Онлайн & Ко", "15.2.1", "ru.example.app");
        assert!(url.starts_with(REPORT_PREFIX) && is_report_url(&url), "{url}");
        let query = &url[REPORT_PREFIX.len()..];
        let params: Vec<(&str, String)> = query.split('&').map(|kv| kv.split_once('=').map(|(k, v)| (k, decode(v))).unwrap()).collect();
        assert_eq!(params.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["title", "body"]);
        let (title, body) = (&params[0].1, &params[1].1);
        assert!(title.contains("Сбер Онлайн & Ко") && title.contains("1234567890"), "{title}");
        for part in ["1234567890", "Сбер Онлайн & Ко", "15.2.1", "ru.example.app", "https://apps.apple.com/ru/app/id1234567890"] {
            assert!(body.contains(part), "{part} в {body}");
        }
        assert!(!body.contains('@') && !body.to_lowercase().contains("apple id") && !body.contains("udid"), "{body}");
        // всё, кроме безопасных символов, закодировано: ни пробелов, ни & внутри значений
        assert!(query.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~%=&".contains(c)), "{query}");
        assert_eq!(query.matches('&').count(), 1);
    }

    #[test]
    fn report_url_cleans_name() {
        let long = "Я".repeat(300);
        let url = report_url(1, &format!(" {long}\n\u{7}x "), "", "");
        let title = decode(url[REPORT_PREFIX.len()..].split('&').next().unwrap().trim_start_matches("title="));
        assert!(!title.contains('\n') && !title.contains('\u{7}'), "{title}");
        assert!(title.chars().count() < 200, "{}", title.chars().count());
    }

    #[test]
    fn report_url_check_is_strict() {
        assert!(is_report_url("https://github.com/Jkaotlic/app-restore/issues/new?title=a&body=b"));
        for bad in [
            "https://github.com/Jkaotlic/app-restore/issues/new",
            "https://github.com/Jkaotlic/app-restore/issues/newx?title=a",
            "https://github.com/Jkaotlic/app-restore/issues/new/../../../evil?a",
            "http://github.com/Jkaotlic/app-restore/issues/new?title=a",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=a b",
            "https://github.com.evil.example.com/Jkaotlic/app-restore/issues/new?x",
            "https://github.com/Jkaotlic/other/issues/new?title=a",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=a#frag",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=a@b",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=a\\b",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=a/b",
            "https://github.com/Jkaotlic/app-restore/issues/new?title=я",
            "https://github.com/Jkaotlic/app-restore/issues/new?",
        ] {
            assert!(!is_report_url(bad), "{bad}");
        }
    }

    #[test]
    fn rows_are_tagged_with_the_source_they_were_built_for() {
        let mut c = Ctx::new(vec![]);
        c.source = Some(vec![bought_by("com.example.gone", "1.0", 60, None)]);
        c.source_udid = Some("OLD".into());
        assert_eq!(purchase_rows(&c.get(), vec![], Some(&HashSet::new())).source_udid.as_deref(), Some("OLD"));
        let d = source_rows(&c.get(), &HashSet::new(), &HashSet::new());
        assert_eq!((d.source_udid.as_deref(), d.rows.len()), (Some("OLD"), 1));
        // каталог без группы — без источника
        assert_eq!(catalog_rows(&c.get()).source_udid, None);
    }

    #[test]
    fn one_read_refreshes_the_target_cache_too() {
        let apps = vec![inst("com.example.new", "2.0", Some(60))];
        let mut cache = Some(("NEW".to_string(), vec![inst("com.example.old", "1.0", Some(60))]));
        refresh_target(&mut cache, "NEW", &apps);
        assert_eq!(cache.as_ref().map(|(u, a)| (u.as_str(), a.clone())), Some(("NEW", apps.clone())));
        // другой телефон и «без телефона» цель не трогают
        refresh_target(&mut cache, "OLD", &[]);
        assert_eq!(cache.as_ref().map(|(_, a)| a.len()), Some(1));
        let mut none = None;
        refresh_target(&mut none, "NEW", &apps);
        assert_eq!(none, None);
    }

    fn store_app(id: i64, name: &str) -> crate::lookup::StoreApp {
        crate::lookup::StoreApp { track_id: id, name: name.into(), bundle_id: format!("com.example.s{id}"), version: "2.0".into(), size: Some(1000), ipad_only: false, seller: String::new(), min_os: None }
    }

    /// Живой Search API: `cargo test live_search -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_search_relevance() {
        let f = crate::lookup::ReqwestFetch::new();
        for term in ["сбер", "вконтакте"] {
            let all = crate::lookup::search(&f, term).await.unwrap();
            let kept = relevant(all.clone(), term);
            println!("{term}: {} из {}", kept.len(), all.len());
            println!("  отброшено: {}", all.iter().filter(|a| !kept.contains(a)).map(|a| a.name.as_str()).collect::<Vec<_>>().join("; "));
            for a in &kept {
                println!("  {} | {} | {}", a.track_id, a.name, a.seller);
            }
        }
    }

    #[test]
    fn relevant_keeps_name_or_seller_matches_only() {
        let body = std::fs::read_to_string(format!("{}/tests/fixtures/lookup/search-synthetic.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let found = crate::lookup::parse_search_for_tests(&body);
        let ids = |term: &str| relevant(found.clone(), term).iter().map(|a| a.track_id).collect::<Vec<_>>();
        // «Посторонний» Search API тоже вернул бы — его нет ни в имени, ни в продавце
        assert_eq!(ids(" ПРИМЕР "), [1000000001, 1000000002, 1000000003]);
        assert_eq!(ids("example  llc"), [1000000001], "по продавцу, пробелы схлопнуты");
        assert_eq!(ids("пример   банк"), [1000000001]);
        let yo = crate::lookup::StoreApp { name: "Ёлка".into(), ..found[0].clone() };
        assert_eq!(relevant(vec![yo], "елк").len(), 1, "ё = е");
        assert!(relevant(found, "нет такого").is_empty());
    }

    #[test]
    fn store_taken_is_catalog_copies_screen_rows_and_source_phone() {
        let mut c = Ctx::new(vec![removed(900, "Сбер")]);
        c.saved = vec![saved(5, "/x/5.ipa")];
        c.source = Some(vec![inst("com.example.p", "1", Some(7)), inst("com.example.noid", "1", None)]);
        let got = store_taken(&c.get(), Some(&[1, 2].into()));
        assert_eq!(got, [900, 5, 7, 1, 2].into());
        // покупки ещё не загружены: что есть
        assert_eq!(store_taken(&c.get(), None), [900, 5, 7].into());
    }

    #[test]
    fn store_rows_skip_taken_and_duplicates_and_carry_owned_and_versions() {
        let mut c = Ctx::new(vec![]);
        c.owned = [(11, true), (12, false)].into();
        c.latest = [(11, "2.1".to_string())].into();
        c.target_iphone = true;
        c.installed = Some(vec![inst("com.example.s11", "1.9", Some(11))]);
        let found = vec![store_app(10, "Взято"), store_app(11, "Есть"), store_app(12, "Нет"), store_app(11, "Повтор"), store_app(13, "Новое")];
        let rows = store_rows(found, &[10].into(), &c.get());
        let got: Vec<_> = rows.iter().map(|r| (r.track_id, r.name.as_str(), r.source, r.owned, r.checked)).collect();
        assert_eq!(got, [(11, "Есть", "store", Some(true), false), (12, "Нет", "store", Some(false), false), (13, "Новое", "store", None, false)]);
        let r = &rows[0];
        assert_eq!((r.installed_version.as_deref(), r.available_version.as_deref(), r.install_state), (Some("1.9"), Some("2.1"), InstallState::UpdateAvailable));
        assert_eq!((r.size_bytes, r.removed_from_ru, r.bundle_id.as_str()), (Some(1000), false, "com.example.s11"));
        // версия из магазина, пока своей проверки не было
        assert_eq!(rows[2].available_version.as_deref(), Some("2.0"));
        let pad = store_rows(vec![crate::lookup::StoreApp { ipad_only: true, ..store_app(14, "iPad") }], &HashSet::new(), &c.get());
        assert!(pad[0].ipad_only);
    }
}
