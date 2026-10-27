
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

/// Сколько номеров берётся из одного перетаскивания или вставки: список не растёт без конца.
pub const MAX_TEXT_IDS: usize = 100;

/// Номера приложений из перетащенного или вставленного текста, по порядку и без повторов.
/// Берутся только ссылки `apps.apple.com/…/idN` и строки «Имя: N» (N — от 6 цифр, как в
/// `AppsList.txt`); голые числа и прочие адреса — нет.
pub fn ids_in_text(text: &str) -> Vec<i64> {
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let found = line
            .split(|c: char| c.is_whitespace() || "\"'<>()[]{}«»,;=".contains(c))
            .filter_map(store_link_id)
            .chain(name_line_id(line));
        for id in found {
            if out.len() == MAX_TEXT_IDS {
                return out;
            }
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    out
}

/// Номер из ссылки `apps.apple.com` (другие адреса, в том числе itunes.apple.com, — нет).
fn store_link_id(token: &str) -> Option<i64> {
    let token = token.trim_end_matches(['.', '!', '?', ':']);
    let lower = token.to_ascii_lowercase();
    let rest = SCHEMES.iter().find_map(|p| lower.strip_prefix(p)).unwrap_or(&lower);
    if !rest.starts_with("apps.apple.com/") {
        return None;
    }
    parse_app_link(token)
}

/// «Имя: N» — строка списка вида `AppsList.txt`: имя не пустое, N — только цифры, от 6.
fn name_line_id(line: &str) -> Option<i64> {
    let (name, num) = line.trim().rsplit_once(':')?;
    let num = num.trim();
    if name.trim().is_empty() || num.len() < 6 || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    num.parse().ok().filter(|&id| id > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_links_and_name_lines() {
        let text = "Сбер: https://apps.apple.com/ru/app/sberbank/id492224193?mt=8\n\
                    «https://apps.apple.com/app/id1631810724».\n\
                    ВКонтакте: 564177498\n\
                    Тинькофф : 1234567 \n\
                    (apps.apple.com/us/app/x/id987654321)\r\n\
                    URL=https://apps.apple.com/ru/app/vk/id111111111?mt=8";
        assert_eq!(ids_in_text(text), vec![492224193, 1631810724, 564177498, 1234567, 987654321, 111111111]);
    }

    #[test]
    fn text_without_repeats_in_order() {
        let text = "https://apps.apple.com/ru/app/a/id222222222 https://apps.apple.com/ru/app/b/id111111111\nA: 222222222";
        assert_eq!(ids_in_text(text), vec![222222222, 111111111]);
    }

    #[test]
    fn text_ignores_everything_else() {
        let bad = [
            "",
            "492224193",
            "просто текст 492224193",
            "Имя: 12345",
            ": 492224193",
            "Имя: 4922241930000000000000",
            "Имя: 492224193 и ещё",
            "https://example.com/ru/app/x/id492224193",
            "https://apps.apple.com.evil.example/ru/app/x/id492224193",
            "https://apps.apple.com/ru/developer/x/id492224193",
            "https://itunes.apple.com/app/id492224193",
            "Имя: 000000",
        ];
        for t in bad {
            assert_eq!(ids_in_text(t), Vec::<i64>::new(), "{t}");
        }
    }

    #[test]
    fn text_ids_are_capped() {
        let text: String = (0..250).map(|i| format!("A{i}: {}\n", 1_000_000 + i)).collect();
        let ids = ids_in_text(&text);
        assert_eq!(ids.len(), MAX_TEXT_IDS);
        assert_eq!(ids[0], 1_000_000);
    }

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
