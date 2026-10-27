
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
/// Берутся ссылки `apps.apple.com/…/idN` и строки «Имя: N» (N — 6–10 цифр, как в
/// `AppsList.txt`). Строки «Имя: N» — только если текст похож на список: строк со ссылкой или
/// «Имя: N» больше половины непустых (иначе «Заказ: 123456» в письме дал бы номер). Голые
/// числа и прочие адреса — нет.
pub fn ids_in_text(text: &str) -> Vec<i64> {
    let lines: Vec<(Vec<i64>, Option<i64>)> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| (l.split(|c: char| c.is_whitespace() || "\"'<>()[]{}«»,;=".contains(c)).filter_map(store_link_id).collect(), name_line_id(l)))
        .collect();
    let listy = lines.iter().filter(|(links, name)| !links.is_empty() || name.is_some()).count();
    let list_shaped = listy * 2 > lines.len();
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for (links, name) in lines {
        for id in links.into_iter().chain(name.filter(|_| list_shaped)) {
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

/// Номер из ссылки `apps.apple.com` (другие адреса, в том числе itunes.apple.com, — нет). Ссылка
/// может быть приклеена к слову («Ссылка:https://…»), но не продолжать имя хоста или путь
/// (`evilapps.apple.com`, `x.apps.apple.com`, `…/apps.apple.com`).
fn store_link_id(token: &str) -> Option<i64> {
    let token = token.trim_end_matches(['.', '!', '?', ':']);
    let lower = token.to_ascii_lowercase();
    let at = lower.find("apps.apple.com/")?;
    let start = SCHEMES.iter().find(|p| lower[..at].ends_with(*p)).map_or(at, |p| at - p.len());
    let glued = lower[..start].bytes().last().is_some_and(|b| b.is_ascii_alphanumeric() || b".-_@/%".contains(&b));
    if glued {
        return None;
    }
    // ASCII в нижнем регистре: границы байтов те же, что в исходной строке
    parse_app_link(&token[start..])
}

/// «Имя: N» — строка списка вида `AppsList.txt`: имя не пустое, N — только цифры, 6–10
/// (11 — уже телефон, 12 — ИНН).
fn name_line_id(line: &str) -> Option<i64> {
    let (name, num) = line.trim().rsplit_once(':')?;
    let num = num.trim();
    if name.trim().is_empty() || !(6..=10).contains(&num.len()) || !num.bytes().all(|b| b.is_ascii_digit()) {
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
    fn name_lines_are_six_to_ten_digits() {
        assert_eq!(ids_in_text("А: 123456\nБ: 1234567890"), vec![123456, 1234567890]);
        assert!(ids_in_text("А: 12345678901\nБ: 98765432109").is_empty(), "11 цифр — телефон");
        assert!(ids_in_text("Телефон: 89161234567").is_empty());
        assert!(ids_in_text("ИНН: 770708389312").is_empty(), "12 цифр — ИНН человека");
    }

    #[test]
    fn name_lines_only_in_list_shaped_text() {
        // одна строка «Имя: N» среди прозы — не список
        let prose = "Здравствуйте!\nВаш заказ оформлен.\nЗаказ: 123456\nСпасибо за покупку";
        assert!(ids_in_text(prose).is_empty());
        let inn = "Реквизиты организации\nИНН: 7707083893\nКПП: 773601001\nАдрес: Москва\nБанк: Сбер";
        assert!(ids_in_text(inn).is_empty(), "две строки из пяти — меньше половины");
        let amount = "Счёт\nСумма: 1500000\nОплатить до пятницы";
        assert!(ids_in_text(amount).is_empty());
        // одна строка, и она вся — «Имя: N»
        assert_eq!(ids_in_text("Сбер: 492224193"), vec![492224193]);
        // ссылка в прозе берётся всегда
        assert_eq!(ids_in_text("Заказ: 123456\nвот https://apps.apple.com/ru/app/x/id492224193\nи ещё текст\nи подпись\nс уважением"), vec![492224193]);
    }

    #[test]
    fn link_glued_after_colon_or_word() {
        assert_eq!(ids_in_text("Ссылка:https://apps.apple.com/ru/app/x/id492224193"), vec![492224193]);
        assert_eq!(ids_in_text("тут:apps.apple.com/app/id1631810724"), vec![1631810724]);
        assert!(ids_in_text("evilapps.apple.com/app/id492224193").is_empty());
        assert!(ids_in_text("x.apps.apple.com/app/id492224193").is_empty());
        assert!(ids_in_text("https://evil.example/apps.apple.com/app/id492224193").is_empty());
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
