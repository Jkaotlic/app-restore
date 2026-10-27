
//! Сверка с российским App Store через публичный iTunes Lookup.
use crate::errors::AppError;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use crate::retry::{APPLE_DELAYS, with_retry};
use std::sync::Arc;
use std::time::Duration;

pub const BATCH: usize = 150;
const LOOKUP_URL: &str = "https://itunes.apple.com/lookup";

/// Итог Lookup: какие trackId есть в магазине страны и размер (`fileSizeBytes`) тех, у кого
/// Apple его дал. Размер берётся из того же ответа, отдельных запросов ради него нет.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Presence {
    pub ids: HashSet<i64>,
    pub sizes: HashMap<i64, u64>,
}

#[async_trait]
pub trait Lookup: Send + Sync {
    async fn present_in(&self, ids: &[i64], country: &str) -> Result<Presence, AppError>;
}

/// Подаёт тело ответа по URL. Выделено, чтобы тесты работали на фикстурах без сети.
#[async_trait]
pub trait Fetch: Send + Sync {
    async fn get(&self, url: &str) -> Result<String, AppError>;
}

pub struct ReqwestFetch(reqwest::Client);

const TIMEOUT: Duration = Duration::from_secs(20);

impl ReqwestFetch {
    pub fn new() -> Self {
        // GitHub API без User-Agent отвечает 403.
        let ua = concat!("app-restore/", env!("CARGO_PKG_VERSION"));
        Self(reqwest::Client::builder().timeout(TIMEOUT).user_agent(ua).build().unwrap_or_default())
    }
}

#[async_trait]
impl Fetch for ReqwestFetch {
    async fn get(&self, url: &str) -> Result<String, AppError> {
        let resp = self.0.get(url).send().await.map_err(|_| AppError::AppleUnavailable { status: None })?;
        let status = resp.status();
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(AppError::AppleUnavailable { status: Some(status.as_u16()) });
        }
        if !status.is_success() {
            return Err(AppError::Unknown { detail: format!("lookup: HTTP {}", status.as_u16()) });
        }
        resp.text().await.map_err(|_| AppError::AppleUnavailable { status: None })
    }
}

pub struct HttpLookup {
    fetch: Arc<dyn Fetch>,
    base: String,
    delays: Vec<Duration>,
}

impl HttpLookup {
    pub fn new() -> Self {
        Self { fetch: Arc::new(ReqwestFetch::new()), base: LOOKUP_URL.into(), delays: APPLE_DELAYS.to_vec() }
    }

    #[cfg(test)]
    fn with_fetch(fetch: Arc<dyn Fetch>, base: &str) -> Self {
        Self { fetch, base: base.into(), delays: APPLE_DELAYS.to_vec() }
    }

    #[cfg(test)]
    fn with_delays(mut self, delays: Vec<Duration>) -> Self {
        self.delays = delays;
        self
    }

    fn url(&self, batch: &[i64], country: &str) -> String {
        let ids: Vec<String> = batch.iter().map(i64::to_string).collect();
        format!("{}?id={}&country={country}&entity=software", self.base, ids.join(","))
    }
}

fn parse(body: &str) -> Result<Presence, AppError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| AppError::Unknown { detail: format!("lookup: неверный JSON: {e}") })?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Unknown { detail: "lookup: нет поля results".into() })?;
    let mut p = Presence::default();
    for r in results {
        let Some(id) = r.get("trackId").and_then(Value::as_i64) else { continue };
        p.ids.insert(id);
        // Apple отдаёт размер строкой («243731456»); на всякий случай и числом
        let size = r.get("fileSizeBytes").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
        if let Some(n) = size {
            p.sizes.insert(id, n);
        }
    }
    Ok(p)
}

/// Имя и bundle id приложения из ответа Lookup; запись ищется по trackId.
fn parse_name(body: &str, track_id: i64) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(body).ok()?;
    let r = v.get("results")?.as_array()?.iter().find(|r| r.get("trackId").and_then(Value::as_i64) == Some(track_id))?;
    let name = r.get("trackName")?.as_str()?.trim();
    let bundle = r.get("bundleId").and_then(Value::as_str).unwrap_or_default();
    (!name.is_empty()).then(|| (name.to_string(), bundle.to_string()))
}

/// Имя и bundle id по trackId из Lookup без страны. Любой сбой даёт `None`: имя нужно лишь для
/// подписи строки, его отсутствие не мешает скачиванию.
pub async fn app_name(fetch: &dyn Fetch, track_id: i64) -> Option<(String, String)> {
    let body = fetch.get(&format!("{LOOKUP_URL}?id={track_id}&entity=software")).await.ok()?;
    parse_name(&body, track_id)
}

#[async_trait]
impl Lookup for HttpLookup {
    async fn present_in(&self, ids: &[i64], country: &str) -> Result<Presence, AppError> {
        let mut present = Presence::default();
        for batch in ids.chunks(BATCH) {
            let url = self.url(batch, country);
            let body = with_retry(&self.delays, || self.fetch.get(&url)).await?;
            let p = parse(&body)?;
            present.ids.extend(p.ids);
            present.sizes.extend(p.sizes);
        }
        Ok(present)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FlakyFetch {
        fails: usize,
        calls: Mutex<usize>,
    }
    #[async_trait]
    impl Fetch for FlakyFetch {
        async fn get(&self, _: &str) -> Result<String, AppError> {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            if *c <= self.fails { Err(AppError::AppleUnavailable { status: Some(503) }) } else { Ok(fixture("lookup-ru.json")) }
        }
    }

    #[tokio::test]
    async fn retries_503_then_succeeds() {
        let f = Arc::new(FlakyFetch { fails: 2, calls: Mutex::new(0) });
        let l = HttpLookup::with_fetch(f.clone(), "https://lookup.example.com/lookup").with_delays(vec![Duration::ZERO; 3]);
        assert!(l.present_in(&[686449807], "ru").await.unwrap().ids.contains(&686449807));
        assert_eq!(*f.calls.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn persistent_failure_is_error_after_four_calls() {
        let f = Arc::new(FlakyFetch { fails: 99, calls: Mutex::new(0) });
        let l = HttpLookup::with_fetch(f.clone(), "https://lookup.example.com/lookup").with_delays(vec![Duration::ZERO; 3]);
        assert_eq!(l.present_in(&[1], "ru").await, Err(AppError::AppleUnavailable { status: Some(503) }));
        assert_eq!(*f.calls.lock().unwrap(), 4);
    }

    struct FakeFetch {
        body: Result<String, AppError>,
        urls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Fetch for FakeFetch {
        async fn get(&self, url: &str) -> Result<String, AppError> {
            self.urls.lock().unwrap().push(url.into());
            self.body.clone()
        }
    }
    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/lookup/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }
    fn lookup(body: Result<String, AppError>) -> (HttpLookup, Arc<FakeFetch>) {
        let f = Arc::new(FakeFetch { body, urls: Mutex::new(vec![]) });
        (HttpLookup::with_fetch(f.clone(), "https://lookup.example.com/lookup").with_delays(vec![]), f)
    }

    #[tokio::test]
    async fn removed_app_absent_present_app_listed() {
        let (l, _) = lookup(Ok(fixture("lookup-ru.json")));
        let got = l.present_in(&[492224193, 564177498, 686449807], "ru").await.unwrap();
        assert!(got.ids.contains(&686449807));
        assert!(!got.ids.contains(&492224193));
        // размер из того же ответа: строкой у Apple
        assert_eq!(got.sizes.get(&686449807), Some(&243731456));
        assert_eq!(got.sizes.get(&492224193), None);
    }

    #[tokio::test]
    async fn three_hundred_twenty_ids_make_three_requests() {
        let (l, f) = lookup(Ok(fixture("lookup-ru.json")));
        let ids: Vec<i64> = (1..=320).collect();
        l.present_in(&ids, "ru").await.unwrap();
        assert_eq!(f.urls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn empty_ids_make_no_requests() {
        let (l, f) = lookup(Ok(fixture("lookup-ru.json")));
        assert_eq!(l.present_in(&[], "ru").await.unwrap(), Presence::default());
        assert!(f.urls.lock().unwrap().is_empty());
    }

    #[test]
    fn name_from_lookup_by_track_id() {
        assert_eq!(
            parse_name(&fixture("lookup-us.json"), 686449807),
            Some(("Telegram Messenger".into(), "ph.telegra.Telegraph".into()))
        );
        assert_eq!(parse_name(&fixture("lookup-us.json"), 1), None, "чужой trackId");
        assert_eq!(parse_name(&fixture("lookup-ru-removed.json"), 492224193), None, "пустой results");
        assert_eq!(parse_name("<html>", 1), None);
        assert_eq!(parse_name(r#"{"results":[{"trackId":5,"trackName":"  "}]}"#, 5), None, "пустое имя");
    }

    #[test]
    fn size_as_string_or_number_and_bad_size_ignored() {
        let p = parse(r#"{"results":[{"trackId":1,"fileSizeBytes":"100"},{"trackId":2,"fileSizeBytes":200},{"trackId":3,"fileSizeBytes":"x"},{"trackId":4}]}"#).unwrap();
        assert_eq!(p.ids, [1, 2, 3, 4].into());
        assert_eq!(p.sizes, [(1, 100), (2, 200)].into());
    }

    #[test]
    fn url_builder() {
        let (l, _) = lookup(Ok(String::new()));
        assert_eq!(
            l.url(&[1, 2, 3], "ru"),
            "https://lookup.example.com/lookup?id=1,2,3&country=ru&entity=software"
        );
    }

    #[tokio::test]
    async fn fetch_error_propagates_and_bad_json_is_unknown() {
        let (l, _) = lookup(Err(AppError::AppleUnavailable { status: Some(503) }));
        assert_eq!(l.present_in(&[1], "ru").await, Err(AppError::AppleUnavailable { status: Some(503) }));
        let (l, _) = lookup(Ok("<html>".into()));
        assert!(matches!(l.present_in(&[1], "ru").await, Err(AppError::Unknown { .. })));
    }
}
