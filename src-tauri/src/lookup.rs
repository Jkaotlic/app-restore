
//! Сверка с российским App Store через публичный iTunes Lookup.
use crate::errors::AppError;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use crate::retry::{APPLE_DELAYS, with_retry};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const BATCH: usize = 150;
const LOOKUP_URL: &str = "https://itunes.apple.com/lookup";

/// Итог Lookup: какие trackId есть в магазине страны и размер (`fileSizeBytes`) тех, у кого
/// Apple его дал. Размер берётся из того же ответа, отдельных запросов ради него нет.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Presence {
    pub ids: HashSet<i64>,
    pub sizes: HashMap<i64, u64>,
    /// `minimumOsVersion` последней версии: «Нужна iOS X» на плитке.
    pub min_os: HashMap<i64, String>,
}

/// `minimumOsVersion` записи Lookup/Search; пусто — `None`.
fn min_os_of(r: &Value) -> Option<String> {
    r.get("minimumOsVersion").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
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
        if let Some(m) = min_os_of(r) {
            p.min_os.insert(id, m);
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

/// Сколько trackId спрашиваем у Lookup за один запрос.
pub const ICON_BATCH: usize = 150;

/// Пары (trackId, https-адрес `artworkUrl100`) из ответа Lookup; записи без адреса или с не-https пропускаются.
/// `None` — это не ответ Lookup (не JSON, нет `results`: например, страница входа в Wi-Fi).
fn parse_artwork(body: &str) -> Option<Vec<(i64, String)>> {
    let v = serde_json::from_str::<Value>(body).ok()?;
    let results = v.get("results")?.as_array()?;
    Some(
        results
            .iter()
            .filter_map(|r| {
                let id = r.get("trackId")?.as_i64()?;
                let url = r.get("artworkUrl100")?.as_str()?;
                url.starts_with("https://").then(|| (id, url.to_string()))
            })
            .collect(),
    )
}

/// Итог `artwork`: найденные иконки; id, про которые ответ окончательный (иконка есть или Apple
/// ответил «нет» и без страны, и в российском магазине); `failed` — какая-то пачка не ответила.
#[derive(Debug, Default)]
pub struct Artwork {
    pub found: HashMap<i64, String>,
    pub answered: Vec<i64>,
    pub failed: bool,
}

/// Один проход пачками по `ICON_BATCH`; после первого сбоя дальше не спрашивает (общий лимит Lookup).
async fn artwork_pass(fetch: &dyn Fetch, ids: &[i64], country: Option<&str>) -> (HashMap<i64, String>, Vec<i64>, bool) {
    let mut out = HashMap::new();
    let mut answered = vec![];
    for chunk in ids.chunks(ICON_BATCH) {
        let list = chunk.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let url = match country {
            Some(c) => format!("{LOOKUP_URL}?id={list}&country={c}&entity=software"),
            None => format!("{LOOKUP_URL}?id={list}&entity=software"),
        };
        match fetch.get(&url).await.ok().as_deref().and_then(parse_artwork) {
            Some(pairs) => {
                out.extend(pairs);
                answered.extend_from_slice(chunk);
            }
            None => return (out, answered, true),
        }
    }
    (out, answered, false)
}

/// Иконки: сначала Lookup без страны (US), потом для оставшихся без иконки — `country=ru`
/// (приложения только российского магазина). Сбой — дальше не спрашиваем, ошибок наружу нет.
pub async fn artwork(fetch: &dyn Fetch, ids: &[i64]) -> Artwork {
    let (mut found, us, failed) = artwork_pass(fetch, ids, None).await;
    let rest: Vec<i64> = us.iter().copied().filter(|id| !found.contains_key(id)).collect();
    let (ru, failed) = if failed || rest.is_empty() {
        (vec![], failed)
    } else {
        let (ru_found, ru, ru_failed) = artwork_pass(fetch, &rest, Some("ru")).await;
        found.extend(ru_found);
        (ru, ru_failed)
    };
    let ru: HashSet<i64> = ru.into_iter().collect();
    let answered = us.into_iter().filter(|id| found.contains_key(id) || ru.contains(id)).collect();
    Artwork { found, answered, failed }
}

/// Сколько ждём после сбоя пачки иконок: Lookup общий с проверкой покупок и поиском.
pub const ICONS_BACKOFF: Duration = Duration::from_secs(60);

/// Можно ли сейчас спрашивать иконки у Apple: пауза после сбоя кончилась (или её не было).
pub fn icons_net_allowed(now: Instant, until: Option<Instant>) -> bool {
    until.is_none_or(|u| now >= u)
}

/// Иконки для `ids` (без повторов): из кэша, недостающие — у Lookup, если не идёт пауза после сбоя.
/// В кэш «иконки нет» — только для окончательных ответов; сбой ставит паузу `ICONS_BACKOFF`.
pub async fn cached_icons(
    fetch: &dyn Fetch,
    cache: &Mutex<HashMap<i64, Option<String>>>,
    backoff_until: &Mutex<Option<Instant>>,
    ids: &[i64],
    now: Instant,
) -> HashMap<i64, String> {
    let missing: Vec<i64> = {
        let c = cache.lock().unwrap();
        ids.iter().copied().filter(|id| !c.contains_key(id)).collect()
    };
    let allowed = icons_net_allowed(now, *backoff_until.lock().unwrap());
    if !missing.is_empty() && allowed {
        let got = artwork(fetch, &missing).await;
        if got.failed {
            *backoff_until.lock().unwrap() = Some(now + ICONS_BACKOFF);
        }
        let mut c = cache.lock().unwrap();
        for id in got.answered {
            c.insert(id, got.found.get(&id).cloned());
        }
    }
    let c = cache.lock().unwrap();
    ids.iter().filter_map(|id| Some((*id, c.get(id)?.clone()?))).collect()
}

const SEARCH_URL: &str = "https://itunes.apple.com/search";
/// Сколько результатов Search API просим за раз.
pub const SEARCH_LIMIT: usize = 50;
/// Короче — запроса нет: группы «В App Store» тоже.
pub const SEARCH_MIN: usize = 3;

/// Приложение из ответа Search API: столько, сколько нужно плитке и скачиванию по trackId.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreApp {
    pub track_id: i64,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub size: Option<u64>,
    /// В `supportedDevices` нет ни одного iPhone.
    pub ipad_only: bool,
    /// Продавец (`sellerName`): по нему тоже отбирается выдача (`logic::relevant`).
    pub seller: String,
    /// `minimumOsVersion` последней версии.
    pub min_os: Option<String>,
}

/// Значение параметра адреса: всё, кроме `A-Z a-z 0-9 - _ . ~`, — `%XX` по байтам UTF-8.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn search_url(base: &str, term: &str) -> String {
    format!("{base}?term={}&country=ru&entity=software&limit={SEARCH_LIMIT}", url_encode(term))
}

/// Разбор ответа Search API. Записи без trackId или без имени пропускаются.
fn parse_search(body: &str) -> Result<Vec<StoreApp>, AppError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| AppError::Unknown { detail: format!("search: неверный JSON: {e}") })?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Unknown { detail: "search: нет поля results".into() })?;
    let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    Ok(results
        .iter()
        .filter_map(|r| {
            let track_id = r.get("trackId").and_then(Value::as_i64)?;
            let name = text(r, "trackName");
            if name.is_empty() {
                return None;
            }
            let size = r.get("fileSizeBytes").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
            let ipad_only = r
                .get("supportedDevices")
                .and_then(Value::as_array)
                .is_some_and(|d| !d.is_empty() && !d.iter().any(|x| x.as_str().is_some_and(|s| s.starts_with("iPhone"))));
            Some(StoreApp { track_id, name, bundle_id: text(r, "bundleId"), version: text(r, "version"), size, ipad_only, seller: text(r, "sellerName"), min_os: min_os_of(r) })
        })
        .collect())
}

#[cfg(test)]
pub fn parse_search_for_tests(body: &str) -> Vec<StoreApp> {
    parse_search(body).unwrap()
}

/// Поиск в российском App Store (группа «В App Store»). Один запрос без повторов: человек ждёт
/// ответа на экране, а следующий ввод всё равно пришлёт новый. Короткий запрос — пусто без сети.
pub async fn search(fetch: &dyn Fetch, term: &str) -> Result<Vec<StoreApp>, AppError> {
    let term = term.trim();
    if term.chars().count() < SEARCH_MIN {
        return Ok(vec![]);
    }
    parse_search(&fetch.get(&search_url(SEARCH_URL, term)).await?)
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
            present.min_os.extend(p.min_os);
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
    #[test]
    fn artwork_parses_lookup() {
        let got = parse_artwork(&fixture("lookup-us.json")).unwrap();
        assert!(got.iter().any(|(id, u)| *id == 686449807 && u.starts_with("https://is1-ssl.mzstatic.com/")));
        let body = r#"{"results":[{"trackId":1},{"trackId":2,"artworkUrl100":"http://example.com/a.jpg"},{"trackId":3,"artworkUrl100":"https://example.com/a.jpg"}]}"#;
        assert_eq!(parse_artwork(body), Some(vec![(3, "https://example.com/a.jpg".to_string())]));
        // не ответ Lookup (страница входа в Wi-Fi, чужой JSON) — не «иконки нет»
        assert_eq!(parse_artwork("<html>"), None);
        assert_eq!(parse_artwork(r#"{"error":"x"}"#), None);
        assert_eq!(parse_artwork(r#"{"results":[]}"#), Some(vec![]));
    }

    /// Ответ по адресу: `fail` — ошибка для пачки с этим id; `ru` — иконки только в российском магазине.
    struct UrlFetch {
        urls: Mutex<Vec<String>>,
        fail: Option<i64>,
        ru: Vec<i64>,
        portal: bool,
    }
    impl UrlFetch {
        fn new() -> Self {
            Self { urls: Mutex::new(vec![]), fail: None, ru: vec![], portal: false }
        }
        fn ids(url: &str) -> Vec<i64> {
            let ids = url.strip_prefix(&format!("{LOOKUP_URL}?id=")).unwrap();
            ids.split('&').next().unwrap().split(',').map(|x| x.parse().unwrap()).collect()
        }
    }
    #[async_trait]
    impl Fetch for UrlFetch {
        async fn get(&self, url: &str) -> Result<String, AppError> {
            self.urls.lock().unwrap().push(url.into());
            let ids = Self::ids(url);
            if self.fail.is_some_and(|f| ids.contains(&f)) {
                return Err(AppError::AppleUnavailable { status: Some(503) });
            }
            if self.portal {
                return Ok("<html>Войдите в сеть</html>".into());
            }
            let icons: Vec<String> = if url.contains("&country=ru&") {
                ids.iter().filter(|id| self.ru.contains(id)).map(|id| format!(r#"{{"trackId":{id},"artworkUrl100":"https://example.com/{id}.jpg"}}"#)).collect()
            } else if ids.contains(&320) {
                vec![r#"{"trackId":320,"artworkUrl100":"https://example.com/a.jpg"}"#.into()]
            } else {
                vec![]
            };
            Ok(format!(r#"{{"results":[{}]}}"#, icons.join(",")))
        }
    }

    #[tokio::test]
    async fn artwork_batches_by_150() {
        let f = UrlFetch::new();
        let got = artwork(&f, &(1..=320).collect::<Vec<i64>>()).await;
        let urls = f.urls.lock().unwrap();
        let us: Vec<Vec<i64>> = urls.iter().filter(|u| !u.contains("country=")).map(|u| UrlFetch::ids(u)).collect();
        assert_eq!(us.iter().map(Vec::len).collect::<Vec<_>>(), vec![150, 150, 20]);
        assert_eq!(us.concat(), (1..=320).collect::<Vec<i64>>());
        assert!(urls.iter().all(|u| u.ends_with("&entity=software")));
        // без иконки — ещё раз в российском магазине, теми же пачками: 319 id → 150 + 150 + 19
        let ru: Vec<usize> = urls.iter().filter(|u| u.contains("&country=ru&")).map(|u| UrlFetch::ids(u).len()).collect();
        assert_eq!(ru, vec![150, 150, 19]);
        assert_eq!(got.found.get(&320).map(String::as_str), Some("https://example.com/a.jpg"));
        assert_eq!(got.answered.len(), 320);
        assert!(!got.failed);
    }

    #[tokio::test]
    async fn artwork_stops_after_the_first_failed_batch() {
        let f = UrlFetch { fail: Some(151), ..UrlFetch::new() };
        let got = artwork(&f, &(1..=320).collect::<Vec<i64>>()).await;
        let urls = f.urls.lock().unwrap();
        // 1-я пачка ответила, 2-я упала: 3-й и российского прохода нет
        assert_eq!(urls.len(), 2);
        assert!(got.failed);
        // «иконки нет» без российского ответа не окончательно: в кэш не пойдёт
        assert!(got.answered.is_empty());
    }

    #[tokio::test]
    async fn artwork_asks_the_russian_store_for_ru_only_apps() {
        let f = UrlFetch { ru: vec![7], ..UrlFetch::new() };
        let got = artwork(&f, &[7, 8]).await;
        assert_eq!(got.found.get(&7).map(String::as_str), Some("https://example.com/7.jpg"));
        assert!(!got.found.contains_key(&8));
        let mut answered = got.answered.clone();
        answered.sort();
        assert_eq!(answered, vec![7, 8]);
        let urls = f.urls.lock().unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls[1].contains("?id=7,8&country=ru&entity=software"));
    }

    #[tokio::test]
    async fn artwork_portal_page_is_not_an_answer() {
        let f = UrlFetch { portal: true, ..UrlFetch::new() };
        let got = artwork(&f, &[1, 2]).await;
        assert!(got.failed);
        assert!(got.answered.is_empty());
        assert_eq!(f.urls.lock().unwrap().len(), 1);
    }

    #[test]
    fn icons_net_allowed_after_backoff() {
        let now = Instant::now();
        assert!(icons_net_allowed(now, None));
        assert!(!icons_net_allowed(now, Some(now + Duration::from_secs(1))));
        assert!(icons_net_allowed(now + Duration::from_secs(1), Some(now + Duration::from_secs(1))));
    }

    #[tokio::test]
    async fn cached_icons_waits_after_a_failed_batch() {
        let f = UrlFetch { fail: Some(1), ..UrlFetch::new() };
        let cache = Mutex::new(HashMap::new());
        let until = Mutex::new(None);
        let t0 = Instant::now();
        assert!(cached_icons(&f, &cache, &until, &[1, 320], t0).await.is_empty());
        assert_eq!(f.urls.lock().unwrap().len(), 1);
        assert_eq!(*until.lock().unwrap(), Some(t0 + ICONS_BACKOFF));
        // в паузе — только кэш, к Apple не идём
        assert!(cached_icons(&f, &cache, &until, &[1, 320], t0 + Duration::from_secs(30)).await.is_empty());
        assert_eq!(f.urls.lock().unwrap().len(), 1);
        assert!(cache.lock().unwrap().is_empty());
        // пауза кончилась — спрашиваем снова
        cached_icons(&f, &cache, &until, &[1, 320], t0 + ICONS_BACKOFF).await;
        assert_eq!(f.urls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cached_icons_answers_known_ids_from_cache() {
        let f = UrlFetch::new();
        let cache = Mutex::new(HashMap::new());
        let until = Mutex::new(None);
        let t0 = Instant::now();
        let got = cached_icons(&f, &cache, &until, &[320, 5], t0).await;
        assert_eq!(got.len(), 1);
        let n = f.urls.lock().unwrap().len();
        // и с иконкой, и окончательное «нет» — без сети
        let again = cached_icons(&f, &cache, &until, &[320, 5], t0).await;
        assert_eq!(again, got);
        assert_eq!(f.urls.lock().unwrap().len(), n);
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
        // минимальная iOS последней версии — из того же ответа
        assert_eq!(got.min_os.get(&686449807).map(String::as_str), Some("15.0"));
        assert_eq!(got.min_os.get(&492224193), None);
    }

    #[test]
    fn search_takes_minimum_ios() {
        let got = parse_search(&fixture("search-ru-telegram.json")).unwrap();
        let insta = got.iter().find(|a| a.track_id == 389801252).unwrap();
        assert_eq!(insta.min_os.as_deref(), Some("16.3"));
        let p = parse(r#"{"results":[{"trackId":1,"minimumOsVersion":" 12.0 "},{"trackId":2,"minimumOsVersion":""},{"trackId":3}]}"#).unwrap();
        assert_eq!(p.min_os, [(1, "12.0".to_string())].into());
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

    #[test]
    fn search_parses_apps_and_skips_broken() {
        let got = parse_search(&fixture("search-synthetic.json")).unwrap();
        assert_eq!(
            got,
            vec![
                StoreApp { track_id: 1000000001, name: "Пример Банк".into(), bundle_id: "com.example.bank".into(), version: "3.2.1".into(), size: Some(104857600), ipad_only: false, seller: "Example LLC".into(), min_os: None },
                StoreApp { track_id: 1000000002, name: "Пример Почта".into(), bundle_id: "com.example.mail".into(), version: "1.0".into(), size: Some(52428800), ipad_only: false, seller: String::new(), min_os: None },
                StoreApp { track_id: 1000000003, name: "Пример для iPad".into(), bundle_id: "com.example.pad".into(), version: "2.0".into(), size: None, ipad_only: true, seller: String::new(), min_os: None },
                StoreApp { track_id: 1000000006, name: "Посторонний".into(), bundle_id: "com.example.other".into(), version: "5.0".into(), size: None, ipad_only: false, seller: "Other Inc".into(), min_os: None },
            ],
            "без trackId и без имени — пропущены"
        );
        assert!(matches!(parse_search("<html>"), Err(AppError::Unknown { .. })));
        assert!(matches!(parse_search("{}"), Err(AppError::Unknown { .. })));
    }

    #[test]
    fn search_url_encodes_term() {
        assert_eq!(
            search_url("https://search.example.com/search", "Сбер & Ко"),
            "https://search.example.com/search?term=%D0%A1%D0%B1%D0%B5%D1%80%20%26%20%D0%9A%D0%BE&country=ru&entity=software&limit=50"
        );
    }

    #[tokio::test]
    async fn search_short_term_makes_no_request_and_errors_propagate() {
        let f = FakeFetch { body: Ok(fixture("search-synthetic.json")), urls: Mutex::new(vec![]) };
        assert_eq!(search(&f, "  сб ").await, Ok(vec![]));
        assert!(f.urls.lock().unwrap().is_empty());
        let got = search(&f, " пример ").await.unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(f.urls.lock().unwrap().as_slice(), [format!("{SEARCH_URL}?term=%D0%BF%D1%80%D0%B8%D0%BC%D0%B5%D1%80&country=ru&entity=software&limit=50")]);
        let f = FakeFetch { body: Err(AppError::AppleUnavailable { status: None }), urls: Mutex::new(vec![]) };
        assert_eq!(search(&f, "пример").await, Err(AppError::AppleUnavailable { status: None }));
    }
}
