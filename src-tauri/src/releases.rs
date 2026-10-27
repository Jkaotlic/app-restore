//! Сигнал «вышел новый выпуск, успейте»: свежий каталог с GitHub сравнивается с прежним (кэш,
//! а без него — вшитая копия). Новые доступные записи — системное уведомление (один раз на
//! trackId за установку, `notified-releases.json`) и бейдж «Новое» на плитке.
use crate::removed_catalog::RemovedApp;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

/// Сколько держится бейдж «Новое» после сигнала.
pub const BADGE_SECS: u64 = 14 * 24 * 60 * 60;
/// Время записи тихой точки отсчёта (`Signal::Baseline`): старше любого бейджа.
pub const BASELINE_AT: u64 = 0;
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

/// Итог сравнения каталогов. `Notify` — был годный прежний кэш: новые выпуски, о них говорим.
/// `Baseline` — кэша не было (первый запуск, кэш пропал или битый): тихая точка отсчёта — все
/// доступные сейчас записи запоминаются без уведомления и без бейджа, иначе новичку пришло бы
/// «Вышли новые выпуски: N» обо всём, что добавили после сборки.
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    Notify(Vec<i64>),
    Baseline(Vec<i64>),
}

pub fn signal(before: Option<&[RemovedApp]>, after: &[RemovedApp]) -> Signal {
    match before {
        Some(b) => Signal::Notify(new_releases(b, after)),
        None => Signal::Baseline(new_releases(&[], after)),
    }
}

/// Первый `refresh` после обновления программы — тихий: новая версия могла принести массовый
/// импорт старых выпусков в каталог, это не «вышли новые выпуски».
pub fn quiet_after_upgrade(sig: Signal, upgraded: bool) -> Signal {
    match sig {
        Signal::Notify(ids) if upgraded => Signal::Baseline(ids),
        other => other,
    }
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
    const TAIL_MANY: &str = "Возьмите их в App Store на iPhone, пока не убрали: программа потом сохранит копии.";
    match names {
        [] => None,
        [one] => Some((format!("Вышел новый выпуск: {one}"), TAIL.into())),
        many => {
            let shown = many[..many.len().min(NAMES_IN_NOTICE)].join(", ");
            let more = many.len().saturating_sub(NAMES_IN_NOTICE);
            let list = if more > 0 { format!("{shown} и ещё {more}.") } else { format!("{shown}.") };
            Some((format!("Вышли новые выпуски: {}", many.len()), format!("{list} {TAIL_MANY}")))
        }
    }
}

/// Файл: `{app_version, ids}`; 0.2 писала только `ids` (карту trackId → время) — читается тоже.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoreFile {
    #[serde(default)]
    app_version: Option<String>,
    ids: Notified,
}

/// Кому уже сказали — файл в папке данных; битый или чужого вида — пусто.
pub struct NotifiedStore {
    path: PathBuf,
    data: Mutex<Notified>,
    /// Версия программы, при которой последний раз сравнивали каталоги.
    version: Mutex<Option<String>>,
}

impl NotifiedStore {
    pub fn open(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).ok();
        let file = text.as_deref().and_then(|t| {
            serde_json::from_str::<StoreFile>(t).ok().or_else(|| serde_json::from_str::<Notified>(t).ok().map(|ids| StoreFile { app_version: None, ids }))
        });
        let (version, data) = file.map(|f| (f.app_version, f.ids)).unwrap_or_default();
        Self { path, data: Mutex::new(data), version: Mutex::new(version) }
    }

    /// Программа обновилась (или файла не было) с последнего сравнения каталогов.
    pub fn upgraded(&self, current: &str) -> bool {
        self.version.lock().unwrap().as_deref() != Some(current)
    }

    /// Запомнить версию, при которой сравнили каталоги.
    pub fn set_version(&self, current: &str) {
        *self.version.lock().unwrap() = Some(current.to_string());
        self.save(&self.data.lock().unwrap());
    }

    fn save(&self, ids: &Notified) {
        let file = StoreFile { app_version: self.version.lock().unwrap().clone(), ids: ids.clone() };
        let r = serde_json::to_vec(&file).map_err(|e| e.to_string()).and_then(|b| crate::accounts::write_atomic(&self.path, &b));
        if let Err(e) = r {
            crate::log::write("notified releases", &e);
        }
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
        self.save(&d);
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
    fn no_previous_cache_is_a_silent_baseline() {
        let after = vec![app(1, "А", Some("back_in_store"), false), app(2, "Б", None, false), app(3, "В", None, true), app(4, "Г", Some("removed"), true)];
        assert_eq!(signal(None, &after), Signal::Baseline(vec![1, 3]));
        assert_eq!(signal(Some(&after[1..2]), &after), Signal::Notify(vec![1, 3]));
        assert_eq!(signal(Some(&after), &after), Signal::Notify(vec![]));
        // точка отсчёта записана без бейджа, и потом о тех же записях не говорят
        let t = tempfile::tempdir().unwrap();
        let s = NotifiedStore::open(t.path().join("n.json"));
        assert_eq!(s.record(&[1, 3], BASELINE_AT), vec![1, 3]);
        assert!(s.badges(10_000_000).is_empty());
        assert!(s.record(&[1, 3], 10_000_000).is_empty());
        assert_eq!(s.record(&[1, 3, 5], 10_000_000), vec![5]);
        assert_eq!(s.badges(10_000_000), vec![5]);
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
        assert!(body.contains("Возьмите его"), "{body}");
        let (title, body) = notice(&["А".into(), "Б".into(), "В".into(), "Г".into(), "Д".into()]).unwrap();
        assert_eq!(title, "Вышли новые выпуски: 5");
        assert!(body.contains("Возьмите их") && !body.contains(" его "), "{body}");
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

    #[test]
    fn first_refresh_after_an_app_update_is_quiet() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("notified-releases.json");
        // файл 0.2: только trackId → время, версии программы нет — это обновление
        std::fs::write(&path, r#"{"10":500}"#).unwrap();
        let s = NotifiedStore::open(path.clone());
        assert!(s.upgraded("0.3.0"));
        assert_eq!(s.badges(600), vec![10], "old records are kept");
        assert_eq!(quiet_after_upgrade(Signal::Notify(vec![11, 12]), true), Signal::Baseline(vec![11, 12]));
        assert_eq!(quiet_after_upgrade(Signal::Notify(vec![11]), false), Signal::Notify(vec![11]));
        assert_eq!(quiet_after_upgrade(Signal::Baseline(vec![11]), false), Signal::Baseline(vec![11]));
        s.set_version("0.3.0");
        let again = NotifiedStore::open(path.clone());
        assert!(!again.upgraded("0.3.0"));
        assert!(again.upgraded("0.3.1"));
        assert_eq!(again.badges(600), vec![10]);
        // запись новых не теряет версию
        again.record(&[20], 700);
        assert!(!NotifiedStore::open(path.clone()).upgraded("0.3.0"));
        // нет файла — тоже «обновление» (но там и так тихая точка отсчёта)
        assert!(NotifiedStore::open(t.path().join("none.json")).upgraded("0.3.0"));
    }
}
