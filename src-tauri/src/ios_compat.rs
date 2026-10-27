//! Запустится ли приложение на iOS телефона: сравнение версий iOS и поиск самой новой версии
//! приложения, которая наверняка запустится. Чистая логика, без сети и телефона.
use crate::errors::AppError;
use std::cmp::Ordering;
use std::future::Future;

/// Части версии iOS числами: «15.8.8» → [15, 8, 8], «17» → [17]. Пусто или не цифры — `None`.
fn parts(v: &str) -> Option<Vec<u64>> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    v.split('.').map(|p| if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) { p.parse().ok() } else { None }).collect()
}

/// Сравнение версий iOS; недостающие части — нули («17» = «17.0.0»). `None` — не разобрать.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (mut x, mut y) = (parts(a)?, parts(b)?);
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    Some(x.cmp(&y))
}

/// iOS `have` не старше `need`? `None` — одну из версий не разобрать.
pub fn runs_on(need: &str, have: &str) -> Option<bool> {
    compare(have, need).map(|o| o != Ordering::Less)
}

/// Перед установкой: минимальная iOS копии против iOS телефона. Что-то неизвестно — ставим
/// (installation_proxy всё равно откажет, и ошибка будет понятна по `map_install_error`).
pub fn check(need: Option<&str>, have: Option<&str>) -> Result<(), AppError> {
    match (need, have) {
        (Some(n), Some(h)) if runs_on(n, h) == Some(false) => Err(AppError::IosTooOld { need: n.trim().to_string(), have: h.trim().to_string() }),
        _ => Ok(()),
    }
}

/// Даты выхода крупных версий iOS (публичный выпуск). После 18 Apple перешла к номеру года — 26.
const IOS_RELEASES: [(u64, &str); 12] = [
    (9, "2015-09-16"),
    (10, "2016-09-13"),
    (11, "2017-09-19"),
    (12, "2018-09-17"),
    (13, "2019-09-19"),
    (14, "2020-09-16"),
    (15, "2021-09-20"),
    (16, "2022-09-12"),
    (17, "2023-09-18"),
    (18, "2024-09-16"),
    (26, "2025-09-15"),
    (27, "2026-09-14"),
];

/// Выход следующей крупной iOS после версии телефона: что вышло раньше, не может требовать iOS
/// новее телефона (разве что более позднее обновление той же крупной). `None` — телефон на
/// последней известной iOS или версию не разобрать.
pub fn next_ios_release(have: &str) -> Option<(u64, &'static str)> {
    let major = *parts(have)?.first()?;
    IOS_RELEASES.iter().find(|(m, _)| *m > major).copied()
}

/// Что известно о версии приложения из `get-version-metadata`.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionMeta {
    pub display: String,
    /// `releaseDate` («2022-03-18T06:23:40Z»); `None` — Apple не дал.
    pub released: Option<String>,
}

/// Итог поиска `newest_before`.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// Индекс в списке версий и её метаданные.
    Version(usize, VersionMeta),
    /// Даже самая первая версия вышла позже: подходящей наверняка нет.
    None,
    /// Запросов не хватило или у версии нет даты: ответа нет.
    Unknown,
}

/// Самая новая версия, вышедшая до `cutoff` (дата «ГГГГ-ММ-ДД»). `ids` — от старой к новой, как
/// отдаёт `list-versions`. Делением пополам, не больше `budget` запросов `meta`. Ошибка `meta`
/// (например, Apple просит подождать — 429) — сразу наверх: следующие запросы не делаются.
pub async fn newest_before<F, Fut>(ids: &[String], cutoff: &str, budget: usize, mut meta: F) -> Result<Found, AppError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<VersionMeta, AppError>>,
{
    let (mut lo, mut hi) = (0usize, ids.len());
    let mut best: Option<(usize, VersionMeta)> = None;
    let mut used = 0;
    // искомое — в [lo, hi): всё левее lo вышло раньше cutoff, всё с hi — не раньше
    while lo < hi {
        if used == budget {
            return Ok(best.map_or(Found::Unknown, |(i, m)| Found::Version(i, m)));
        }
        let mid = lo + (hi - lo) / 2;
        used += 1;
        let m = meta(ids[mid].clone()).await?;
        let Some(date) = m.released.as_deref().map(|d| d.get(..10).unwrap_or(d)) else { return Ok(Found::Unknown) };
        if date < cutoff {
            lo = mid + 1;
            best = Some((mid, m));
        } else {
            hi = mid;
        }
    }
    Ok(match best {
        Some((i, m)) => Found::Version(i, m),
        None if lo == 0 && !ids.is_empty() => Found::None,
        None => Found::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn compares_dotted_and_short_versions() {
        assert_eq!(compare("15.8.8", "16"), Some(Ordering::Less));
        assert_eq!(compare("17", "17.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("26.0.1", "26"), Some(Ordering::Greater));
        assert_eq!(compare("16.3", "16.10"), Some(Ordering::Less));
        assert_eq!(compare(" 18.1 ", "18.1"), Some(Ordering::Equal));
        assert_eq!(compare("", "17"), None);
        assert_eq!(compare("17.x", "17"), None);
        assert_eq!(compare("17..1", "17"), None);
    }

    #[test]
    fn runs_on_compares_phone_against_minimum() {
        assert_eq!(runs_on("15.0", "15.8.8"), Some(true));
        assert_eq!(runs_on("16.3", "15.8.8"), Some(false));
        assert_eq!(runs_on("17", "17.0"), Some(true));
        assert_eq!(runs_on("26.0.1", "26"), Some(false));
        assert_eq!(runs_on("?", "17"), None);
    }

    #[test]
    fn check_refuses_only_when_both_known() {
        assert_eq!(check(Some("16.0"), Some("15.7")), Err(AppError::IosTooOld { need: "16.0".into(), have: "15.7".into() }));
        assert_eq!(check(Some("15.0"), Some("15.7")), Ok(()));
        assert_eq!(check(None, Some("15.7")), Ok(()));
        assert_eq!(check(Some("16.0"), None), Ok(()));
        assert_eq!(check(Some("бред"), Some("15.7")), Ok(()));
        assert_eq!(
            AppError::IosTooOld { need: "16.0".into(), have: "15.7".into() }.user_text(),
            "Нужна iOS 16.0, на iPhone 15.7"
        );
    }

    #[test]
    fn next_release_after_phone_ios() {
        assert_eq!(next_ios_release("15.8.8"), Some((16, "2022-09-12")));
        assert_eq!(next_ios_release("12"), Some((13, "2019-09-19")));
        assert_eq!(next_ios_release("18.7"), Some((26, "2025-09-15")));
        assert_eq!(next_ios_release("26.1"), Some((27, "2026-09-14")));
        assert_eq!(next_ios_release("8.4"), Some((9, "2015-09-16")));
        assert_eq!(next_ios_release("27.0.1"), None);
        assert_eq!(next_ios_release(""), None);
    }

    fn meta(display: &str, date: &str) -> VersionMeta {
        VersionMeta { display: display.into(), released: Some(format!("{date}T06:00:00Z")) }
    }

    /// Версии v0..v{n-1}, i-я вышла в (2015 + i)-01-01; считает запросы.
    struct Store {
        n: usize,
        asked: RefCell<Vec<String>>,
    }
    impl Store {
        fn ids(&self) -> Vec<String> {
            (0..self.n).map(|i| format!("v{i}")).collect()
        }
        async fn get(&self, id: String) -> Result<VersionMeta, AppError> {
            self.asked.borrow_mut().push(id.clone());
            let i: usize = id[1..].parse().unwrap();
            Ok(meta(&format!("{i}.0"), &format!("{}-01-01", 2015 + i)))
        }
    }

    #[tokio::test]
    async fn finds_newest_before_cutoff_in_few_requests() {
        let s = Store { n: 12, asked: RefCell::default() };
        let found = newest_before(&s.ids(), "2022-09-12", 10, |id| s.get(id)).await.unwrap();
        // 2022-01-01 — v7, v8 уже 2023
        assert_eq!(found, Found::Version(7, meta("7.0", "2022-01-01")));
        assert!(s.asked.borrow().len() <= 4, "{:?}", s.asked.borrow());
    }

    #[tokio::test]
    async fn long_history_stays_within_budget() {
        let s = Store { n: 600, asked: RefCell::default() };
        let found = newest_before(&s.ids(), "2100-01-01", 10, |id| s.get(id)).await.unwrap();
        assert!(s.asked.borrow().len() <= 10);
        // запросов на 600 версий хватает: log2(600) < 10
        assert_eq!(found, Found::Version(84, meta("84.0", "2099-01-01")));
        let s = Store { n: 600, asked: RefCell::default() };
        let found = newest_before(&s.ids(), "2100-01-01", 3, |id| s.get(id)).await.unwrap();
        assert_eq!(s.asked.borrow().len(), 3);
        assert!(matches!(found, Found::Version(..)), "лучшее из найденного: {found:?}");
    }

    #[tokio::test]
    async fn everything_newer_or_older() {
        let s = Store { n: 5, asked: RefCell::default() };
        assert_eq!(newest_before(&s.ids(), "2010-01-01", 10, |id| s.get(id)).await, Ok(Found::None));
        let s = Store { n: 5, asked: RefCell::default() };
        assert_eq!(newest_before(&s.ids(), "2030-01-01", 10, |id| s.get(id)).await, Ok(Found::Version(4, meta("4.0", "2019-01-01"))));
        assert_eq!(newest_before(&[], "2030-01-01", 10, |id| s.get(id)).await, Ok(Found::Unknown));
    }

    #[tokio::test]
    async fn apple_429_stops_the_search() {
        let n = RefCell::new(0);
        let ids: Vec<String> = (0..50).map(|i| format!("v{i}")).collect();
        let r = newest_before(&ids, "2030-01-01", 10, |_| {
            *n.borrow_mut() += 1;
            async { Err::<VersionMeta, _>(AppError::AppleUnavailable { status: Some(429) }) }
        })
        .await;
        assert_eq!(r, Err(AppError::AppleUnavailable { status: Some(429) }));
        assert_eq!(*n.borrow(), 1);
    }

    #[tokio::test]
    async fn missing_release_date_is_unknown() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let r = newest_before(&ids, "2030-01-01", 10, |_| async { Ok(VersionMeta { display: "1".into(), released: None }) }).await;
        assert_eq!(r, Ok(Found::Unknown));
    }
}
