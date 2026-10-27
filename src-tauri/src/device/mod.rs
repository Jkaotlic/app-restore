
//! Установка ipa на iPhone по USB. Вся логика (разбор ошибок, проценты, имена) живёт здесь
//! и тестируется; `idevice_impl` — тонкий адаптер к USB.
use crate::errors::AppError;
use async_trait::async_trait;
use std::path::Path;
use tokio::sync::mpsc::Sender;

pub mod idevice_impl;

#[derive(Debug, Clone, PartialEq)]
pub struct PhoneInfo {
    pub udid: String,
    pub name: String,
    pub ios_version: String,
    /// Lockdown `DeviceClass`: "iPhone", "iPad", ...
    pub device_class: String,
    /// Lockdown `ProductType`, например "iPhone15,2".
    pub product_type: String,
}

/// Приложение, которое уже стоит на телефоне (installation_proxy Browse, только чтение).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InstalledApp {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    /// App Store id (`itemId` из iTunesMetadata). Нет у корпоративных и поставленных в обход
    /// магазина приложений.
    pub item_id: Option<i64>,
    /// Apple ID покупателя из iTunesMetadata. Личные данные: в логи не пишется, наружу из Rust
    /// не отдаётся (`skip`), интерфейс получает только маску (`mask_email`).
    #[serde(skip)]
    pub purchaser: Option<String>,
}

/// Что нужно из iTunesMetadata.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItunesMeta {
    pub item_id: Option<i64>,
    /// `softwareVersionExternalIdentifier`.
    pub version_id: Option<i64>,
    /// `com.apple.iTunesStore.downloadInfo` → `accountInfo` → `AppleID`, в старом формате
    /// `apple-id` в корне. Проверено вживую 26.10.
    pub purchaser: Option<String>,
}

/// iTunesMetadata (binary plist) -> поля. Мусор и не словарь дают `None`.
pub fn parse_itunes_metadata(bytes: &[u8]) -> Option<ItunesMeta> {
    let v = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?;
    Some(itunes_meta(v.as_dictionary()?))
}

/// То же из уже разобранного словаря (Browse иногда отдаёт iTunesMetadata словарём).
pub fn itunes_meta(d: &plist::Dictionary) -> ItunesMeta {
    let int = |k: &str| d.get(k).and_then(plist_int);
    let text = |v: Option<&plist::Value>| v.and_then(plist::Value::as_string).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let new_format = d
        .get("com.apple.iTunesStore.downloadInfo")
        .and_then(plist::Value::as_dictionary)
        .and_then(|di| di.get("accountInfo"))
        .and_then(plist::Value::as_dictionary)
        .and_then(|ai| text(ai.get("AppleID")));
    ItunesMeta {
        item_id: int("itemId"),
        version_id: int("softwareVersionExternalIdentifier"),
        purchaser: new_format.or_else(|| text(d.get("apple-id"))),
    }
}

/// Целое из plist; на всякий случай и из строки с цифрами.
fn plist_int(v: &plist::Value) -> Option<i64> {
    match v {
        plist::Value::Integer(i) => i.as_signed(),
        plist::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Ответ Browse -> список приложений. Записи без bundle id пропускаются.
pub fn installed_from_browse(list: &[plist::Value]) -> Vec<InstalledApp> {
    list.iter()
        .filter_map(plist::Value::as_dictionary)
        .filter_map(|d| {
            let text = |k: &str| d.get(k).and_then(plist::Value::as_string).map(str::trim).filter(|s| !s.is_empty());
            let bundle_id = text("CFBundleIdentifier")?.to_string();
            let name = text("CFBundleDisplayName").or(text("CFBundleName")).unwrap_or(&bundle_id).to_string();
            let version = text("CFBundleShortVersionString").or(text("CFBundleVersion")).unwrap_or_default().to_string();
            let meta = match d.get("iTunesMetadata") {
                Some(plist::Value::Data(b)) => parse_itunes_metadata(b).unwrap_or_default(),
                Some(plist::Value::Dictionary(m)) => itunes_meta(m),
                _ => ItunesMeta::default(),
            };
            Some(InstalledApp { bundle_id, name, version, item_id: meta.item_id, purchaser: meta.purchaser })
        })
        .collect()
}

/// Сколько приложений должно быть у покупателя, чтобы считать его аккаунтом телефона.
pub const PHONE_ACCOUNT_MIN: usize = 3;

/// Apple ID телефона: самый частый покупатель среди установленных приложений (без учёта
/// регистра). Только если у него не меньше `PHONE_ACCOUNT_MIN` приложений и он строго впереди
/// второго; иначе `None`. Отдаётся первое встреченное написание.
pub fn phone_account(installed: &[InstalledApp]) -> Option<String> {
    let mut counts: Vec<(String, String, usize)> = vec![]; // (ключ, написание, сколько)
    for p in installed.iter().filter_map(|a| a.purchaser.as_deref()) {
        let key = normalize_id(p);
        if key.is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(k, _, _)| *k == key) {
            Some(c) => c.2 += 1,
            None => counts.push((key, p.trim().to_string(), 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.2));
    let first = counts.first()?;
    let second = counts.get(1).map_or(0, |c| c.2);
    (first.2 >= PHONE_ACCOUNT_MIN && first.2 > second).then(|| first.1.clone())
}

/// Совпадает ли Apple ID телефона с тем, в который вошли в программе.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountMatch {
    Same,
    /// `phone` — полный адрес: показывать только через `mask_email`.
    Different { phone: String },
    /// Определить не удалось: задаём вопрос.
    Unknown,
}

/// Адрес для сравнения: без регистра и пробелов.
fn normalize_id(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

pub fn account_match(phone: Option<&str>, program: &str) -> AccountMatch {
    let program = normalize_id(program);
    match phone {
        Some(p) if !program.is_empty() && !normalize_id(p).is_empty() => {
            if normalize_id(p) == program {
                AccountMatch::Same
            } else {
                AccountMatch::Different { phone: p.trim().to_string() }
            }
        }
        _ => AccountMatch::Unknown,
    }
}

/// `ow***@example.com`: первые два символа и домен. Без `@` — только два символа.
pub fn mask_email(email: &str) -> String {
    let email = email.trim();
    let (user, domain) = match email.split_once('@') {
        Some((u, d)) => (u, Some(d)),
        None => (email, None),
    };
    let head: String = user.chars().take(2).collect();
    match domain {
        Some(d) => format!("{head}***@{d}"),
        None => format!("{head}***"),
    }
}

/// Телефон по умолчанию: единственный iPhone. iPad и неоднозначность (0 или 2+ iPhone) -> `None`,
/// выбор за пользователем.
pub fn pick_default(phones: &[PhoneInfo]) -> Option<&PhoneInfo> {
    let mut it = phones.iter().filter(|p| p.device_class == "iPhone");
    match (it.next(), it.next()) {
        (Some(p), None) => Some(p),
        _ => None,
    }
}

/// Итог опроса устройств по одному. Сбойное устройство (например, недоверенный iPad) не прячет
/// остальные. Если не ответило ни одно: вопрос «Доверять», если он был хоть у одного (это
/// человек может исправить сам), иначе первая ошибка.
pub fn merge_probes(results: Vec<Result<PhoneInfo, AppError>>) -> Result<Vec<PhoneInfo>, AppError> {
    let (ok, failed): (Vec<_>, Vec<_>) = results.into_iter().partition(Result::is_ok);
    if !ok.is_empty() || failed.is_empty() {
        return Ok(ok.into_iter().filter_map(Result::ok).collect());
    }
    let errors: Vec<AppError> = failed.into_iter().filter_map(Result::err).collect();
    if errors.contains(&AppError::DeviceNotTrusted) {
        return Err(AppError::DeviceNotTrusted);
    }
    Err(errors.into_iter().next().unwrap_or(AppError::DeviceNotFound { drivers_missing: false }))
}

/// Открывает локальную ipa до обращения к телефону; ошибка диска — не вина телефона.
pub async fn open_local(ipa: &Path) -> Result<(tokio::fs::File, u64), AppError> {
    let local = |e: std::io::Error| AppError::Unknown { detail: format!("local file: {e}") };
    let f = tokio::fs::File::open(ipa).await.map_err(local)?;
    let len = f.metadata().await.map_err(local)?.len();
    Ok((f, len))
}

/// Телефон не ответил в срок (USB, usbmuxd, lockdown): понятный отказ вместо вечного ожидания.
pub const NO_ANSWER: &str = "iPhone не ответил вовремя. Отключите кабель, подключите снова и попробуйте ещё раз";
pub const CONNECT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
pub const BROWSE_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);
/// Загрузка и установка целиком: большой ipa по USB 2 идёт минуты, не десятки минут.
pub const INSTALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// `f` не дольше `limit`; дольше — `DeviceRejected(NO_ANSWER)`, шаг (`what`) — в журнал.
pub async fn deadline<T>(limit: std::time::Duration, what: &str, f: impl std::future::Future<Output = Result<T, AppError>>) -> Result<T, AppError> {
    match tokio::time::timeout(limit, f).await {
        Ok(r) => r,
        Err(_) => {
            crate::log::write("device", &format!("{what}: no answer in {} s", limit.as_secs()));
            Err(AppError::DeviceRejected { reason: NO_ANSWER.into() })
        }
    }
}

#[async_trait]
pub trait DeviceHub: Send + Sync {
    /// Все подключённые по USB устройства (iPhone и iPad); пустой список, если никого нет.
    async fn list(&self) -> Result<Vec<PhoneInfo>, AppError>;
    /// Ставит ipa. `progress`: 0..=100 сквозным счётом (загрузка 0–50, установка 50–100).
    async fn install(&self, udid: &str, ipa: &Path, progress: Sender<u8>) -> Result<(), AppError>;
    /// Что уже стоит на телефоне (только пользовательские приложения). Ничего не меняет.
    async fn installed(&self, udid: &str) -> Result<Vec<InstalledApp>, AppError>;
}

const NO_SPACE: &str = "На iPhone не хватает места";
const BAD_IOS: &str = "Эта версия приложения не подходит для iOS на телефоне";
const IPAD_ONLY: &str = "Это приложение только для iPad";

/// Сырой текст ошибки -> `AppError`. Идентификаторы взяты из исходников idevice 0.1.68:
/// `src/lib.rs` (`IdeviceError::from_device_error_type`: InvalidHostID, DeviceLocked,
/// PairingDialogResponsePending, UserDeniedPairing, PasswordProtected, DeviceNotFound),
/// `src/services/afc/errors.rs` (NoSpaceLeft = "No space left"),
/// `src/services/installation_proxy.rs` (OperationFailed(ErrorDescription)).
/// Живых негативных образцов пока нет: добавлять их в тесты по мере появления.
pub fn map_install_error(raw: &str) -> AppError {
    let l = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| l.contains(&n.to_lowercase()));
    if has(&[
        "PairingDialogResponsePending",
        "UserDeniedPairing",
        "InvalidHostID",
        "NotPaired",
        "InvalidPairRecord",
        "pairing trust dialog pending",
        "user denied pairing",
        "device does not have pairing file",
    ]) {
        AppError::DeviceNotTrusted
    } else if has(&["PasswordProtected", "PasscodeLocked", "DeviceLocked", "device is locked", "device locked"]) {
        AppError::DeviceLocked
    } else if has(&["InsufficientStorage", "NoSpaceLeft", "No space left", "not enough space", "not enough free space"]) {
        AppError::DeviceRejected { reason: NO_SPACE.into() }
    } else if has(&["DeviceFamilyNotSupported", "UIDeviceFamily", "DeviceFamily", "device family"]) {
        AppError::DeviceRejected { reason: IPAD_ONLY.into() }
    } else if has(&["DeviceOSVersionTooLow", "MinimumOSVersion", "IncorrectArchitecture"]) {
        AppError::DeviceRejected { reason: BAD_IOS.into() }
    } else if has(&[
        "ConnectionReset",
        "connection reset",
        "BrokenPipe",
        "broken pipe",
        "UnexpectedEof",
        "NotConnected",
        "DeviceNotFound",
        "device not found",
        "No such device",
    ]) {
        AppError::DeviceNotFound { drivers_missing: false }
    } else {
        AppError::DeviceRejected { reason: raw.to_string() }
    }
}

/// Устройство не прошло первоначальную настройку (lockdown `ActivationState` = "Unactivated"):
/// lockdown даёт только часть служб, installation_proxy среди них нет. Проверено вживую 27.10.
pub const NOT_ACTIVATED: &str = "Устройство не активировано: пройдите на нём первоначальную настройку до главного экрана (Wi‑Fi, Apple ID) и подключите кабель снова";
/// Тот же отказ службы, но состояние активации узнать не вышло.
pub const SERVICE_UNAVAILABLE: &str = "Устройство не пускает к списку приложений. Разблокируйте его, проверьте, что настройка завершена, и подключите кабель снова";

/// Отказ lockdown StartService: стоит спросить у устройства `ActivationState`.
pub fn needs_activation_check(raw: &str) -> bool {
    raw.contains("InvalidService")
}

/// Ошибка подключения к службе -> `AppError` с учётом `ActivationState` (если его удалось
/// прочитать в сессии lockdown). Прочие ошибки — как `map_install_error`.
pub fn map_service_error(raw: &str, activation_state: Option<&str>) -> AppError {
    if !needs_activation_check(raw) {
        return map_install_error(raw);
    }
    match activation_state {
        Some("Unactivated") => AppError::DeviceRejected { reason: NOT_ACTIVATED.into() },
        _ => AppError::DeviceRejected { reason: SERVICE_UNAVAILABLE.into() },
    }
}

/// Загрузка в телефон занимает первую половину шкалы.
pub fn upload_percent(sent: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((sent.min(total) as u128 * 50) / total as u128) as u8
}

/// Процент installation_proxy занимает вторую половину шкалы.
pub fn install_percent(p: u64) -> u8 {
    50 + (p.min(100) / 2) as u8
}

/// `<track_id>_имя.ipa` -> `PublicStaging/<track_id>.ipa`: имя на телефоне только из цифр.
pub fn staging_path(ipa: &Path) -> String {
    let digits: String = ipa
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let stem = if digits.is_empty() { "app".to_string() } else { digits };
    format!("PublicStaging/{stem}.ipa")
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Clone)]
    pub enum Behavior {
        Ok,
        /// Прогресс 10, 30, затем разрыв кабеля.
        DisconnectMidway,
        Fail(AppError),
        Panic,
    }

    pub struct FakeHub {
        pub behavior: Behavior,
        pub calls: Mutex<Vec<(String, PathBuf)>>,
    }

    impl FakeHub {
        pub fn new(behavior: Behavior) -> Self {
            Self { behavior, calls: Mutex::new(vec![]) }
        }
    }

    #[async_trait]
    impl DeviceHub for FakeHub {
        async fn list(&self) -> Result<Vec<PhoneInfo>, AppError> {
            Ok(vec![])
        }
        async fn installed(&self, _udid: &str) -> Result<Vec<InstalledApp>, AppError> {
            Ok(vec![])
        }
        async fn install(&self, udid: &str, ipa: &Path, progress: Sender<u8>) -> Result<(), AppError> {
            self.calls.lock().unwrap().push((udid.to_string(), ipa.to_path_buf()));
            match &self.behavior {
                Behavior::Ok => {
                    for p in [10u8, 60, 100] {
                        let _ = progress.send(p).await;
                    }
                    Ok(())
                }
                Behavior::DisconnectMidway => {
                    let _ = progress.send(10).await;
                    let _ = progress.send(30).await;
                    Err(map_install_error("Socket(Os { code: 32, kind: BrokenPipe, message: \"Broken pipe\" })"))
                }
                Behavior::Fail(e) => Err(e.clone()),
                Behavior::Panic => panic!("fake hub panic"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rej(s: &str) -> AppError {
        AppError::DeviceRejected { reason: s.into() }
    }

    #[test]
    fn trust_errors() {
        for raw in [
            "PairingDialogResponsePending",
            "UserDeniedPairing",
            "InvalidHostID",
            "device does not have pairing file",
            "pairing trust dialog pending",
            "user denied pairing trust",
            "NotPaired",
            "InvalidPairRecord",
        ] {
            assert_eq!(map_install_error(raw), AppError::DeviceNotTrusted, "{raw}");
        }
    }

    #[test]
    fn locked_errors() {
        for raw in ["PasswordProtected", "PasscodeLocked", "DeviceLocked", "device is locked", "device locked"] {
            assert_eq!(map_install_error(raw), AppError::DeviceLocked, "{raw}");
        }
    }

    #[test]
    fn storage_errors() {
        for raw in [
            "InsufficientStorage",
            "afc error: No space left",
            "Afc(NoSpaceLeft)",
            "installation proxy operation failed: There is not enough space",
        ] {
            assert_eq!(map_install_error(raw), rej("На iPhone не хватает места"), "{raw}");
        }
    }

    #[test]
    fn incompatible_ios_errors() {
        for raw in ["DeviceOSVersionTooLow", "MinimumOSVersion", "IncorrectArchitecture"] {
            assert_eq!(map_install_error(raw), rej("Эта версия приложения не подходит для iOS на телефоне"), "{raw}");
        }
    }

    /// installd: код `DeviceFamilyNotSupported`, в ErrorDescription упоминается UIDeviceFamily.
    /// Живого образца нет (сбой видели 26.10 на «ВКонтакте для iPad», текст не сохранился).
    #[test]
    fn ipad_only_errors() {
        for raw in [
            "DeviceFamilyNotSupported",
            "installation proxy operation failed: The UIDeviceFamily key in this app's Info.plist does not specify a value that is supported on this device.",
            "Operation failed: device family not supported",
        ] {
            assert_eq!(map_install_error(raw), rej("Это приложение только для iPad"), "{raw}");
        }
    }

    #[test]
    fn disconnect_errors() {
        for raw in [
            "device not found",
            "DeviceNotFound",
            "Socket(Os { code: 54, kind: ConnectionReset, message: \"Connection reset by peer\" })",
            "Socket(Os { code: 32, kind: BrokenPipe, message: \"Broken pipe\" })",
            "Socket(Os { code: 19, kind: Uncategorized, message: \"No such device\" })",
            "Socket(Custom { kind: UnexpectedEof, error: \"failed to fill whole buffer\" })",
        ] {
            assert_eq!(map_install_error(raw), AppError::DeviceNotFound { drivers_missing: false }, "{raw}");
        }
    }

    fn ph(class: &str, udid: &str) -> PhoneInfo {
        PhoneInfo {
            udid: udid.into(),
            name: "n".into(),
            ios_version: "17.0".into(),
            device_class: class.into(),
            product_type: format!("{class}1,1"),
        }
    }

    #[test]
    fn pick_default_only_single_iphone() {
        let (ipad, a, b) = (ph("iPad", "i"), ph("iPhone", "a"), ph("iPhone", "b"));
        assert_eq!(pick_default(&[ipad.clone(), a.clone()]).map(|p| p.udid.as_str()), Some("a"));
        assert_eq!(pick_default(std::slice::from_ref(&a)).map(|p| p.udid.as_str()), Some("a"));
        assert!(pick_default(&[ipad]).is_none());
        assert!(pick_default(&[a, b]).is_none());
        assert!(pick_default(&[]).is_none());
    }

    #[test]
    fn merge_probes_skips_failed_devices() {
        let a = ph("iPhone", "a");
        let ipad_untrusted = Err(AppError::DeviceNotTrusted);
        assert_eq!(merge_probes(vec![ipad_untrusted, Ok(a.clone())]), Ok(vec![a.clone()]));
        let other = Err(AppError::DeviceRejected { reason: "x".into() });
        assert_eq!(merge_probes(vec![Ok(a.clone()), other]), Ok(vec![a]));
        assert_eq!(merge_probes(vec![]), Ok(vec![]));
    }

    #[test]
    fn merge_probes_all_failed() {
        let nt = || Err(AppError::DeviceNotTrusted);
        assert_eq!(merge_probes(vec![nt(), nt()]), Err(AppError::DeviceNotTrusted));
        // вопрос «Доверять» — единственное, что человек может сделать сам: он важнее прочих сбоев
        let locked = Err(AppError::DeviceLocked);
        assert_eq!(merge_probes(vec![locked.clone(), nt()]), Err(AppError::DeviceNotTrusted));
        assert_eq!(merge_probes(vec![locked]), Err(AppError::DeviceLocked));
    }

    #[tokio::test]
    async fn deadline_turns_silence_into_clear_error() {
        let ms = std::time::Duration::from_millis;
        assert_eq!(deadline(ms(50), "t", async { Ok::<_, AppError>(1) }).await, Ok(1));
        let hung = deadline(ms(50), "t", std::future::pending::<Result<(), AppError>>()).await;
        assert_eq!(hung, Err(AppError::DeviceRejected { reason: NO_ANSWER.into() }));
        assert_eq!(deadline(ms(50), "t", async { Err::<(), _>(AppError::DeviceLocked) }).await, Err(AppError::DeviceLocked));
    }

    #[tokio::test]
    async fn open_local_missing_file_is_unknown() {
        let e = open_local(Path::new("/nonexistent/1_a.ipa")).await.unwrap_err();
        assert!(matches!(e, AppError::Unknown { ref detail } if detail.starts_with("local file: ")), "{e:?}");
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("1_a.ipa"), b"abc").unwrap();
        assert_eq!(open_local(&t.path().join("1_a.ipa")).await.unwrap().1, 3);
    }

    fn meta(dict: &[(&str, plist::Value)]) -> Vec<u8> {
        let d: plist::Dictionary = dict.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let mut out = vec![];
        plist::Value::Dictionary(d).to_writer_binary(&mut out).unwrap();
        out
    }

    fn im(item: Option<i64>, ver: Option<i64>, purchaser: Option<&str>) -> ItunesMeta {
        ItunesMeta { item_id: item, version_id: ver, purchaser: purchaser.map(str::to_string) }
    }

    fn dict(pairs: &[(&str, plist::Value)]) -> plist::Value {
        plist::Value::Dictionary(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    /// Новый формат: downloadInfo.accountInfo.AppleID (живой образец 26.10, адрес заменён).
    fn download_info(apple_id: &str) -> plist::Value {
        dict(&[("accountInfo", dict(&[("AppleID", plist::Value::String(apple_id.into())), ("DSPersonID", plist::Value::Integer(1.into()))]))])
    }

    #[test]
    fn itunes_metadata_binary_plist() {
        let b = meta(&[
            ("itemId", plist::Value::Integer(492224193.into())),
            ("softwareVersionExternalIdentifier", plist::Value::Integer(847801982.into())),
            ("itemName", plist::Value::String("СберБанк".into())),
        ]);
        assert_eq!(&b[..6], b"bplist");
        assert_eq!(parse_itunes_metadata(&b), Some(im(Some(492224193), Some(847801982), None)));
        let b = meta(&[("itemId", plist::Value::Integer(1.into()))]);
        assert_eq!(parse_itunes_metadata(&b), Some(im(Some(1), None, None)));
        // без itemId словарь всё равно разбирается; не словарь и мусор — нет
        assert_eq!(
            parse_itunes_metadata(&meta(&[("softwareVersionExternalIdentifier", plist::Value::Integer(5.into()))])),
            Some(im(None, Some(5), None))
        );
        let mut arr = vec![];
        plist::Value::Array(vec![]).to_writer_binary(&mut arr).unwrap();
        assert_eq!(parse_itunes_metadata(&arr), None);
        assert_eq!(parse_itunes_metadata(b"not a plist"), None);
        assert_eq!(parse_itunes_metadata(&[]), None);
    }

    #[test]
    fn itunes_metadata_purchaser_both_formats() {
        let s = |v: &str| plist::Value::String(v.into());
        // новый формат
        let b = meta(&[("itemId", plist::Value::Integer(7.into())), ("com.apple.iTunesStore.downloadInfo", download_info(" owner@example.com "))]);
        assert_eq!(parse_itunes_metadata(&b), Some(im(Some(7), None, Some("owner@example.com"))));
        // старый формат: apple-id в корне
        let b = meta(&[("itemId", plist::Value::Integer(7.into())), ("apple-id", s("old@example.com"))]);
        assert_eq!(parse_itunes_metadata(&b), Some(im(Some(7), None, Some("old@example.com"))));
        // оба: новый главнее
        let b = meta(&[("apple-id", s("old@example.com")), ("com.apple.iTunesStore.downloadInfo", download_info("new@example.com"))]);
        assert_eq!(parse_itunes_metadata(&b).unwrap().purchaser.as_deref(), Some("new@example.com"));
        // пустой AppleID -> берём apple-id; пустой везде -> None
        let b = meta(&[("apple-id", s("old@example.com")), ("com.apple.iTunesStore.downloadInfo", download_info("  "))]);
        assert_eq!(parse_itunes_metadata(&b).unwrap().purchaser.as_deref(), Some("old@example.com"));
        let b = meta(&[("apple-id", s("")), ("com.apple.iTunesStore.downloadInfo", dict(&[("accountInfo", s("не словарь"))]))]);
        assert_eq!(parse_itunes_metadata(&b).unwrap().purchaser, None);
    }

    fn buyer(p: Option<&str>) -> InstalledApp {
        InstalledApp { bundle_id: "b".into(), name: "n".into(), version: String::new(), item_id: None, purchaser: p.map(str::to_string) }
    }

    fn many(n: usize, p: Option<&str>) -> Vec<InstalledApp> {
        (0..n).map(|_| buyer(p)).collect()
    }

    #[test]
    fn phone_account_majority() {
        // живой случай «Max»: 36 владельца, 5 программы
        let apps = [many(36, Some("owner@example.com")), many(5, Some("program@example.com")), many(3, None)].concat();
        assert_eq!(phone_account(&apps).as_deref(), Some("owner@example.com"));
        // меньше трёх приложений — не решаем
        assert_eq!(phone_account(&many(2, Some("owner@example.com"))), None);
        assert_eq!(phone_account(&[]), None);
        assert_eq!(phone_account(&many(10, None)), None);
        // ничья — не решаем
        let tie = [many(4, Some("a@example.com")), many(4, Some("b@example.com"))].concat();
        assert_eq!(phone_account(&tie), None);
        // перевес на одно — решаем
        let close = [many(4, Some("a@example.com")), many(3, Some("b@example.com"))].concat();
        assert_eq!(phone_account(&close).as_deref(), Some("a@example.com"));
    }

    #[test]
    fn phone_account_ignores_case() {
        let apps = vec![
            buyer(Some("Owner@Example.com")),
            buyer(Some("owner@example.com")),
            buyer(Some("OWNER@EXAMPLE.COM")),
            buyer(Some("x@example.com")),
            buyer(Some("x@example.com")),
        ];
        // 3 против 2 только с учётом регистра; отдаётся первое написание
        assert_eq!(phone_account(&apps).as_deref(), Some("Owner@Example.com"));
    }

    #[test]
    fn account_match_cases() {
        assert_eq!(account_match(Some(" Me@Example.com "), "me@example.com"), AccountMatch::Same);
        assert_eq!(account_match(Some("me@example.com"), "ME@EXAMPLE.COM\n"), AccountMatch::Same);
        assert_eq!(
            account_match(Some("owner@example.com"), "me@example.com"),
            AccountMatch::Different { phone: "owner@example.com".into() }
        );
        assert_eq!(account_match(None, "me@example.com"), AccountMatch::Unknown);
        // программа без адреса — сравнивать не с чем
        assert_eq!(account_match(Some("owner@example.com"), "  "), AccountMatch::Unknown);
    }

    #[test]
    fn mask_email_shows_two_chars_and_domain() {
        assert_eq!(mask_email("owner2@example.com"), "ow***@example.com");
        assert_eq!(mask_email(" ab@example.com "), "ab***@example.com");
        assert_eq!(mask_email("a@example.com"), "a***@example.com");
        assert_eq!(mask_email("юзер@example.com"), "юз***@example.com");
        // не адрес: не показываем ничего лишнего
        assert_eq!(mask_email("+79990000000"), "+7***");
        assert_eq!(mask_email("@example.com"), "***@example.com");
    }

    #[test]
    fn installed_from_browse_reads_attributes() {
        let entry = |pairs: &[(&str, plist::Value)]| {
            plist::Value::Dictionary(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
        };
        let s = |v: &str| plist::Value::String(v.into());
        let list = vec![
            entry(&[
                ("CFBundleIdentifier", s("ru.sberbank.onlineiphone")),
                ("CFBundleDisplayName", s("СберБанк")),
                ("CFBundleShortVersionString", s("12.14.0")),
                ("CFBundleVersion", s("1214")),
                (
                    "iTunesMetadata",
                    plist::Value::Data(meta(&[
                        ("itemId", plist::Value::Integer(492224193.into())),
                        ("com.apple.iTunesStore.downloadInfo", download_info("owner@example.com")),
                    ])),
                ),
            ]),
            // корпоративное: без iTunesMetadata, имя из CFBundleName, версия из CFBundleVersion
            entry(&[("CFBundleIdentifier", s("com.example.corp")), ("CFBundleName", s("Corp")), ("CFBundleVersion", s("7"))]),
            // iTunesMetadata уже словарём (не Data)
            entry(&[
                ("CFBundleIdentifier", s("com.example.dict")),
                (
                    "iTunesMetadata",
                    plist::Value::Dictionary(
                        [("itemId".to_string(), plist::Value::Integer(77.into())), ("apple-id".to_string(), s("old@example.com"))].into_iter().collect(),
                    ),
                ),
            ]),
            // битые метаданные не прячут приложение
            entry(&[("CFBundleIdentifier", s("com.example.bad")), ("iTunesMetadata", plist::Value::Data(b"zz".to_vec()))]),
            entry(&[("CFBundleDisplayName", s("без bundle id"))]),
            s("не словарь"),
        ];
        assert_eq!(
            installed_from_browse(&list),
            vec![
                InstalledApp {
                    bundle_id: "ru.sberbank.onlineiphone".into(),
                    name: "СберБанк".into(),
                    version: "12.14.0".into(),
                    item_id: Some(492224193),
                    purchaser: Some("owner@example.com".into())
                },
                InstalledApp { bundle_id: "com.example.corp".into(), name: "Corp".into(), version: "7".into(), item_id: None, purchaser: None },
                InstalledApp {
                    bundle_id: "com.example.dict".into(),
                    name: "com.example.dict".into(),
                    version: String::new(),
                    item_id: Some(77),
                    purchaser: Some("old@example.com".into())
                },
                InstalledApp { bundle_id: "com.example.bad".into(), name: "com.example.bad".into(), version: String::new(), item_id: None, purchaser: None },
            ]
        );
    }

    /// Живой образец 27.10: iPad (iOS 15.8.8, ActivationState=Unactivated) — StartService
    /// installation_proxy отвечает "InvalidService", AFC при этом открывается.
    const LIVE_INVALID_SERVICE: &str = "unknown error `InvalidService` returned from device / UnknownErrorType(\"InvalidService\")";

    #[test]
    fn invalid_service_on_unactivated_device_says_finish_setup() {
        assert_eq!(map_service_error(LIVE_INVALID_SERVICE, Some("Unactivated")), rej(NOT_ACTIVATED));
    }

    #[test]
    fn invalid_service_with_unknown_activation_is_still_readable() {
        for state in [None, Some("Activated"), Some("")] {
            assert_eq!(map_service_error(LIVE_INVALID_SERVICE, state), rej(SERVICE_UNAVAILABLE), "{state:?}");
        }
    }

    #[test]
    fn other_errors_ignore_activation_state() {
        let pipe = "Socket(Os { code: 32, kind: BrokenPipe, message: \"Broken pipe\" })";
        assert_eq!(map_service_error(pipe, Some("Unactivated")), map_install_error(pipe));
        assert_eq!(map_service_error("DeviceLocked", None), AppError::DeviceLocked);
    }

    #[test]
    fn needs_activation_check_only_for_invalid_service() {
        assert!(needs_activation_check(LIVE_INVALID_SERVICE));
        assert!(!needs_activation_check("DeviceLocked"));
        assert!(!needs_activation_check("PackageInspectionFailed"));
    }

    #[test]
    fn unknown_error_passes_through() {
        assert_eq!(map_install_error("PackageInspectionFailed"), rej("PackageInspectionFailed"));
    }

    #[test]
    fn percent_scales() {
        assert_eq!(upload_percent(0, 100), 0);
        assert_eq!(upload_percent(50, 100), 25);
        assert_eq!(upload_percent(100, 100), 50);
        assert_eq!(upload_percent(500, 100), 50);
        assert_eq!(upload_percent(5, 0), 0);
        assert_eq!(install_percent(0), 50);
        assert_eq!(install_percent(50), 75);
        assert_eq!(install_percent(100), 100);
        assert_eq!(install_percent(900), 100);
    }

    #[test]
    fn staging_name_is_numeric() {
        assert_eq!(staging_path(Path::new("/x/111_com.example.t111_1.2.3.ipa")), "PublicStaging/111.ipa");
        assert_eq!(staging_path(Path::new("/x/weird ../name.ipa")), "PublicStaging/app.ipa");
    }
}
