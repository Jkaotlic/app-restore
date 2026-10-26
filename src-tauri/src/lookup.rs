
//! Сверка с российским App Store через публичный iTunes Lookup.
use crate::errors::AppError;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use crate::retry::{APPLE_DELAYS, with_retry};
use std::sync::Arc;
use std::time::Duration;

pub const BATCH: usize = 150;
const LOOKUP_URL: &str = "https://itunes.apple.com/lookup";

#[async_trait]
pub trait Lookup: Send + Sync {
    async fn present_in(&self, ids: &[i64], country: &str) -> Result<HashSet<i64>, AppError>;
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

fn parse(body: &str) -> Result<HashSet<i64>, AppError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| AppError::Unknown { detail: format!("lookup: неверный JSON: {e}") })?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Unknown { detail: "lookup: нет поля results".into() })?;
    Ok(results.iter().filter_map(|r| r.get("trackId").and_then(Value::as_i64)).collect())
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
    async fn present_in(&self, ids: &[i64], country: &str) -> Result<HashSet<i64>, AppError> {
        let mut present = HashSet::new();
        for batch in ids.chunks(BATCH) {
            let url = self.url(batch, country);
            let body = with_retry(&self.delays, || self.fetch.get(&url)).await?;
            present.extend(parse(&body)?);
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
        assert!(l.present_in(&[686449807], "ru").await.unwrap().contains(&686449807));
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
        assert!(got.contains(&686449807));
        assert!(!got.contains(&492224193));
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
        assert!(l.present_in(&[], "ru").await.unwrap().is_empty());
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
