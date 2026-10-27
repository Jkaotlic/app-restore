
//! Список «что вернуть»: каталог удалённых из магазина приложений, покупки,
//! пометка «удалено из российского App Store» и сохранённая копия из «Мои приложения».
use crate::device::InstalledApp;
use crate::ipatool::output::OwnedApp;
use crate::removed_catalog::RemovedApp;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct SavedIpa {
    /// `None` у файлов со старым именем `<bundle>_<версия>.ipa`.
    pub track_id: Option<i64>,
    pub bundle_id: String,
    /// Название из ipa (`itemName`, иначе `CFBundleDisplayName`); `None` — не прочитать.
    pub name: Option<String>,
    pub version: String,
    /// `UIDeviceFamily` из Info.plist: 1 — iPhone, 2 — iPad; пусто, если неизвестно.
    pub device_family: Vec<u8>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSource {
    /// Запись каталога удалённых приложений.
    Removed,
    Purchase,
}

/// Что с приложением на выбранном телефоне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    /// Не стоит, или телефон не выбран.
    NotInstalled,
    Installed,
    /// Доступная версия новее установленной.
    UpdateAvailable,
    /// Стоит, но версии не сравнить (например, «50800» и «5.8.0»).
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppRow {
    /// У записей каталога `bundle_id` пустой: он известен только после скачивания.
    pub app: OwnedApp,
    pub removed_from_ru: bool,
    pub saved_copy: Option<PathBuf>,
    /// Права: у записей каталога `None`, пока `list-versions` не проверен; у покупок всегда
    /// `Some(true)` — `list-purchases` их уже доказал.
    pub owned: Option<bool>,
    pub source: RowSource,
    /// Буква и цвет значка; только у записей каталога.
    pub letter: Option<String>,
    pub color: Option<String>,
    /// Версия на телефоне; `None`, если не стоит или телефон не выбран.
    pub installed_version: Option<String>,
    /// Что поставится: у покупок версия из `list-purchases`, у записей каталога сохранённая копия
    /// (её и поставит `plan_start`), иначе последняя версия у Apple.
    pub available_version: Option<String>,
    pub install_state: InstallState,
    /// Только для iPad: флаг каталога или UIDeviceFamily сохранённой копии.
    pub ipad_only: bool,
    /// Категория записи каталога (`RemovedApp::category`); у покупок `None`.
    pub category: Option<&'static str>,
    /// Другие названия записи каталога — для поиска.
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    pub rows: Vec<AppRow>,
    /// Сверка не удалась: на экране выбора одна строка «Не удалось проверить российский App Store».
    pub ru_check_failed: bool,
}

/// Копия ищется по trackId, при его отсутствии у файла (старое имя) по bundle id.
/// Если версий несколько, берём самую свежую по времени изменения файла.
pub(crate) fn find_saved(saved: &[SavedIpa], track_id: i64, bundle_id: &str) -> Option<PathBuf> {
    let newest = |it: Vec<&SavedIpa>| {
        it.into_iter()
            .max_by_key(|s| std::fs::metadata(&s.path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH))
            .map(|s| s.path.clone())
    };
    newest(saved.iter().filter(|s| s.track_id == Some(track_id)).collect()).or_else(|| {
        if bundle_id.is_empty() {
            return None;
        }
        newest(saved.iter().filter(|s| s.track_id.is_none() && s.bundle_id == bundle_id).collect())
    })
}

/// Сравнение версий по числовым частям («12.15.0» < «12.16»). `None`, если сравнить нельзя:
/// нечисловая часть, пустая строка или одна часть против нескольких («50800» и «5.8.0»).
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.trim().split('.').map(|p| if p.bytes().all(|c| c.is_ascii_digit()) { p.parse().ok() } else { None }).collect()
    }
    let (mut x, mut y) = (parts(a)?, parts(b)?);
    if x.len() != y.len() && x.len().min(y.len()) == 1 {
        return None;
    }
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    Some(x.cmp(&y))
}

/// Состояние по версиям. Доступная версия неизвестна — просто «стоит».
pub fn install_state(installed: Option<&str>, available: Option<&str>) -> InstallState {
    let Some(have) = installed else { return InstallState::NotInstalled };
    let Some(avail) = available else { return InstallState::Installed };
    match compare_versions(have, avail) {
        Some(Ordering::Less) => InstallState::UpdateAvailable,
        Some(_) => InstallState::Installed,
        None => InstallState::Unknown,
    }
}

/// Установленное приложение для строки: по App Store id, а если такого нет, по bundle id.
pub fn find_installed<'a>(installed: &'a [InstalledApp], track_id: i64, bundle_id: &str) -> Option<&'a InstalledApp> {
    installed
        .iter()
        .find(|a| a.item_id == Some(track_id))
        .or_else(|| installed.iter().find(|a| !bundle_id.is_empty() && a.bundle_id == bundle_id))
}

/// UIDeviceFamily без iPhone (1), например `[2]`. Пусто — неизвестно, считаем, что iPhone подходит.
pub fn is_ipad_only(family: &[u8]) -> bool {
    !family.is_empty() && !family.contains(&1)
}

/// Сохранённая копия по пути — только для iPad?
fn copy_ipad_only(saved: &[SavedIpa], path: Option<&std::path::Path>) -> bool {
    path.and_then(|p| saved.iter().find(|s| s.path == p)).is_some_and(|s| is_ipad_only(&s.device_family))
}

/// Отмечать ли строку сразу. Только записи каталога (покупок сотни — их выбирают руками) и только
/// те, что можно поставить: права подтверждены или есть копия. `blocked` — приложение только для iPad,
/// а ставим на iPhone.
pub fn default_checked(source: RowSource, owned: Option<bool>, has_saved_copy: bool, state: InstallState, blocked: bool) -> bool {
    let can_install = (owned == Some(true) || has_saved_copy) && !blocked;
    source == RowSource::Removed
        && can_install
        && matches!(state, InstallState::NotInstalled | InstallState::UpdateAvailable)
}

/// Подставляет версии и состояние. `installed = None` — телефона нет: колонки «На iPhone» нет.
/// `latest`: последние версии у Apple по записям каталога (итоги `check_owned`).
pub fn apply_versions(rows: &mut [AppRow], installed: Option<&[InstalledApp]>, latest: &HashMap<i64, String>, saved: &[SavedIpa]) {
    for r in rows {
        let copy = r.saved_copy.as_deref().and_then(|p| saved.iter().find(|s| s.path == p));
        if r.source == RowSource::Removed {
            r.available_version = copy
                .map(|s| s.version.clone())
                .filter(|v| !v.is_empty())
                .or_else(|| latest.get(&r.app.id).cloned());
        }
        // у записей каталога bundle id известен только из сохранённой копии
        let bundle = if r.app.bundle_id.is_empty() { copy.map(|s| s.bundle_id.as_str()).unwrap_or("") } else { &r.app.bundle_id };
        r.installed_version = installed.and_then(|list| find_installed(list, r.app.id, bundle)).map(|a| a.version.clone());
        r.install_state = install_state(r.installed_version.as_deref(), r.available_version.as_deref());
    }
}

/// Версия сохранённой копии по её пути.
pub(crate) fn saved_version(saved: &[SavedIpa], path: &std::path::Path) -> Option<String> {
    saved.iter().find(|s| s.path == path).map(|s| s.version.clone()).filter(|v| !v.is_empty())
}

/// Записи каталога идут сверху в порядке файла; покупки с тем же trackId не дублируются.
/// Дальше покупки: сначала удалённые из российского App Store, внутри групп по имени без учёта регистра.
/// `present_ru = None` означает, что Lookup упал: все `removed_from_ru = false`.
pub fn build_rows(
    removed: &[RemovedApp],
    owned_removed: &HashMap<i64, bool>,
    purchases: Vec<OwnedApp>,
    present_ru: Option<&HashSet<i64>>,
    saved: &[SavedIpa],
) -> Rows {
    let catalog_ids: HashSet<i64> = removed.iter().map(|r| r.track_id).collect();
    let mut rows: Vec<AppRow> = removed
        .iter()
        .map(|r| {
            let saved_copy = find_saved(saved, r.track_id, "");
            let ipad_only = r.ipad_only || copy_ipad_only(saved, saved_copy.as_deref());
            AppRow {
                app: OwnedApp {
                    id: r.track_id,
                    bundle_id: String::new(),
                    name: r.name.clone(),
                    version: String::new(),
                    platforms: vec![],
                },
                removed_from_ru: true,
                available_version: saved_copy.as_deref().and_then(|p| saved_version(saved, p)),
                saved_copy,
                owned: owned_removed.get(&r.track_id).copied(),
                source: RowSource::Removed,
                letter: Some(r.letter.clone()),
                color: Some(r.color.clone()),
                installed_version: None,
                install_state: InstallState::NotInstalled,
                ipad_only,
                category: Some(r.category()),
                aliases: r.aliases.clone(),
            }
        })
        .collect();
    let mut bought: Vec<AppRow> = purchases
        .into_iter()
        .filter(|app| !catalog_ids.contains(&app.id))
        .map(|app| {
            let removed_from_ru = present_ru.is_some_and(|p| !p.contains(&app.id));
            let saved_copy = find_saved(saved, app.id, &app.bundle_id);
            let available_version = Some(app.version.clone()).filter(|v| !v.is_empty());
            let ipad_only = copy_ipad_only(saved, saved_copy.as_deref());
            AppRow {
                app,
                removed_from_ru,
                saved_copy,
                owned: Some(true),
                source: RowSource::Purchase,
                letter: None,
                color: None,
                installed_version: None,
                available_version,
                install_state: InstallState::NotInstalled,
                ipad_only,
                category: None,
                aliases: vec![],
            }
        })
        .collect();
    bought.sort_by_cached_key(|r| (!r.removed_from_ru, r.app.name.to_lowercase()));
    rows.extend(bought);
    Rows { rows, ru_check_failed: present_ru.is_none() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::removed_catalog::RemovedApp;

    fn app(id: i64, bundle: &str, name: &str) -> OwnedApp {
        OwnedApp { id, bundle_id: bundle.into(), name: name.into(), version: "1.0".into(), platforms: vec![] }
    }

    #[test]
    fn removed_first_then_by_name_case_insensitive() {
        let owned = vec![app(1, "a", "beta"), app(2, "b", "Alpha"), app(3, "c", "zeta"), app(4, "d", "Omega")];
        let present: HashSet<i64> = [1, 2].into();
        let r = build_rows(&[], &HashMap::new(), owned, Some(&present), &[]);
        let names: Vec<_> = r.rows.iter().map(|x| x.app.name.as_str()).collect();
        assert_eq!(names, ["Omega", "zeta", "Alpha", "beta"]);
        assert!(r.rows[0].removed_from_ru && r.rows[1].removed_from_ru);
        assert!(!r.rows[2].removed_from_ru && !r.ru_check_failed);
    }

    #[test]
    fn empty_owned_gives_empty_rows() {
        let r = build_rows(&[], &HashMap::new(), vec![], Some(&HashSet::new()), &[]);
        assert!(r.rows.is_empty() && !r.ru_check_failed);
        let r = build_rows(&[], &HashMap::new(), vec![], None, &[]);
        assert!(r.rows.is_empty());
    }

    #[test]
    fn saved_copy_matched_by_bundle_id() {
        let saved = vec![SavedIpa { track_id: None, bundle_id: "b".into(), name: None, version: "2".into(), device_family: vec![], path: "/x/b_2.ipa".into() }];
        let r = build_rows(&[], &HashMap::new(), vec![app(1, "a", "A"), app(2, "b", "B")], Some(&[1, 2].into()), &saved);
        assert_eq!(r.rows[0].saved_copy, None);
        assert_eq!(r.rows[1].saved_copy, Some(PathBuf::from("/x/b_2.ipa")));
    }

    #[test]
    fn newest_saved_copy_wins_by_mtime() {
        let t = tempfile::tempdir().unwrap();
        let mk = |name: &str, age_s: u64| {
            let p = t.path().join(name);
            let f = std::fs::File::create(&p).unwrap();
            f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age_s)).unwrap();
            SavedIpa { track_id: None, bundle_id: "b".into(), name: None, version: "x".into(), device_family: vec![], path: p }
        };
        // самая новая версия стоит в списке посередине
        let saved = vec![mk("b_1.0.ipa", 300), mk("b_3.0.ipa", 10), mk("b_2.0.ipa", 100)];
        let r = build_rows(&[], &HashMap::new(), vec![app(1, "b", "B")], Some(&[1].into()), &saved);
        assert_eq!(r.rows[0].saved_copy, Some(t.path().join("b_3.0.ipa")));
    }

    #[test]
    fn lookup_failed_marks_nothing_removed() {
        let r = build_rows(&[], &HashMap::new(), vec![app(1, "a", "A"), app(2, "b", "B")], None, &[]);
        assert!(r.ru_check_failed);
        assert!(r.rows.iter().all(|x| !x.removed_from_ru));
    }

    fn removed(id: i64, name: &str) -> RemovedApp {
        RemovedApp {
            track_id: id,
            name: name.into(),
            aliases: vec![],
            letter: name.chars().next().unwrap().to_string(),
            color: "#000000".into(),
            source: "test".into(),
            status: None,
            seller: None,
            ipad_only: false,
            category: None,
        }
    }

    #[test]
    fn catalog_rows_first_in_file_order_and_not_duplicated() {
        let cat = vec![removed(900, "Яблоко"), removed(100, "Арбуз")];
        // покупка 100 совпала с каталогом по trackId и не дублируется
        let owned = vec![app(100, "ru.arbuz", "Арбуз (магазин)"), app(1, "a", "Alpha"), app(2, "b", "Beta")];
        let r = build_rows(&cat, &HashMap::new(), owned, Some(&[1, 2].into()), &[]);
        let ids: Vec<_> = r.rows.iter().map(|x| x.app.id).collect();
        assert_eq!(ids, [900, 100, 1, 2]);
        assert!(r.rows[..2].iter().all(|x| x.source == RowSource::Removed && x.removed_from_ru));
        assert!(r.rows[2..].iter().all(|x| x.source == RowSource::Purchase));
        assert_eq!(r.rows[1].app.name, "Арбуз");
        assert_eq!((r.rows[0].letter.as_deref(), r.rows[0].color.as_deref()), (Some("Я"), Some("#000000")));
        assert_eq!(r.rows[2].letter, None);
    }

    #[test]
    fn owned_is_none_until_checked() {
        let cat = vec![removed(900, "A"), removed(901, "B"), removed(902, "C")];
        let checked: HashMap<i64, bool> = [(900, true), (901, false)].into();
        let r = build_rows(&cat, &checked, vec![app(1, "a", "X")], Some(&[1].into()), &[]);
        let owned: Vec<_> = r.rows.iter().map(|x| x.owned).collect();
        // покупка из list-purchases уже доказывает права
        assert_eq!(owned, [Some(true), Some(false), None, Some(true)]);
    }

    #[test]
    fn versions_compare_by_numeric_parts() {
        use Ordering::*;
        assert_eq!(compare_versions("12.15.0", "12.16"), Some(Less));
        assert_eq!(compare_versions("12.16", "12.15.0"), Some(Greater));
        assert_eq!(compare_versions("2.1.1", "2.1.1"), Some(Equal));
        assert_eq!(compare_versions("2.1", "2.1.0"), Some(Equal));
        assert_eq!(compare_versions("1.10", "1.9"), Some(Greater), "числа, не строки");
        assert_eq!(compare_versions(" 3.0 ", "3.0"), Some(Equal));
        assert_eq!(compare_versions("50800", "5.8.0"), None);
        assert_eq!(compare_versions("5.8.0", "50800"), None);
        assert_eq!(compare_versions("7", "8"), Some(Less), "обе из одной части сравнимы");
        for (a, b) in [("1.0b3", "1.0"), ("", "1.0"), ("1..2", "1.2"), ("1.0", "-1.0")] {
            assert_eq!(compare_versions(a, b), None, "{a} vs {b}");
        }
    }

    #[test]
    fn install_state_from_versions() {
        use InstallState::*;
        assert_eq!(install_state(None, Some("1.0")), NotInstalled);
        assert_eq!(install_state(None, None), NotInstalled);
        assert_eq!(install_state(Some("12.15.0"), Some("12.16")), UpdateAvailable);
        assert_eq!(install_state(Some("2.1.1"), Some("2.1.1")), Installed);
        assert_eq!(install_state(Some("3.0"), Some("2.9")), Installed, "на телефоне новее");
        assert_eq!(install_state(Some("50800"), Some("5.8.0")), Unknown);
        assert_eq!(install_state(Some("1.0"), None), Installed, "доступная версия ещё не известна");
        assert_eq!(install_state(Some(""), Some("1.0")), Unknown);
    }

    fn inst(bundle: &str, version: &str, item: Option<i64>) -> InstalledApp {
        InstalledApp { bundle_id: bundle.into(), name: bundle.into(), version: version.into(), item_id: item, purchaser: None }
    }

    #[test]
    fn installed_matched_by_item_id_then_bundle() {
        let phone = [inst("ru.sberbank.onlineiphone", "12.14.0", Some(492224193)), inst("com.example.corp", "1", None)];
        // bundle id из покупок другой, но App Store id тот же
        assert_eq!(find_installed(&phone, 492224193, "ru.sberbankmobile").map(|a| a.version.as_str()), Some("12.14.0"));
        assert_eq!(find_installed(&phone, 5, "com.example.corp").map(|a| a.bundle_id.as_str()), Some("com.example.corp"));
        assert_eq!(find_installed(&phone, 5, ""), None, "пустой bundle id ни с чем не совпадает");
        assert_eq!(find_installed(&phone, 5, "com.example.other"), None);
        assert_eq!(find_installed(&[], 492224193, "x"), None);
    }

    #[test]
    fn default_checked_by_state() {
        use InstallState::*;
        let cat = |owned, copy, st| default_checked(RowSource::Removed, owned, copy, st, false);
        assert!(cat(Some(true), false, NotInstalled));
        assert!(!cat(Some(true), false, Installed));
        assert!(cat(Some(true), false, UpdateAvailable));
        assert!(!cat(Some(true), false, Unknown));
        assert!(cat(Some(false), true, NotInstalled), "копия на компьютере ставится и без прав");
        assert!(!cat(Some(false), false, NotInstalled), "без прав и без копии не ставится");
        assert!(!cat(None, false, NotInstalled), "права ещё проверяются");
        assert!(!default_checked(RowSource::Purchase, Some(true), false, NotInstalled, false), "покупки выбирают руками");
        assert!(!default_checked(RowSource::Purchase, Some(true), false, UpdateAvailable, false));
        assert!(!default_checked(RowSource::Removed, Some(true), true, NotInstalled, true), "только для iPad, ставим на iPhone");
    }

    #[test]
    fn ipad_only_from_family_and_catalog() {
        assert!(is_ipad_only(&[2]));
        assert!(!is_ipad_only(&[1, 2]));
        assert!(!is_ipad_only(&[1]));
        assert!(!is_ipad_only(&[]), "неизвестно — не прячем");
        let saved = vec![SavedIpa { track_id: Some(901), bundle_id: "com.vk.vkhd".into(), name: None, version: "2.0.4".into(), device_family: vec![2], path: "/x/901.ipa".into() }];
        let flagged = RemovedApp { ipad_only: true, ..removed(900, "Флаг") };
        let r = build_rows(&[flagged, removed(901, "Копия"), removed(902, "Обычное")], &HashMap::new(), vec![], Some(&HashSet::new()), &saved);
        assert_eq!(r.rows.iter().map(|x| x.ipad_only).collect::<Vec<_>>(), [true, true, false]);
    }

    #[test]
    fn apply_versions_fills_rows() {
        let saved = vec![SavedIpa { track_id: Some(901), bundle_id: "ru.copy".into(), name: None, version: "3.0".into(), device_family: vec![], path: "/x/901.ipa".into() }];
        let cat = vec![removed(900, "Сбер"), removed(901, "Копия"), removed(902, "Нет")];
        let buy = vec![OwnedApp { version: "12.16".into(), ..app(1, "com.example.a", "A") }, app(2, "com.example.b", "B")];
        let mut r = build_rows(&cat, &HashMap::new(), buy, Some(&[1, 2].into()), &saved);
        // до телефона: доступные версии уже известны (копия, покупка)
        let avail: Vec<_> = r.rows.iter().map(|x| x.available_version.as_deref()).collect();
        assert_eq!(avail, [None, Some("3.0"), None, Some("12.16"), Some("1.0")]);
        let phone = [
            inst("ru.sberbank.onlineiphone", "12.14.0", Some(900)),
            inst("ru.copy", "3.0", None),
            inst("com.example.a", "12.15.0", None),
            inst("com.example.b", "50800", Some(2)),
        ];
        let latest: HashMap<i64, String> = [(900, "12.15.0".to_string()), (901, "9.9".to_string())].into();
        apply_versions(&mut r.rows, Some(&phone), &latest, &saved);
        let got: Vec<_> = r.rows.iter().map(|x| (x.app.id, x.installed_version.as_deref(), x.available_version.as_deref(), x.install_state)).collect();
        use InstallState::*;
        assert_eq!(
            got,
            [
                (900, Some("12.14.0"), Some("12.15.0"), UpdateAvailable),
                // копия важнее версии у Apple: поставится именно она; на телефоне ищется по bundle id копии
                (901, Some("3.0"), Some("3.0"), Installed),
                (902, None, None, NotInstalled),
                (1, Some("12.15.0"), Some("12.16"), UpdateAvailable),
                (2, Some("50800"), Some("1.0"), Unknown),
            ]
        );
        // телефона нет: установленного нет, доступное остаётся
        apply_versions(&mut r.rows, None, &latest, &saved);
        assert!(r.rows.iter().all(|x| x.installed_version.is_none() && x.install_state == NotInstalled));
        assert_eq!(r.rows[0].available_version.as_deref(), Some("12.15.0"));
    }

    #[test]
    fn saved_copy_matched_by_track_id_first_then_bundle() {
        let sv = |t: Option<i64>, b: &str, p: &str| SavedIpa { track_id: t, bundle_id: b.into(), name: None, version: "1".into(), device_family: vec![], path: p.into() };
        let saved = vec![sv(Some(900), "ru.removed", "/x/900.ipa"), sv(None, "b", "/x/b_1.ipa"), sv(Some(2), "b", "/x/2_b_1.ipa")];
        let r = build_rows(&[removed(900, "R")], &HashMap::new(), vec![app(2, "b", "B")], Some(&[2].into()), &saved);
        assert_eq!(r.rows[0].saved_copy, Some(PathBuf::from("/x/900.ipa")));
        // покупка 2: по trackId есть копия, она важнее копии со старым именем (без trackId)
        assert_eq!(r.rows[1].saved_copy, Some(PathBuf::from("/x/2_b_1.ipa")));
        // старое имя без trackId находится по bundle id
        let r = build_rows(&[], &HashMap::new(), vec![app(3, "b", "B")], Some(&[3].into()), &saved[1..2]);
        assert_eq!(r.rows[0].saved_copy, Some(PathBuf::from("/x/b_1.ipa")));
    }
}
