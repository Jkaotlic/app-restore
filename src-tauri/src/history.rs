//! «История покупок Apple» (reportaproblem.apple.com): строгая проверка того, что прислала
//! страница окна `history`. Страница чужая, поэтому каждое поле — недоверенные данные: номер —
//! только цифры (6–12), имя — без управляющих символов и не длиннее 200, всего не больше 5000
//! записей, берутся только приложения iPhone (`IOSApp`). Токены и cookie сайта сюда не приходят.
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

pub const MAX_ITEMS: usize = 5000;
pub const MAX_NAME: usize = 200;
/// Пачек больше, чем страниц у скрипта (500), с запасом: дальше — не слушаем.
pub const MAX_BATCHES: u32 = 600;
/// Не вошли за это время — окно закрывается с понятной ошибкой.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Пачки шли и перестали — Apple не отвечает.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Пачка от страницы — не больше стольких записей (скрипт режет так же); больше — отказ при разборе.
pub const MAX_BATCH: usize = 500;
/// Поле записи (номер, имя, тип) длиннее — отказ при разборе, до копирования.
pub const MAX_FIELD_BYTES: usize = 1024;

/// Запись от страницы: `{adamId, name, type, date}`; `date` не нужна и не читается.
#[derive(Debug, Clone, Deserialize)]
pub struct PageItem {
    #[serde(rename = "adamId", deserialize_with = "short")]
    pub adam_id: String,
    #[serde(default, deserialize_with = "short_opt")]
    pub name: Option<String>,
    #[serde(rename = "type", default, deserialize_with = "short_opt")]
    pub kind: Option<String>,
}

/// Строка не длиннее `MAX_FIELD_BYTES`: длина проверяется до копирования.
struct Short(String);

impl<'de> Deserialize<'de> for Short {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Short;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a string of at most {MAX_FIELD_BYTES} bytes")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Short, E> {
                if v.len() > MAX_FIELD_BYTES {
                    return Err(E::invalid_length(v.len(), &self));
                }
                Ok(Short(v.to_owned()))
            }
        }
        d.deserialize_str(V)
    }
}

fn short<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Short::deserialize(d).map(|s| s.0)
}

fn short_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<Short>::deserialize(d)?.map(|s| s.0))
}

/// Пачка `items` команды `plugin:history|items`: больше `MAX_BATCH` записей — отказ на `MAX_BATCH + 1`-й.
#[derive(Debug)]
pub struct PageBatch(pub Vec<PageItem>);

impl<'de> Deserialize<'de> for PageBatch {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = PageBatch;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a list of at most {MAX_BATCH} items")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PageBatch, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_BATCH));
                while let Some(it) = seq.next_element::<PageItem>()? {
                    if out.len() == MAX_BATCH {
                        return Err(de::Error::invalid_length(MAX_BATCH + 1, &self));
                    }
                    out.push(it);
                }
                Ok(PageBatch(out))
            }
        }
        d.deserialize_seq(V)
    }
}

/// Куда может ходить окно истории и что может открыть: только https на apple.com и его поддоменах.
pub fn allowed_url(url: &tauri::Url) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(|h| h == "apple.com" || h.ends_with(".apple.com"))
}

/// Переходы окна (WebKit спрашивает и про рамки страницы): `allowed_url` и пустые рамки
/// `about:blank`/`about:srcdoc` — у них нет своего сайта, страница Apple создаёт их сама.
pub fn allowed_navigation(url: &tauri::Url) -> bool {
    allowed_url(url) || (url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: i64,
    /// `None` — страница не дала годного имени: строка будет «Приложение {id}».
    pub name: Option<String>,
}

/// Номер приложения: только 6–12 цифр ASCII.
pub fn valid_id(s: &str) -> Option<i64> {
    if !(6..=12).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|n| *n > 0)
}

/// Без управляющих и bidi-символов, пробелы схлопнуты, не длиннее `MAX_NAME` символов.
pub fn clean_name(s: &str) -> Option<String> {
    let bidi = |c: char| matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    let kept: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control() && !bidi(*c)).collect();
    let name: String = kept.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").chars().take(MAX_NAME).collect();
    let name = name.trim_end().to_string();
    (!name.is_empty()).then_some(name)
}

/// Что собрано за один заход: без повторов, не больше `MAX_ITEMS`.
#[derive(Debug, Default)]
pub struct Collected {
    pub items: Vec<Found>,
    seen: HashSet<i64>,
    pub batches: u32,
}

impl Collected {
    /// Пачка от страницы. Чужие типы и негодные номера отбрасываются; пачка больше `MAX_ITEMS`
    /// и пачки сверх `MAX_BATCHES` не читаются вовсе. Возвращает, сколько добавлено.
    pub fn add(&mut self, batch: Vec<PageItem>) -> usize {
        self.batches += 1;
        if batch.len() > MAX_ITEMS || self.batches > MAX_BATCHES {
            return 0;
        }
        let before = self.items.len();
        for it in batch {
            if self.items.len() >= MAX_ITEMS {
                break;
            }
            if it.kind.as_deref() != Some("IOSApp") {
                continue;
            }
            let Some(id) = valid_id(&it.adam_id) else { continue };
            if self.seen.insert(id) {
                self.items.push(Found { id, name: it.name.as_deref().and_then(clean_name) });
            }
        }
        self.items.len() - before
    }

    pub fn full(&self) -> bool {
        self.items.len() >= MAX_ITEMS
    }

    /// Пачек пришло больше `MAX_BATCHES`: дальше не слушаем, заход надо закончить.
    pub fn over_limit(&self) -> bool {
        self.batches > MAX_BATCHES
    }
}

/// Только то, чего ещё нет на «Выборе» (покупки, каталог, телефон, App Store).
pub fn fresh(found: &[Found], known: &HashSet<i64>) -> Vec<Found> {
    found.iter().filter(|f| !known.contains(&f.id)).cloned().collect()
}

/// Чем закончился заход, кроме успеха.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Problem {
    /// Ответ не того формата (страница сообщила `api_changed`) или незнакомый код.
    ApiChanged,
    /// Apple не ответила на догрузку (или пачки перестали идти).
    Http,
    /// Не вошли за `LOGIN_TIMEOUT`.
    LoginTimeout,
    /// Человек закрыл окно раньше конца.
    Closed,
}

impl Problem {
    /// Код ошибки от страницы. Незнакомое — как «изменился формат».
    pub fn from_page(code: &str) -> Self {
        match code {
            "http" => Self::Http,
            _ => Self::ApiChanged,
        }
    }

    /// `found` — сколько успели собрать: при закрытом окне показываем найденное.
    pub fn text(self, found: usize) -> String {
        match self {
            Self::ApiChanged => "Apple изменила страницу истории покупок — найти приложения не вышло. Остальное работает как раньше.".into(),
            Self::Http => "Apple не ответила на запрос истории покупок. Попробуйте позже.".into(),
            Self::LoginTimeout => "Вход на reportaproblem.apple.com не завершён за 10 минут — окно закрыто. Попробуйте ещё раз.".into(),
            Self::Closed if found > 0 => "Окно закрыто раньше конца — показано то, что успели найти.".into(),
            Self::Closed => "Окно закрыто до того, как нашлась история покупок.".into(),
        }
    }
}

/// Пора ли закрыть окно по времени: `since_open` — с открытия, `since_batch` — с последней пачки
/// (`None` — пачек ещё не было, ждём входа).
pub fn timed_out(since_open: Duration, since_batch: Option<Duration>) -> Option<Problem> {
    match since_batch {
        None if since_open >= LOGIN_TIMEOUT => Some(Problem::LoginTimeout),
        Some(d) if d >= STALL_TIMEOUT => Some(Problem::Http),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, name: &str, kind: &str) -> PageItem {
        PageItem { adam_id: id.into(), name: Some(name.into()), kind: Some(kind.into()) }
    }

    /// Что отправил бы скрипт страницы по синтетической фикстуре, если бы не фильтровал типы.
    fn fixture_items() -> Vec<PageItem> {
        let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rap/purchase-search.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["purchases"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["plis"].as_array().unwrap().iter())
            .map(|pli| item(pli["adamId"].as_str().unwrap(), pli["localizedContent"]["nameForDisplay"].as_str().unwrap_or(""), pli["lineItemType"].as_str().unwrap()))
            .collect()
    }

    #[test]
    fn fixture_keeps_only_iphone_apps() {
        let mut c = Collected::default();
        assert_eq!(c.add(fixture_items()), 1);
        assert_eq!(c.items, vec![Found { id: 6000000001, name: Some("Пример: музыка".into()) }]);
    }

    #[test]
    fn page_items_deserialize_from_script_shape() {
        let v: Vec<PageItem> = serde_json::from_str(r#"[{"adamId":"6000000001","name":"A","type":"IOSApp","date":"2026-01-01T00:00:00Z"}]"#).unwrap();
        assert_eq!(v[0].adam_id, "6000000001");
        assert_eq!(v[0].kind.as_deref(), Some("IOSApp"));
    }

    #[test]
    fn ids_are_digits_six_to_twelve() {
        assert_eq!(valid_id("123456"), Some(123456));
        assert_eq!(valid_id("123456789012"), Some(123456789012));
        assert_eq!(valid_id("12345"), None);
        assert_eq!(valid_id("1234567890123"), None);
        assert_eq!(valid_id("12345a"), None);
        assert_eq!(valid_id("-123456"), None);
        assert_eq!(valid_id(" 123456"), None);
        assert_eq!(valid_id("１２３４５６"), None);
        assert_eq!(valid_id("000000"), None);
    }

    #[test]
    fn names_are_sanitized_and_capped() {
        assert_eq!(clean_name("  Сбер\u{0}Онлайн\n Банк\t"), Some("СберОнлайн Банк".into()));
        assert_eq!(clean_name("\u{202E}evil"), Some("evil".into()));
        assert_eq!(clean_name(" \u{7} "), None);
        let long = "я".repeat(300);
        assert_eq!(clean_name(&long).unwrap().chars().count(), MAX_NAME);
    }

    #[test]
    fn dedups_and_ignores_other_types_and_bad_ids() {
        let mut c = Collected::default();
        let n = c.add(vec![item("1234567", "A", "IOSApp"), item("1234567", "A2", "IOSApp"), item("7654321", "Mac", "MacApp"), item("12", "x", "IOSApp"), item("2345678", "", "IOSApp")]);
        assert_eq!(n, 2);
        assert_eq!(c.add(vec![item("1234567", "A", "IOSApp")]), 0);
        assert_eq!(c.items, vec![Found { id: 1234567, name: Some("A".into()) }, Found { id: 2345678, name: None }]);
    }

    #[test]
    fn caps_total_and_oversized_batches() {
        let mut c = Collected::default();
        let batch = |from: i64, n: i64| (from..from + n).map(|i| item(&i.to_string(), "x", "IOSApp")).collect::<Vec<_>>();
        assert_eq!(c.add(batch(1_000_000, MAX_ITEMS as i64 + 1)), 0, "oversized batch is dropped whole");
        assert_eq!(c.add(batch(1_000_000, 4000)), 4000);
        assert_eq!(c.add(batch(2_000_000, 4000)), 1000);
        assert!(c.full());
        assert_eq!(c.items.len(), MAX_ITEMS);
    }

    #[test]
    fn stops_listening_after_max_batches() {
        let mut c = Collected::default();
        for _ in 0..MAX_BATCHES {
            c.add(vec![]);
        }
        assert_eq!(c.add(vec![item("1234567", "A", "IOSApp")]), 0);
    }

    #[test]
    fn over_limit_after_max_batches() {
        let mut c = Collected::default();
        for _ in 0..MAX_BATCHES {
            c.add(vec![]);
        }
        assert!(!c.over_limit());
        c.add(vec![]);
        assert!(c.over_limit());
    }

    #[test]
    fn only_https_apple_hosts_are_allowed() {
        let ok = |s: &str| allowed_url(&s.parse().unwrap());
        assert!(ok("https://reportaproblem.apple.com/"));
        assert!(ok("https://idmsa.apple.com/appleauth/auth/signin"));
        assert!(ok("https://apple.com/"));
        assert!(!ok("http://reportaproblem.apple.com/"));
        assert!(!ok("https://apple.com.evil.example/"));
        assert!(!ok("https://evilapple.com/"));
        assert!(!ok("https://example.com/?u=apple.com"));
        assert!(!ok("javascript:alert(1)"));
        assert!(!ok("about:blank"));
        let nav = |s: &str| allowed_navigation(&s.parse().unwrap());
        assert!(nav("https://idmsa.apple.com/x"));
        assert!(nav("about:blank"));
        assert!(nav("about:srcdoc"));
        assert!(!nav("about:config"));
        assert!(!nav("https://example.com/"));
        assert!(!nav("data:text/html,hi"));
    }

    #[test]
    fn batch_over_500_is_rejected_while_parsing() {
        let one = r#"{"adamId":"6000000001","name":"A","type":"IOSApp","date":null}"#;
        let list = |n: usize| format!("[{}]", vec![one; n].join(","));
        let b: PageBatch = serde_json::from_str(&list(MAX_BATCH)).unwrap();
        assert_eq!(b.0.len(), MAX_BATCH);
        assert!(serde_json::from_str::<PageBatch>(&list(MAX_BATCH + 1)).is_err());
    }

    #[test]
    fn long_fields_are_rejected_while_parsing() {
        let name = "я".repeat(MAX_FIELD_BYTES / 2 + 1);
        let bad = format!(r#"[{{"adamId":"6000000001","name":"{name}","type":"IOSApp"}}]"#);
        assert!(serde_json::from_str::<PageBatch>(&bad).is_err());
        let id = "1".repeat(MAX_FIELD_BYTES + 1);
        let bad = format!(r#"[{{"adamId":"{id}","name":"A","type":"IOSApp"}}]"#);
        assert!(serde_json::from_str::<PageBatch>(&bad).is_err());
        let ok = format!(r#"[{{"adamId":"6000000001","name":"{}","type":"IOSApp"}}]"#, "a".repeat(MAX_FIELD_BYTES));
        assert_eq!(serde_json::from_str::<PageBatch>(&ok).unwrap().0[0].name.as_deref().map(str::len), Some(MAX_FIELD_BYTES));
    }

    #[test]
    fn fresh_drops_known() {
        let found = vec![Found { id: 1234567, name: None }, Found { id: 2345678, name: None }];
        let known: HashSet<i64> = [1234567].into();
        assert_eq!(fresh(&found, &known), vec![Found { id: 2345678, name: None }]);
    }

    #[test]
    fn page_codes_and_texts() {
        assert_eq!(Problem::from_page("http"), Problem::Http);
        assert_eq!(Problem::from_page("api_changed"), Problem::ApiChanged);
        assert_eq!(Problem::from_page("<script>"), Problem::ApiChanged);
        assert!(Problem::Closed.text(3).contains("показано"));
        assert!(!Problem::Closed.text(0).contains("показано"));
    }

    #[test]
    fn timeouts() {
        let m = |s: u64| Duration::from_secs(s);
        assert_eq!(timed_out(m(60), None), None);
        assert_eq!(timed_out(LOGIN_TIMEOUT, None), Some(Problem::LoginTimeout));
        assert_eq!(timed_out(m(3600), Some(m(10))), None);
        assert_eq!(timed_out(m(3600), Some(STALL_TIMEOUT)), Some(Problem::Http));
    }
}
