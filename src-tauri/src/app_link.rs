
//! Разбор ссылки на приложение из App Store.

const SCHEMES: [&str; 4] = ["https://", "http://", "itms-apps://", "itms-appss://"];
const HOSTS: [&str; 2] = ["apps.apple.com", "itunes.apple.com"];

/// Принимает `https://apps.apple.com/<cc>/app/<slug>/id123`, `apps.apple.com/app/id123`,
/// `itms-apps://…id123` и голое число. Всё остальное даёт `None`.
pub fn parse_app_link(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().ok().filter(|&id| id > 0);
    }
    let lower = s.to_ascii_lowercase();
    let rest = SCHEMES.iter().find_map(|p| lower.strip_prefix(p)).unwrap_or(&lower);
    let rest = rest.split(['?', '#']).next()?;
    let mut parts = rest.split('/');
    if !HOSTS.contains(&parts.next()?) {
        return None;
    }
    let mut path: Vec<&str> = parts.collect();
    if path.last() == Some(&"") {
        path.pop(); // слеш в конце: …/id123/
    }
    // /<cc>/app/<slug>/idN или /app/idN: «app» стоит первым или вторым сегментом, id — последний
    if !path.iter().take(2).any(|p| *p == "app") {
        return None;
    }
    let digits = path.last()?.strip_prefix("id")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&id| id > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_forms() {
        let ok = [
            "https://apps.apple.com/ru/app/sberbank-online/id492224193",
            "https://apps.apple.com/ru/app/%D1%81%D0%B1%D0%B5%D1%80/id492224193?mt=8",
            "apps.apple.com/app/id492224193",
            "http://apps.apple.com/us/app/id492224193#reviews",
            "itms-apps://apps.apple.com/ru/app/x/id492224193",
            "itms-apps://itunes.apple.com/app/id492224193",
            "492224193",
            "  492224193\n",
            "HTTPS://Apps.Apple.com/ru/app/x/id492224193",
            "https://apps.apple.com/ru/app/x/id492224193/",
            "https://apps.apple.com/ru/app/x/id492224193/?mt=8",
        ];
        for s in ok {
            assert_eq!(parse_app_link(s), Some(492224193), "{s}");
        }
    }

    #[test]
    fn garbage_is_none() {
        let bad = [
            "",
            "привет",
            "https://example.com/ru/app/x/id492224193",
            "https://apps.apple.com/ru/developer/x/id492224193",
            "https://apps.apple.com/ru/app/x/",
            "https://apps.apple.com/ru/app/x/id1/extra",
            "https://apps.apple.com/ru/app/x/idabc",
            "https://apps.apple.com.evil.example/ru/app/x/id1",
            "-5",
            "0",
            "12 34",
            "99999999999999999999999",
            "id492224193",
        ];
        for s in bad {
            assert_eq!(parse_app_link(s), None, "{s}");
        }
    }
}
