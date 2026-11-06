//! Найденное в «Истории покупок Apple» — между запусками, по Apple ID (ключ как в accounts.json):
//! только номер и название приложения, ни cookie, ни токенов сайта. Файл `history-found.json`
//! пишется целиком через временный; битый — пропускается и пересобирается следующей записью.
//! «Выйти» и «Забыть» убирают записи своего Apple ID, как ответы `owned-cache.json`.
use crate::history::{Found, MAX_ITEMS, MAX_NAME, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

/// Apple ID → найденное, в порядке находок.
type Data = HashMap<String, Vec<Saved>>;

/// Прежнее и новое найденное: прежний порядок, новое — в конец, без повторов; имя берётся
/// известное (новое, если есть). Больше `MAX_ITEMS` не хранится.
pub fn merge(old: &[Found], new: &[Found]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for f in old.iter().chain(new) {
        match at.get(&f.id) {
            Some(&i) => {
                if f.name.is_some() {
                    out[i].name = f.name.clone();
                }
            }
            None if out.len() < MAX_ITEMS => {
                at.insert(f.id, out.len());
                out.push(f.clone());
            }
            None => {}
        }
    }
    out
}

/// Файл свой, но правится руками и ломается: те же правила, что у страницы истории.
fn checked(list: Vec<Saved>) -> Vec<Found> {
    let mut seen = HashSet::new();
    list.into_iter()
        .filter(|s| valid_id(&s.id.to_string()).is_some() && seen.insert(s.id))
        .take(MAX_ITEMS)
        .map(|s| Found { id: s.id, name: s.name.filter(|n| !n.trim().is_empty() && n.chars().count() <= MAX_NAME && !n.chars().any(char::is_control)) })
        .collect()
}

fn parse(text: &str) -> Data {
    serde_json::from_str(text).unwrap_or_default()
}

/// Найденное в истории и его файл.
pub struct HistoryStore {
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

impl HistoryStore {
    /// Читает файл; нет его или он битый — пусто.
    pub fn open(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        Self { path: Some(path), data: Mutex::new(data) }
    }

    /// Найденное Apple ID `email` (проверенное, без повторов).
    pub fn found(&self, email: &str) -> Vec<Found> {
        self.data.lock().unwrap().get(&crate::accounts::key(email)).cloned().map(checked).unwrap_or_default()
    }

    /// Дописать найденное Apple ID `email` и сохранить файл. Нечего дописать — файл не трогается.
    pub fn remember(&self, email: &str, items: &[Found]) {
        let k = crate::accounts::key(email);
        if k.is_empty() || items.is_empty() {
            return;
        }
        let mut d = self.data.lock().unwrap();
        let old = d.get(&k).cloned().map(checked).unwrap_or_default();
        let merged = merge(&old, items);
        d.insert(k, merged.into_iter().map(|f| Saved { id: f.id, name: f.name }).collect());
        self.save(&d);
    }

    /// «Выйти» и «Забыть»: найденное этим Apple ID больше не храним.
    pub fn forget(&self, email: &str) {
        let mut d = self.data.lock().unwrap();
        if d.remove(&crate::accounts::key(email)).is_some() {
            self.save(&d);
        }
    }

    fn save(&self, d: &Data) {
        let Some(p) = &self.path else { return };
        let r = serde_json::to_vec(d).map_err(|e| e.to_string()).and_then(|b| crate::accounts::write_atomic(p, &b));
        if let Err(e) = r {
            crate::log::write("history store", &e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: i64, name: Option<&str>) -> Found {
        Found { id, name: name.map(str::to_string) }
    }

    fn store(dir: &tempfile::TempDir) -> HistoryStore {
        HistoryStore::open(dir.path().join("history-found.json"))
    }

    #[test]
    fn merge_keeps_order_appends_new_and_skips_repeats() {
        let m = merge(&[f(111111, Some("А")), f(222222, None)], &[f(222222, Some("Б")), f(333333, Some("В")), f(111111, None)]);
        assert_eq!(m, vec![f(111111, Some("А")), f(222222, Some("Б")), f(333333, Some("В"))]);
    }

    #[test]
    fn merge_caps_at_max_items() {
        let old: Vec<Found> = (0..MAX_ITEMS as i64).map(|i| f(1_000_000 + i, None)).collect();
        let m = merge(&old, &[f(9_999_999, Some("лишнее"))]);
        assert_eq!(m.len(), MAX_ITEMS);
        assert!(!m.iter().any(|x| x.id == 9_999_999));
    }

    #[test]
    fn remembered_items_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).remember("Ma@Example.com ", &[f(111111, Some("Банк")), f(222222, None)]);
        let again = store(&dir);
        assert_eq!(again.found("ma@example.com"), vec![f(111111, Some("Банк")), f(222222, None)]);
    }

    #[test]
    fn second_visit_adds_to_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        s.remember("ma@example.com", &[f(111111, None)]);
        s.remember("ma@example.com", &[f(111111, Some("Банк")), f(222222, Some("Почта"))]);
        assert_eq!(store(&dir).found("ma@example.com"), vec![f(111111, Some("Банк")), f(222222, Some("Почта"))]);
    }

    #[test]
    fn accounts_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        s.remember("a@example.com", &[f(111111, None)]);
        s.remember("b@example.com", &[f(222222, None)]);
        assert_eq!(s.found("a@example.com"), vec![f(111111, None)]);
        assert_eq!(s.found("b@example.com"), vec![f(222222, None)]);
        assert!(s.found("c@example.com").is_empty());
    }

    #[test]
    fn forget_drops_only_that_account_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        s.remember("a@example.com", &[f(111111, None)]);
        s.remember("b@example.com", &[f(222222, None)]);
        s.forget("A@example.com");
        let again = store(&dir);
        assert!(again.found("a@example.com").is_empty());
        assert_eq!(again.found("b@example.com"), vec![f(222222, None)]);
    }

    #[test]
    fn empty_email_or_nothing_found_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        s.remember("  ", &[f(111111, None)]);
        s.remember("a@example.com", &[]);
        assert!(!dir.path().join("history-found.json").exists());
    }

    #[test]
    fn broken_file_reads_as_empty_and_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("history-found.json"), "{не json").unwrap();
        let s = store(&dir);
        assert!(s.found("a@example.com").is_empty());
        s.remember("a@example.com", &[f(111111, None)]);
        assert_eq!(store(&dir).found("a@example.com"), vec![f(111111, None)]);
    }

    #[test]
    fn hand_edited_bad_entries_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let long = "я".repeat(MAX_NAME + 1);
        let text = format!(r#"{{"a@example.com":[{{"id":12}},{{"id":111111,"name":"{long}"}},{{"id":111111}},{{"id":222222,"name":"ok\u0007"}},{{"id":333333,"name":"Почта"}}]}}"#);
        std::fs::write(dir.path().join("history-found.json"), text).unwrap();
        assert_eq!(store(&dir).found("a@example.com"), vec![f(111111, None), f(222222, None), f(333333, Some("Почта"))]);
    }
}
