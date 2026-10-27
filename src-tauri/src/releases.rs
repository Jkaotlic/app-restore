//! Сигнал «вышел новый выпуск, успейте»: свежий каталог с GitHub сравнивается с прежним (кэш,
//! а без него — вшитая копия). Новые доступные записи — системное уведомление (один раз на
//! trackId за установку, `notified-releases.json`) и бейдж «Новое» на плитке.
use crate::removed_catalog::RemovedApp;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

/// Сколько держится бейдж «Новое» после сигнала.
pub const BADGE_SECS: u64 = 14 * 24 * 60 * 60;
/// Больше названий в одном уведомлении не перечисляется.
const NAMES_IN_NOTICE: usize = 3;

/// Запись — выпуск, который сейчас можно взять в App Store: вернулась в магазин
/// (`back_in_store`) или замаскированный выпуск, ещё не снятый (`status` не `removed`).
pub fn available(a: &RemovedApp) -> bool {
    match a.status.as_deref() {
        Some("back_in_store") => true,
        Some("removed") => false,
        _ => a.disguised,
    }
}

/// trackId записей, которые стали доступны с прежнего каталога: новые или раньше недоступные.
/// Порядок свежего каталога, без повторов.
pub fn new_releases(before: &[RemovedApp], after: &[RemovedApp]) -> Vec<i64> {
    let was: HashSet<i64> = before.iter().filter(|a| available(a)).map(|a| a.track_id).collect();
    let mut seen = HashSet::new();
    after.iter().filter(|a| available(a) && !was.contains(&a.track_id) && seen.insert(a.track_id)).map(|a| a.track_id).collect()
}

/// trackId → когда о нём впервые сказали (секунды Unix).
pub type Notified = HashMap<i64, u64>;

/// Из новых — те, о которых ещё не говорили.
pub fn to_notify(new: &[i64], notified: &Notified) -> Vec<i64> {
    let mut seen = HashSet::new();
    new.iter().copied().filter(|id| !notified.contains_key(id) && seen.insert(*id)).collect()
}

/// Плитки с бейджем «Новое»: сказали не раньше `BADGE_SECS` назад. По возрастанию.
pub fn badge_ids(notified: &Notified, now: u64) -> Vec<i64> {
    let mut v: Vec<i64> = notified.iter().filter(|(_, at)| now.saturating_sub(**at) < BADGE_SECS).map(|(id, _)| *id).collect();
    v.sort_unstable();
    v
}

/// Заголовок и текст уведомления по названиям новых выпусков (`None` — говорить не о чем).
pub fn notice(names: &[String]) -> Option<(String, String)> {
    const TAIL: &str = "Возьмите его в App Store на iPhone, пока не убрали: программа потом сохранит копию.";
    match names {
        [] => None,
        [one] => Some((format!("Вышел новый выпуск: {one}"), TAIL.into())),
        many => {
            let shown = many[..many.len().min(NAMES_IN_NOTICE)].join(", ");
            let more = many.len().saturating_sub(NAMES_IN_NOTICE);
            let list = if more > 0 { format!("{shown} и ещё {more}.") } else { format!("{shown}.") };
            Some((format!("Вышли новые выпуски: {}", many.len()), format!("{list} {TAIL}")))
        }
    }
}

/// Кому уже сказали — файл в папке данных; битый или чужого вида — пусто.
pub struct NotifiedStore {
    path: PathBuf,
    data: Mutex<Notified>,
}

impl NotifiedStore {
    pub fn open(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path, data: Mutex::new(data) }
    }

    /// Запомнить, что о `ids` сказали в `now` (о ком уже говорили — время прежнее), и сохранить.
    /// Возвращает тех, о ком раньше не говорили.
    pub fn record(&self, ids: &[i64], now: u64) -> Vec<i64> {
        let mut d = self.data.lock().unwrap();
        let fresh = to_notify(ids, &d);
        if fresh.is_empty() {
            return fresh;
        }
        d.extend(fresh.iter().map(|id| (*id, now)));
        let r = serde_json::to_vec(&*d).map_err(|e| e.to_string()).and_then(|b| crate::accounts::write_atomic(&self.path, &b));
        if let Err(e) = r {
            crate::log::write("notified releases", &e);
        }
        fresh
    }

    pub fn badges(&self, now: u64) -> Vec<i64> {
        badge_ids(&self.data.lock().unwrap(), now)
    }
}

/// Названия по trackId из каталога (неизвестный — пропускается), в порядке `ids`.
pub fn names(ids: &[i64], catalog: &[RemovedApp]) -> Vec<String> {
    let by_id: HashMap<i64, &str> = catalog.iter().map(|a| (a.track_id, a.name.as_str())).collect();
    let mut seen = HashSet::new();
    ids.iter().filter(|id| seen.insert(**id)).filter_map(|id| by_id.get(id).map(|n| n.to_string())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: i64, name: &str, status: Option<&str>, disguised: bool) -> RemovedApp {
        RemovedApp {
            track_id: id,
            name: name.into(),
            aliases: vec![],
            letter: "А".into(),
            color: "#000000".into(),
            source: "test".into(),
            status: status.map(Into::into),
            seller: None,
            ipad_only: false,
            category: None,
            disguised,
        }
    }

    #[test]
    fn available_is_back_in_store_or_live_disguised() {
        assert!(available(&app(1, "А", Some("back_in_store"), false)));
        assert!(available(&app(1, "А", Some("back_in_store"), true)));
        assert!(available(&app(1, "А", None, true)));
        assert!(!available(&app(1, "А", Some("removed"), true)));
        assert!(!available(&app(1, "А", None, false)));
        assert!(!available(&app(1, "А", Some("removed"), false)));
    }

    #[test]
    fn new_releases_only_newly_available() {
        let before = vec![
            app(1, "Был доступен", Some("back_in_store"), false),
            app(2, "Вернулся", Some("removed"), false),
            app(3, "Обычный", None, false),
            app(4, "Сняли", None, true),
        ];
        let after = vec![
            app(10, "Новый замаскированный", None, true),
            app(1, "Был доступен", Some("back_in_store"), false),
            app(2, "Вернулся", Some("back_in_store"), false),
            app(3, "Обычный", None, false),
            app(4, "Сняли", Some("removed"), true),
            app(11, "Новый, но снят", Some("removed"), true),
            app(12, "Новый обычный", None, false),
            app(13, "Новый вернулся", Some("back_in_store"), false),
        ];
        assert_eq!(new_releases(&before, &after), vec![10, 2, 13]);
        assert!(new_releases(&after, &after).is_empty());
        assert!(new_releases(&after, &[]).is_empty());
    }

    #[test]
    fn to_notify_skips_already_told_and_repeats() {
        let told: Notified = [(2, 100)].into();
        assert_eq!(to_notify(&[10, 2, 13, 10], &told), vec![10, 13]);
        assert!(to_notify(&[2], &told).is_empty());
    }

    #[test]
    fn badges_last_two_weeks() {
        let now = 10_000_000;
        let told: Notified = [(3, now - BADGE_SECS), (1, now - BADGE_SECS + 1), (2, now), (4, now + 50)].into();
        assert_eq!(badge_ids(&told, now), vec![1, 2, 4]);
    }

    #[test]
    fn notice_names_up_to_three() {
        assert_eq!(notice(&[]), None);
        let (title, body) = notice(&["СБОЛ".into()]).unwrap();
        assert_eq!(title, "Вышел новый выпуск: СБОЛ");
        assert!(body.contains("App Store"), "{body}");
        let (title, body) = notice(&["А".into(), "Б".into(), "В".into(), "Г".into(), "Д".into()]).unwrap();
        assert_eq!(title, "Вышли новые выпуски: 5");
        assert!(body.starts_with("А, Б, В и ещё 2."), "{body}");
        let (_, body) = notice(&["А".into(), "Б".into()]).unwrap();
        assert!(body.starts_with("А, Б."), "{body}");
    }

    #[test]
    fn names_from_catalog_in_order() {
        let cat = vec![app(1, "А", None, false), app(2, "Б", None, true)];
        assert_eq!(names(&[2, 9, 1, 2], &cat), vec!["Б".to_string(), "А".to_string()]);
    }

    #[test]
    fn store_remembers_once_per_id_across_restarts() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("notified-releases.json");
        let s = NotifiedStore::open(path.clone());
        assert_eq!(s.record(&[10, 13], 500), vec![10, 13]);
        assert_eq!(s.record(&[13, 14], 600), vec![14]);
        let again = NotifiedStore::open(path.clone());
        assert!(again.record(&[10, 13, 14], 700).is_empty());
        assert_eq!(again.badges(700), vec![10, 13, 14]);
        std::fs::write(&path, "{битый").unwrap();
        assert_eq!(NotifiedStore::open(path).record(&[10], 800), vec![10]);
    }
}
