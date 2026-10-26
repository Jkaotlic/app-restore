
//! Ключ для хранилища ipatool. macOS: ipatool сам пишет в Keychain, ключ не нужен.
//! Windows: ключ лежит в Credential Manager (сервис `app-restore`, пользователь `ipatool`).
use crate::errors::AppError;

#[cfg(not(windows))]
pub fn keychain_passphrase() -> Result<Option<String>, AppError> {
    Ok(None)
}

/// Не проверено на живой Windows (собрано и запущено только на macOS).
#[cfg(windows)]
pub fn keychain_passphrase() -> Result<Option<String>, AppError> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let err = |e: keyring::Error| AppError::Unknown { detail: format!("Credential Manager: {e}") };
    let entry = keyring::Entry::new("app-restore", "ipatool").map_err(err)?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => {
            let mut raw = [0u8; 32];
            getrandom::fill(&mut raw).map_err(|e| AppError::Unknown { detail: format!("random: {e}") })?;
            let pass = STANDARD.encode(raw);
            entry.set_password(&pass).map_err(err)?;
            Ok(Some(pass))
        }
        Err(e) => Err(err(e)),
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn macos_has_no_passphrase() {
        assert_eq!(super::keychain_passphrase(), Ok(None));
    }
}
