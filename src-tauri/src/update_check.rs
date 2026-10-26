//! Есть ли новая версия программы: последний релиз на GitHub. Автообновления нет, только подсказка.
use crate::lookup::Fetch;

pub const RELEASES_URL: &str = "https://api.github.com/repos/Jkaotlic/app-restore/releases/latest";

/// `MAJOR.MINOR.PATCH` с необязательной `v` впереди. Предрелизы и мусор дают `None`.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut it = v.split('.').map(|p| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok()).flatten());
    let (a, b, c) = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some((a, b, c))
}

/// Тег релиза новее текущей версии. Непонятный тег — не новее.
pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse(tag), parse(current)), (Some(t), Some(c)) if t > c)
}

/// Тег нового релиза или `None`; любая ошибка сети или ответа даёт `None` молча.
pub async fn check(fetch: &dyn Fetch, current: &str) -> Option<String> {
    let body = fetch.get(RELEASES_URL).await.ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    is_newer(tag, current).then(|| tag.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::AppError;
    use async_trait::async_trait;

    #[test]
    fn newer_same_and_garbage() {
        assert!(is_newer("v0.2.0", "0.1.3"));
        assert!(!is_newer("v0.1.3", "0.1.3"));
        assert!(!is_newer("nightly", "0.1.3"));
        assert!(!is_newer("", "0.1.3"));
        assert!(!is_newer("v0.1.2", "0.1.3"));
        assert!(is_newer("0.1.10", "0.1.9"), "сравнение чисел, не строк");
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.2.0-beta.1", "0.1.3"), "предрелиз не предлагаем");
        assert!(!is_newer("v0.2", "0.1.3"));
        assert!(!is_newer("v0.2.0", "garbage"));
    }

    struct F(Result<String, AppError>);
    #[async_trait]
    impl Fetch for F {
        async fn get(&self, url: &str) -> Result<String, AppError> {
            assert_eq!(url, RELEASES_URL);
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn check_returns_tag_only_when_newer() {
        let body = |t: &str| Ok(format!(r#"{{"tag_name":"{t}","name":"x","draft":false}}"#));
        assert_eq!(check(&F(body("v0.2.0")), "0.1.0").await, Some("v0.2.0".into()));
        assert_eq!(check(&F(body("v0.1.0")), "0.1.0").await, None);
        assert_eq!(check(&F(Ok("<html>".into())), "0.1.0").await, None);
        assert_eq!(check(&F(Ok("{}".into())), "0.1.0").await, None);
        assert_eq!(check(&F(Err(AppError::AppleUnavailable { status: None })), "0.1.0").await, None);
    }
}
