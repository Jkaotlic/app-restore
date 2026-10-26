
//! Каталог приложений, удалённых из российского App Store. Они не видны в `list-purchases`,
//! поэтому права проверяются по trackId (`Session::owns`), а скачивание идёт через `download -i`.
use crate::errors::AppError;
use crate::lookup::Fetch;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/Jkaotlic/app-restore/main/catalog/ru-removed.json";

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RemovedApp {
    pub track_id: i64,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub letter: String,
    pub color: String,
    /// Откуда взят trackId; в интерфейсе не показывается.
    pub source: String,
    /// `"removed"` (по умолчанию) или `"back_in_store"`; ставит `scripts/catalog-check.py`.
    #[serde(default)]
    pub status: Option<String>,
    /// Продавец; нужен скрипту проверки, чтобы искать новое имя или id.
    #[serde(default)]
    pub seller: Option<String>,
    /// Только для iPad (UIDeviceFamily = [2]): iPhone такое не ставит.
    #[serde(default)]
    pub ipad_only: bool,
}

const BUNDLED: &str = include_str!("../../catalog/ru-removed.json");

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 1024 * 1024;

fn err(detail: String) -> AppError {
    AppError::Unknown { detail: format!("каталог удалённых приложений: {detail}") }
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Разбирает каталог и проверяет схему: id > 0 и уникальны, имя и `source` не пустые,
/// цвет `#RRGGBB`, буква 1–3 символа, `status` (если есть) — `removed` или `back_in_store`.
/// Неизвестные поля игнорируются.
pub fn load_removed(json: &str) -> Result<Vec<RemovedApp>, AppError> {
    let apps: Vec<RemovedApp> = serde_json::from_str(json).map_err(|e| err(e.to_string()))?;
    let mut seen = std::collections::HashSet::new();
    for a in &apps {
        let id = a.track_id;
        if a.track_id <= 0 {
            return Err(err(format!("{id}: track_id должен быть больше 0")));
        }
        if !seen.insert(a.track_id) {
            return Err(err(format!("{id}: повтор track_id")));
        }
        if a.name.trim().is_empty() {
            return Err(err(format!("{id}: пустое имя")));
        }
        if !valid_color(&a.color) {
            return Err(err(format!("{id}: цвет «{}» не вида #RRGGBB", a.color)));
        }
        if !(1..=3).contains(&a.letter.chars().count()) {
            return Err(err(format!("{id}: буква должна быть от 1 до 3 символов")));
        }
        if a.source.trim().is_empty() {
            return Err(err(format!("{id}: пустой source")));
        }
        if let Some(st) = &a.status && st != "removed" && st != "back_in_store" {
            return Err(err(format!("{id}: неизвестный status «{st}»")));
        }
    }
    Ok(apps)
}

/// Тело ответа годится, если оно не больше 1 МБ, проходит `load_removed` и записей в нём
/// не меньше 80 % от вшитой копии.
fn accept(body: &str, bundled_len: usize) -> Option<Vec<RemovedApp>> {
    if body.len() > MAX_BODY {
        return None;
    }
    let apps = load_removed(body).ok()?;
    (apps.len() * 5 >= bundled_len * 4).then_some(apps)
}

/// Свежий каталог из репозитория. Порядок: сеть, затем кэш в каталоге данных приложения
/// (дополненный записями вшитой копии), затем вшитая копия. Любой сбой сети, плохой ответ или слишком короткий список даёт
/// следующий источник; ошибок наружу нет.
pub async fn fetch_catalog(fetch: &dyn Fetch, cache_path: &Path) -> Vec<RemovedApp> {
    fetch_catalog_with(fetch, cache_path, FETCH_TIMEOUT).await
}

async fn fetch_catalog_with(fetch: &dyn Fetch, cache_path: &Path, timeout: Duration) -> Vec<RemovedApp> {
    let fallback = bundled();
    let n = fallback.len();
    if let Ok(Ok(body)) = tokio::time::timeout(timeout, fetch.get(CATALOG_URL)).await
        && let Some(apps) = accept(&body, n)
    {
        if let Some(dir) = cache_path.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let _ = tokio::fs::write(cache_path, &body).await;
        return apps;
    }
    if let Ok(cached) = tokio::fs::read_to_string(cache_path).await
        && let Some(mut apps) = accept(&cached, n)
    {
        // кэш главнее по полям, но записи, которые есть только во вшитой копии (её могла
        // дополнить новая версия программы), не теряем
        let known: std::collections::HashSet<i64> = apps.iter().map(|a| a.track_id).collect();
        apps.extend(fallback.into_iter().filter(|a| !known.contains(&a.track_id)));
        return apps;
    }
    fallback
}

/// Каталог, вшитый в бинарник. Файл проверяется тестами, поэтому паника здесь невозможна.
pub fn bundled() -> Vec<RemovedApp> {
    load_removed(BUNDLED).expect("catalog/ru-removed.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipad_only_is_optional() {
        let apps: Vec<RemovedApp> = serde_json::from_str(
            r##"[{"track_id":1,"name":"A","letter":"A","color":"#000000","source":"s"},
                {"track_id":2,"name":"B","letter":"B","color":"#000000","source":"s","ipad_only":true}]"##,
        )
        .unwrap();
        assert_eq!((apps[0].ipad_only, apps[1].ipad_only), (false, true));
        let vk_hd = bundled().into_iter().find(|a| a.track_id == 656485245).expect("ВКонтакте для iPad в каталоге");
        assert!(vk_hd.ipad_only);
        assert_eq!(bundled().iter().filter(|a| a.ipad_only).count(), 1);
    }

    #[test]
    fn bundled_catalog_has_mandatory_entries() {
        let c = bundled();
        let sber = c.iter().find(|a| a.track_id == 492224193).unwrap();
        assert_eq!((sber.name.as_str(), sber.letter.as_str(), sber.color.as_str()), ("СберБанк Онлайн", "С", "#21A038"));
        assert_eq!(sber.aliases, ["Сбербанк", "СБОЛ"]);
        let dk = c.iter().find(|a| a.track_id == 1660762523).unwrap();
        assert_eq!(dk.name, "ДКлик");
        assert_eq!(dk.aliases, ["Домклик", "Дом Лака", "Дом Счастья"]);
        assert!(c.iter().all(|a| !a.source.is_empty()));
    }

    #[test]
    fn real_file_parses_same_as_bundled() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../catalog/ru-removed.json");
        let c = load_removed(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(c, bundled());
    }

    #[test]
    fn bad_json_is_error_and_ids_unique() {
        assert!(matches!(load_removed("{oops"), Err(crate::errors::AppError::Unknown { .. })));
        let mut ids: Vec<_> = bundled().iter().map(|a| a.track_id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), bundled().len());
    }

    use async_trait::async_trait;
    use std::sync::Mutex;

    fn app(id: i64) -> String {
        format!(r##"{{"track_id":{id},"name":"A{id}","letter":"A","color":"#112233","source":"s"}}"##)
    }
    fn one(field_json: &str) -> String {
        format!("[{field_json}]")
    }

    #[test]
    fn rejects_duplicates_empty_name_bad_color_and_more() {
        let dup = format!("[{},{}]", app(1), app(1));
        assert!(load_removed(&dup).is_err());
        assert!(load_removed(&one(&app(1).replace("\"A1\"", "\"\""))).is_err());
        assert!(load_removed(&one(&app(1).replace("#112233", "112233"))).is_err());
        assert!(load_removed(&one(&app(1).replace("#112233", "#12345G"))).is_err());
        assert!(load_removed(&one(&app(1).replace("\"letter\":\"A\"", "\"letter\":\"\""))).is_err());
        assert!(load_removed(&one(&app(1).replace("\"letter\":\"A\"", "\"letter\":\"ABCD\""))).is_err());
        assert!(load_removed(&one(&app(1).replace("\"source\":\"s\"", "\"source\":\"\""))).is_err());
        assert!(load_removed(&one(&app(0))).is_err());
        assert!(load_removed(&one(&app(-5))).is_err());
    }

    #[test]
    fn accepts_unknown_fields_status_and_seller() {
        let j = r##"[{"track_id":1,"name":"A","letter":"Аб","color":"#aabbcc","source":"s","status":"back_in_store","seller":"X LLC","future":[1]}]"##;
        let c = load_removed(j).unwrap();
        assert_eq!(c[0].status.as_deref(), Some("back_in_store"));
        assert_eq!(c[0].seller.as_deref(), Some("X LLC"));
        let bad = j.replace("back_in_store", "gone");
        assert!(load_removed(&bad).is_err());
        assert!(load_removed(&app(1)).is_err()); // не массив
    }

    #[test]
    fn bundled_has_enough_entries() {
        assert!(bundled().len() >= 30);
    }

    enum Mode {
        Body(String),
        Err(AppError),
        Hang,
    }
    struct FakeFetch {
        mode: Mutex<Mode>,
        calls: Mutex<Vec<String>>,
    }
    impl FakeFetch {
        fn new(mode: Mode) -> Self {
            Self { mode: Mutex::new(mode), calls: Mutex::new(vec![]) }
        }
        fn set(&self, mode: Mode) {
            *self.mode.lock().unwrap() = mode;
        }
    }
    #[async_trait]
    impl Fetch for FakeFetch {
        async fn get(&self, url: &str) -> Result<String, AppError> {
            self.calls.lock().unwrap().push(url.into());
            let hang = matches!(*self.mode.lock().unwrap(), Mode::Hang);
            if hang {
                std::future::pending::<()>().await;
            }
            match &*self.mode.lock().unwrap() {
                Mode::Body(b) => Ok(b.clone()),
                Mode::Err(e) => Err(e.clone()),
                Mode::Hang => unreachable!(),
            }
        }
    }
    fn catalog_of(n: usize) -> String {
        let items: Vec<String> = (1..=n as i64).map(|i| app(1_000_000 + i)).collect();
        format!("[{}]", items.join(","))
    }
    fn cache_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("ru-removed.cache.json")
    }

    #[tokio::test]
    async fn fresh_valid_response_wins_and_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let n = bundled().len();
        let f = FakeFetch::new(Mode::Body(catalog_of(n)));
        let got = fetch_catalog(&f, &cache_in(&dir)).await;
        assert_eq!(got.len(), n);
        assert_eq!(got[0].track_id, 1_000_001);
        assert_eq!(f.calls.lock().unwrap().as_slice(), [CATALOG_URL]);
        assert!(cache_in(&dir).exists());
    }

    #[tokio::test]
    async fn bad_responses_fall_back_to_bundled() {
        let n = bundled().len();
        let cases = [
            Mode::Err(AppError::AppleUnavailable { status: Some(500) }),
            Mode::Err(AppError::Unknown { detail: "HTTP 404".into() }),
            Mode::Body("<html>not json".into()),
            Mode::Body(catalog_of(1)),
            Mode::Body(catalog_of(n * 4 / 5 - 1)),
            Mode::Body(format!("[{}]", vec![app(7); 2].join(","))),
            Mode::Body(format!("{}{}", catalog_of(n), " ".repeat(1_100_000))),
        ];
        for mode in cases {
            let dir = tempfile::tempdir().unwrap();
            let f = FakeFetch::new(mode);
            assert_eq!(fetch_catalog(&f, &cache_in(&dir)).await, bundled());
            assert!(!cache_in(&dir).exists(), "мусор не кэшируется");
        }
    }

    #[tokio::test]
    async fn threshold_is_eighty_percent_of_bundled() {
        let n = bundled().len();
        let min = (n * 4).div_ceil(5);
        let dir = tempfile::tempdir().unwrap();
        let f = FakeFetch::new(Mode::Body(catalog_of(min)));
        assert_eq!(fetch_catalog(&f, &cache_in(&dir)).await.len(), min);
    }

    #[tokio::test]
    async fn timeout_falls_back_to_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let f = FakeFetch::new(Mode::Hang);
        let got = fetch_catalog_with(&f, &cache_in(&dir), Duration::from_millis(50)).await;
        assert_eq!(got, bundled());
    }

    #[tokio::test]
    async fn cache_used_when_network_down_and_bad_cache_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let n = bundled().len();
        let f = FakeFetch::new(Mode::Body(catalog_of(n + 1)));
        fetch_catalog(&f, &cache_in(&dir)).await;
        f.set(Mode::Err(AppError::AppleUnavailable { status: None }));
        let got = fetch_catalog(&f, &cache_in(&dir)).await;
        assert_eq!(got.len(), 2 * n + 1, "кэш плюс вшитые записи, которых в нём нет");
        assert_eq!(got[0].track_id, 1_000_001);
        std::fs::write(cache_in(&dir), "garbage").unwrap();
        assert_eq!(fetch_catalog(&f, &cache_in(&dir)).await, bundled());
    }

    #[tokio::test]
    async fn cache_dir_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/deeper/c.json");
        let f = FakeFetch::new(Mode::Body(catalog_of(bundled().len())));
        fetch_catalog(&f, &p).await;
        assert!(p.exists());
    }

    #[test]
    fn bundled_has_confirmed_sellers() {
        let c = bundled();
        let seller = |id: i64| c.iter().find(|a| a.track_id == id).unwrap().seller.clone();
        assert_eq!(seller(1143031400).as_deref(), Some("Центр недвижимости от Сбербанка"));
        assert_eq!(seller(564177498).as_deref(), Some("V Kontakte OOO"));
        assert!(c.iter().filter(|a| a.seller.is_some()).count() >= 10);
    }

    #[tokio::test]
    async fn offline_cache_is_merged_with_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let mut v: Vec<serde_json::Value> = serde_json::from_str(BUNDLED).unwrap();
        let dropped = v.pop().unwrap()["track_id"].as_i64().unwrap();
        v[0]["name"] = "Из кэша".into();
        v.push(serde_json::from_str(&app(4_000_001)).unwrap());
        std::fs::write(cache_in(&dir), serde_json::to_string(&v).unwrap()).unwrap();
        let f = FakeFetch::new(Mode::Err(AppError::AppleUnavailable { status: None }));
        let got = fetch_catalog(&f, &cache_in(&dir)).await;
        assert_eq!(got[0].name, "Из кэша", "поля из кэша главнее");
        assert!(got.iter().any(|a| a.track_id == dropped), "запись только во вшитой копии не теряется");
        assert!(got.iter().any(|a| a.track_id == 4_000_001));
        assert_eq!(got.len(), bundled().len() + 1);
        let mut ids: Vec<_> = got.iter().map(|a| a.track_id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), got.len());
    }
}
