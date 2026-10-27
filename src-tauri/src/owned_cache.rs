//! Кэш ответов `check_owned` между запусками: по Apple ID (ключ как в accounts.json), trackId →
//! {owned, checked_at}. Ошибки не кэшируются. Файл пишется целиком через временный; битый —
//! пропускается и пересобирается следующей записью.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

/// Через сколько ответ перепроверяется в фоне (плитка до того — с ответом из кэша).
pub const TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// Старше этого ответ из файла убирается при записи: строки поиска не копятся без конца.
pub const KEEP_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub owned: bool,
    /// Секунды Unix.
    pub checked_at: u64,
    /// Последняя версия у Apple из того же ответа (у «нет прав» её нет).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Apple ID → trackId → ответ.
pub type Data = HashMap<String, HashMap<i64, Entry>>;

/// Ключ Apple ID: без пробелов по краям и без регистра.
pub fn key(email: &str) -> String {
    crate::accounts::key(email)
}

/// Ответ ещё свежий: младше `TTL_SECS`. Время «из будущего» (часы перевели) — тоже свежий.
pub fn fresh(e: &Entry, now: u64) -> bool {
    now.saturating_sub(e.checked_at) < TTL_SECS
}

/// Содержимое файла; битое или чужого вида — пусто.
pub fn parse(text: &str) -> Data {
    serde_json::from_str(text).unwrap_or_default()
}

/// Убрать ответы старше `KEEP_SECS` и опустевшие Apple ID.
pub fn prune(d: &mut Data, now: u64) {
    for entries in d.values_mut() {
        entries.retain(|_, e| now.saturating_sub(e.checked_at) < KEEP_SECS);
    }
    d.retain(|_, entries| !entries.is_empty());
}

/// Что делать с запрошенными trackId: `cached` — ответ из кэша отдать сразу; `check` — к ipatool
/// (`true` — перепроверка устаревшего: плитка уже с ответом из кэша). Порядок запроса, без повторов.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub cached: Vec<(i64, Entry)>,
    pub check: Vec<(i64, bool)>,
}

/// `stale` — устаревшие записи этого Apple ID, которые стоит перепроверить и без запроса
/// (уходят в конец, после запрошенных).
pub fn plan(ids: &[i64], entries: &HashMap<i64, Entry>, stale: &[i64], now: u64) -> Plan {
    let mut out = Plan::default();
    let mut seen = HashSet::new();
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        match entries.get(&id) {
            Some(e) if fresh(e, now) => out.cached.push((id, e.clone())),
            Some(e) => {
                out.cached.push((id, e.clone()));
                out.check.push((id, true));
            }
            None => out.check.push((id, false)),
        }
    }
    for &id in stale {
        if entries.get(&id).is_some_and(|e| !fresh(e, now)) && seen.insert(id) {
            out.check.push((id, true));
        }
    }
    out
}

/// Какую проверку взять следующей: видимые при текущем поиске/фильтре — вперёд, затем
/// запрошенные раньше перепроверок, внутри — по порядку. `None` — очередь пуста.
pub fn next_check(queue: &[(i64, bool)], visible: &HashSet<i64>) -> Option<usize> {
    queue
        .iter()
        .enumerate()
        .min_by_key(|(i, (id, stale))| (!visible.contains(id), *stale, *i))
        .map(|(i, _)| i)
}

/// Устаревшие ответы, чьи строки сейчас на «Выборе» (`on_screen`): их перепроверяют в фоне. По trackId.
pub fn stale_ids(entries: &HashMap<i64, Entry>, on_screen: &HashSet<i64>, now: u64) -> Vec<i64> {
    let mut v: Vec<i64> = entries.iter().filter(|(id, e)| on_screen.contains(id) && !fresh(e, now)).map(|(id, _)| *id).collect();
    v.sort_unstable();
    v
}

/// Ответы из покупок Apple ID: куплено — значит, права есть; версия — из списка покупок.
pub fn from_purchases(purchases: &[crate::ipatool::output::OwnedApp], now: u64) -> Vec<(i64, Entry)> {
    purchases
        .iter()
        .map(|a| (a.id, Entry { owned: true, checked_at: now, version: Some(a.version.clone()).filter(|v| !v.is_empty()) }))
        .collect()
}

/// Тот же ли Apple ID у программы (вход «с нуля» и выход — смена): ответы одного не годятся другому.
pub fn same_account(before: Option<&str>, after: Option<&str>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(b), Some(a)) => crate::accounts::same_id(b, a),
        _ => false,
    }
}

/// Кэш в памяти и его файл. Пишет каждый ответ сразу (файл маленький, ответы раз в 2–4 с).
pub struct OwnedCache {
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

impl OwnedCache {
    /// Читает файл; нет его или он битый — пустой кэш.
    pub fn open(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        Self { path: Some(path), data: Mutex::new(data) }
    }

    /// Ответы Apple ID `email`.
    pub fn entries(&self, email: &str) -> HashMap<i64, Entry> {
        self.data.lock().unwrap().get(&key(email)).cloned().unwrap_or_default()
    }

    /// Записать ответы Apple ID `email` и сохранить файл (старые ответы убираются, `prune`).
    pub fn put(&self, email: &str, answers: impl IntoIterator<Item = (i64, Entry)>, now: u64) {
        let k = key(email);
        if k.is_empty() {
            return;
        }
        let mut d = self.data.lock().unwrap();
        d.entry(k).or_default().extend(answers);
        prune(&mut d, now);
        self.save(&d);
    }

    /// «Забыть» и выход: ответы этого Apple ID больше не нужны.
    pub fn forget(&self, email: &str) {
        let mut d = self.data.lock().unwrap();
        if d.remove(&key(email)).is_some() {
            self.save(&d);
        }
    }

    fn save(&self, d: &Data) {
        let Some(p) = &self.path else { return };
        let r = serde_json::to_vec(d).map_err(|e| e.to_string()).and_then(|b| crate::accounts::write_atomic(p, &b));
        if let Err(e) = r {
            crate::log::write("owned cache", &e);
        }
    }
}

/// Сейчас, секунды Unix.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(owned: bool, at: u64) -> Entry {
        Entry { owned, checked_at: at, version: None }
    }

    const NOW: u64 = 1_000_000_000;

    #[test]
    fn key_ignores_case_and_spaces() {
        assert_eq!(key("  User@Example.com "), "user@example.com");
    }

    #[test]
    fn fresh_until_seven_days() {
        assert!(fresh(&e(true, NOW - TTL_SECS + 1), NOW));
        assert!(!fresh(&e(true, NOW - TTL_SECS), NOW));
        assert!(fresh(&e(false, NOW + 50), NOW), "часы назад — не повод спрашивать Apple");
    }

    #[test]
    fn corrupt_or_foreign_file_is_empty() {
        assert!(parse("{not json").is_empty());
        assert!(parse("[1,2]").is_empty());
        assert!(parse("").is_empty());
        let ok = parse(r#"{"user@example.com":{"5":{"owned":true,"checked_at":7}}}"#);
        assert_eq!(ok["user@example.com"][&5], e(true, 7));
    }

    #[test]
    fn plan_splits_fresh_stale_and_unknown() {
        let entries: HashMap<i64, Entry> = [(1, e(true, NOW)), (2, e(false, NOW - TTL_SECS - 1)), (9, e(true, 0))].into();
        let p = plan(&[1, 2, 3, 1], &entries, &[9, 2, 1, 4], NOW);
        assert_eq!(p.cached, vec![(1, e(true, NOW)), (2, e(false, NOW - TTL_SECS - 1))]);
        // 9 — устаревшая без запроса; 1 свежая и 4 без записи не перепроверяются; 2 не дважды
        assert_eq!(p.check, vec![(2, true), (3, false), (9, true)]);
    }

    #[test]
    fn next_check_prefers_visible_then_requested() {
        let q = vec![(1, false), (2, true), (3, false), (4, true)];
        assert_eq!(next_check(&q, &HashSet::new()), Some(0));
        assert_eq!(next_check(&q, &[3].into()), Some(2));
        assert_eq!(next_check(&q, &[4, 2].into()), Some(1));
        assert_eq!(next_check(&q[1..2], &HashSet::new()), Some(0));
        assert_eq!(next_check(&[], &[1].into()), None);
    }

    #[test]
    fn prune_drops_answers_older_than_keep_and_empty_accounts() {
        let mut d: Data = [
            ("user@example.com".to_string(), [(1, e(true, NOW - KEEP_SECS)), (2, e(false, NOW - KEEP_SECS + 1))].into()),
            ("other@example.com".to_string(), [(3, e(true, 0))].into()),
        ]
        .into();
        prune(&mut d, NOW);
        assert_eq!(d.len(), 1);
        assert_eq!(d["user@example.com"].keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn put_prunes_old_answers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("owned-cache.json");
        let c = OwnedCache::open(p.clone());
        c.put("user@example.com", [(1, e(true, 0))], NOW);
        c.put("user@example.com", [(2, e(true, NOW))], NOW);
        assert_eq!(OwnedCache::open(p).entries("user@example.com").len(), 1);
    }

    #[test]
    fn stale_ids_only_on_screen() {
        let entries: HashMap<i64, Entry> = [(1, e(true, 0)), (2, e(false, 0)), (3, e(true, NOW)), (4, e(true, 0))].into();
        assert_eq!(stale_ids(&entries, &[4, 1, 3].into(), NOW), vec![1, 4]);
    }

    #[test]
    fn purchases_are_owned() {
        let a = |id, v: &str| crate::ipatool::output::OwnedApp { id, bundle_id: "com.example.a".into(), name: "A".into(), version: v.into(), platforms: vec![] };
        let got = from_purchases(&[a(1, "2.0"), a(2, "")], NOW);
        assert_eq!(got, vec![(1, Entry { owned: true, checked_at: NOW, version: Some("2.0".into()) }), (2, e(true, NOW))]);
    }

    #[test]
    fn same_account_cases() {
        assert!(same_account(None, None));
        assert!(same_account(Some("User@example.com"), Some(" user@example.com")));
        assert!(!same_account(Some("user@example.com"), Some("other@example.com")));
        assert!(!same_account(None, Some("user@example.com")));
        assert!(!same_account(Some("user@example.com"), None));
    }

    #[test]
    fn put_saves_atomically_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("owned-cache.json");
        let c = OwnedCache::open(p.clone());
        c.put("User@Example.com", [(5, e(true, 7))], 7);
        c.put("other@example.com", [(5, e(false, 8))], 8);
        let again = OwnedCache::open(p.clone());
        assert_eq!(again.entries("user@example.com")[&5], e(true, 7));
        assert_eq!(again.entries("other@example.com")[&5], e(false, 8), "у каждого Apple ID свои ответы");
        assert!(!dir.path().join(".owned-cache.json.app-restore.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_ignored_and_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("owned-cache.json");
        std::fs::write(&p, b"{\"user@example.com\": {\"5\": tru").unwrap();
        let c = OwnedCache::open(p.clone());
        assert!(c.entries("user@example.com").is_empty());
        c.put("user@example.com", [(6, e(false, 1))], 1);
        assert_eq!(parse(&std::fs::read_to_string(&p).unwrap())["user@example.com"].len(), 1);
    }

    #[test]
    fn forget_drops_only_that_account() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("owned-cache.json");
        let c = OwnedCache::open(p.clone());
        c.put("user@example.com", [(5, e(true, 7))], 7);
        c.put("other@example.com", [(5, e(true, 7))], 7);
        c.forget(" USER@example.com");
        let again = OwnedCache::open(p);
        assert!(again.entries("user@example.com").is_empty());
        assert_eq!(again.entries("other@example.com").len(), 1);
    }
}
