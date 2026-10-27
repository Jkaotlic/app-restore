
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum AppError {
    #[error("auth code required")]
    AuthCodeRequired,
    #[error("wrong credentials")]
    WrongCredentials,
    #[error("code rejected")]
    CodeRejected,
    #[error("not purchased")]
    NotPurchased,
    #[error("apple unavailable ({status:?})")]
    AppleUnavailable { status: Option<u16> },
    #[error("apple changed login")]
    AppleChangedLogin,
    #[error("not logged in")]
    NotLoggedIn,
    #[error("account disabled")]
    AccountDisabled,
    #[error("device not found")]
    DeviceNotFound { drivers_missing: bool },
    #[error("device not trusted")]
    DeviceNotTrusted,
    #[error("device locked")]
    DeviceLocked,
    #[error("device rejected: {reason}")]
    DeviceRejected { reason: String },
    #[error("no disk space, need {need_mb} MB")]
    NoDiskSpace { need_mb: u64 },
    /// Вставленный текст не ссылка на приложение App Store.
    #[error("bad app link")]
    BadLink,
    /// Из Apple ID не вышло имя папки в «Мои приложения» (`library::account_dir_name`).
    #[error("bad account folder")]
    BadAccountFolder,
    /// Выбранный файл не приложение iPhone (не zip с `Payload/*.app`).
    #[error("not an ipa")]
    NotIpa,
    /// Перед сменой аккаунта не вышло сохранить снимок нынешнего входа: ничего не меняли.
    #[error("save session failed")]
    SaveSessionFailed,
    /// Смена Apple ID посреди скачивания: ipatool скачивал бы уже с другого входа.
    #[error("downloads running")]
    DownloadsRunning,
    /// Очередь к ipatool не дошла: идёт скачивание, шаг со связкой ключей или очередь запросов.
    #[error("ipatool busy")]
    Busy,
    /// ipatool не ответил в срок и остановлен.
    #[error("ipatool timeout")]
    Timeout,
    /// Связка ключей не ответила в срок: macOS, скорее всего, ждёт разрешения в своём окне.
    #[error("keychain waiting")]
    KeychainWaiting,
    #[error("unknown: {detail}")]
    Unknown { detail: String },
}

impl AppError {
    pub fn user_text(&self) -> String {
        match self {
            AppError::AuthCodeRequired => "Apple прислал код на ваши устройства. Введите его, чтобы продолжить.".into(),
            AppError::WrongCredentials | AppError::CodeRejected => {
                "Apple не принял пароль или код. Проверьте и попробуйте снова.".into()
            }
            AppError::NotPurchased => "Не скачивалось с этого Apple ID".into(),
            AppError::AppleUnavailable { .. } => {
                "Серверы Apple сейчас не отвечают. Попробуйте через 10–15 минут.".into()
            }
            AppError::AppleChangedLogin => {
                "Apple изменил вход, ждём обновления. Скачанные ранее копии ставятся как обычно.".into()
            }
            AppError::NotLoggedIn => "Вы не вошли в Apple ID. Войдите снова.".into(),
            AppError::AccountDisabled => {
                "Apple заблокировал вход в этот Apple ID. Проверьте его на appleid.apple.com".into()
            }
            AppError::DeviceNotFound { drivers_missing: true } => {
                "Не вижу iPhone. Установите «Apple Devices» из Microsoft Store и подключите телефон снова".into()
            }
            AppError::DeviceNotFound { drivers_missing: false } => "Подключите iPhone кабелем".into(),
            AppError::DeviceNotTrusted => "На iPhone появился вопрос — нажмите «Доверять»".into(),
            AppError::DeviceLocked => "Разблокируйте iPhone и нажмите «Доверять»".into(),
            AppError::DeviceRejected { reason } => reason.clone(),
            AppError::NoDiskSpace { need_mb } => {
                format!("Не хватает места на компьютере: нужно ещё {need_mb} МБ.")
            }
            AppError::BadLink => "Это не ссылка на приложение. Нужна ссылка вида apps.apple.com/…/id123456789".into(),
            AppError::BadAccountFolder => "Не получилось создать папку для этого Apple ID".into(),
            AppError::NotIpa => "Это не файл приложения iPhone (.ipa)".into(),
            AppError::SaveSessionFailed => "Не получилось сохранить вход текущего Apple ID".into(),
            AppError::DownloadsRunning => "Дождитесь окончания скачивания".into(),
            AppError::Busy => "Программа занята другой операцией (скачивание или Связка ключей) — подождите немного и попробуйте снова.".into(),
            AppError::Timeout => "Apple слишком долго не отвечает. Попробуйте ещё раз.".into(),
            AppError::KeychainWaiting => "macOS ждёт разрешения на доступ к Связке ключей — найдите окно (оно может быть за окном программы), введите пароль Mac и нажмите «Разрешать всегда».".into(),
            AppError::Unknown { .. } => "Что-то пошло не так. Попробуйте ещё раз.".into(),
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, AppError::AppleUnavailable { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_verbatim() {
        let cred = "Apple не принял пароль или код. Проверьте и попробуйте снова.";
        assert_eq!(AppError::WrongCredentials.user_text(), cred);
        assert_eq!(AppError::CodeRejected.user_text(), cred);
        assert_eq!(
            AppError::AppleUnavailable { status: Some(503) }.user_text(),
            "Серверы Apple сейчас не отвечают. Попробуйте через 10–15 минут."
        );
        assert_eq!(
            AppError::AppleChangedLogin.user_text(),
            "Apple изменил вход, ждём обновления. Скачанные ранее копии ставятся как обычно."
        );
        assert_eq!(AppError::DeviceNotFound { drivers_missing: false }.user_text(), "Подключите iPhone кабелем");
        assert_eq!(
            AppError::DeviceNotFound { drivers_missing: true }.user_text(),
            "Не вижу iPhone. Установите «Apple Devices» из Microsoft Store и подключите телефон снова"
        );
        assert_eq!(AppError::DeviceNotTrusted.user_text(), "На iPhone появился вопрос — нажмите «Доверять»");
        assert_eq!(AppError::NotPurchased.user_text(), "Не скачивалось с этого Apple ID");
        assert_eq!(
            AppError::NoDiskSpace { need_mb: 512 }.user_text(),
            "Не хватает места на компьютере: нужно ещё 512 МБ."
        );
        assert_eq!(AppError::DeviceRejected { reason: "r".into() }.user_text(), "r");
        assert_eq!(AppError::Unknown { detail: "{\"x\":1}".into() }.user_text(), "Что-то пошло не так. Попробуйте ещё раз.");
        assert_eq!(AppError::NotLoggedIn.user_text(), "Вы не вошли в Apple ID. Войдите снова.");
        assert_eq!(AppError::AuthCodeRequired.user_text(), "Apple прислал код на ваши устройства. Введите его, чтобы продолжить.");
        assert_eq!(AppError::AccountDisabled.user_text(), "Apple заблокировал вход в этот Apple ID. Проверьте его на appleid.apple.com");
        // Только при первом подключении: доверенный заблокированный телефон ставит и так (проверено 26.10).
        assert_eq!(AppError::DeviceLocked.user_text(), "Разблокируйте iPhone и нажмите «Доверять»");
        assert_eq!(AppError::BadAccountFolder.user_text(), "Не получилось создать папку для этого Apple ID");
        assert_eq!(AppError::NotIpa.user_text(), "Это не файл приложения iPhone (.ipa)");
        assert_eq!(AppError::SaveSessionFailed.user_text(), "Не получилось сохранить вход текущего Apple ID");
        assert_eq!(AppError::DownloadsRunning.user_text(), "Дождитесь окончания скачивания");
        assert!(!AppError::Busy.user_text().contains("скачивает"), "Busy бывает и без скачивания");
        assert!(AppError::KeychainWaiting.user_text().contains("Разрешать всегда"));
        assert_eq!(AppError::BadLink.user_text(), "Это не ссылка на приложение. Нужна ссылка вида apps.apple.com/…/id123456789");
    }

    #[test]
    fn retryable_only_apple_unavailable() {
        assert!(AppError::AppleUnavailable { status: None }.retryable());
        assert!(!AppError::WrongCredentials.retryable());
        assert!(!AppError::DeviceLocked.retryable());
        assert!(!AppError::Unknown { detail: String::new() }.retryable());
    }
}
