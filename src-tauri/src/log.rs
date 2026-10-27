//! Журнал для поддержки: `app.log` в папке журналов программы (macOS — `~/Library/Logs/<id>/`).
//! Размер ограничен: больше `CAP` — прежний файл становится `app.log.1`, новый начинается с нуля.
//! Перед записью строка чистится (`scrub`): адреса — маской, домашняя папка — `~`, значения
//! паролей, кодов, токенов и cookie — `***`. Пароль и код в журнал не передаются никогда; `scrub`
//! — страховка для чужого текста (ответы ipatool, ошибки библиотек).
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const FILE: &str = "app.log";
const CAP: u64 = 1024 * 1024;

struct Sink {
    dir: PathBuf,
    home: Option<String>,
    lock: Mutex<()>,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Один раз при запуске. До неё (и в тестах) строки идут только в stderr.
pub fn init(dir: PathBuf) {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 1);
    let _ = std::fs::create_dir_all(&dir);
    let _ = SINK.set(Sink { dir, home, lock: Mutex::new(()) });
}

/// Папка журнала (для «Открыть журнал»), если журнал включён.
pub fn dir() -> Option<PathBuf> {
    SINK.get().map(|s| s.dir.clone())
}

/// Строка в журнал и stderr. `context` — где (команда, шаг), `msg` — что случилось.
pub fn write(context: &str, msg: &str) {
    let home = SINK.get().and_then(|s| s.home.as_deref());
    let line = scrub(&format!("{context}: {msg}"), home);
    eprintln!("[app-restore] {line}");
    if let Some(s) = SINK.get() {
        let _g = s.lock.lock().unwrap_or_else(|e| e.into_inner());
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = append(&s.dir.join(FILE), &format!("{secs} {line}\n"), CAP);
    }
}

/// Дописывает `line`; файл перерос бы `cap` — сначала `<file>.1` (прежний `.1` заменяется).
fn append(path: &Path, line: &str, cap: u64) -> std::io::Result<()> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len > 0 && len + line.len() as u64 > cap {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        std::fs::rename(path, PathBuf::from(old))?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// Ключи, значения которых не пишутся (без учёта регистра, ищутся как часть имени ключа).
const SECRET_KEYS: [&str; 8] = ["password", "passwd", "token", "cookie", "secret", "passphrase", "auth-code", "authcode"];

pub fn scrub(s: &str, home: Option<&str>) -> String {
    let s = match home {
        Some(h) => s.replace(h, "~"),
        None => s.to_string(),
    };
    redact_secrets(&mask_emails(&s))
}

fn email_char(c: char) -> bool {
    c.is_alphanumeric() || "._%+-".contains(c)
}

/// Каждый `x@y.z` — маской `mask_email`.
fn mask_emails(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' {
            let mut l = i;
            while l > 0 && email_char(chars[l - 1]) {
                l -= 1;
            }
            let mut r = i + 1;
            while r < chars.len() && (chars[r].is_alphanumeric() || chars[r] == '.' || chars[r] == '-') {
                r += 1;
            }
            let domain: String = chars[i + 1..r].iter().collect();
            let domain = domain.trim_end_matches('.');
            if l < i && domain.contains('.') {
                let user_len = i - l;
                // локальная часть уже в `out`: заменить её маской
                let keep: String = out.chars().take(out.chars().count() - user_len).collect();
                let email: String = chars[l..i].iter().collect::<String>() + "@" + domain;
                out = keep + &crate::device::mask_email(&email);
                i += 1 + domain.chars().count();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `"…password…": "x"`, `…token…=x`, `--password x` — значение заменяется `***`.
fn redact_secrets(s: &str) -> String {
    let lower = s.to_ascii_lowercase(); // те же байтовые смещения
    let mut cut: Vec<(usize, usize)> = vec![];
    for key in SECRET_KEYS {
        let mut from = 0;
        while let Some(p) = lower[from..].find(key) {
            let at = p + from;
            let mut j = at + key.len();
            from = j;
            let b = lower.as_bytes();
            // остаток имени ключа: `passwordToken`, `token_x`
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'-') {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                j += 1;
            }
            let sep_start = j;
            while j < b.len() && matches!(b[j], b' ' | b':' | b'=') {
                j += 1;
            }
            // `key: x` / `key=x`; через пробел — только флаг `--key x`
            let flag = at >= 2 && &b[at - 2..at] == b"--";
            let sep = &b[sep_start..j];
            if j >= b.len() || sep.is_empty() || !(sep.contains(&b':') || sep.contains(&b'=') || flag) {
                continue;
            }
            let (start, end) = if b[j] == b'"' {
                let start = j + 1;
                let mut e = start;
                while e < b.len() && b[e] != b'"' {
                    e += if b[e] == b'\\' { 2 } else { 1 };
                }
                (start, e.min(b.len()))
            } else {
                let mut e = j;
                while e < b.len() && !matches!(b[e], b' ' | b',' | b';' | b'&' | b'}' | b'\n') {
                    e += 1;
                }
                (j, e)
            };
            if end > start {
                cut.push((start, end));
            }
        }
    }
    cut.sort();
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for (start, end) in cut {
        if start < at {
            continue;
        }
        out.push_str(&s[at..start]);
        out.push_str("***");
        at = end;
    }
    out.push_str(&s[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_masked_everywhere() {
        assert_eq!(scrub("login user@example.com failed", None), "login us***@example.com failed");
        assert_eq!(scrub("{\"email\":\"user@example.com\"}", None), "{\"email\":\"us***@example.com\"}");
        assert_eq!(scrub("a@b and c.d@example.com.", None), "a@b and c.***@example.com.");
        assert_eq!(scrub("no address @ here", None), "no address @ here");
        assert_eq!(scrub("юзер@example.com", None), "юз***@example.com");
    }

    #[test]
    fn home_dir_is_hidden() {
        assert_eq!(scrub("open /srv/profile-x/Мои приложения/x.ipa", Some("/srv/profile-x")), "open ~/Мои приложения/x.ipa");
    }

    #[test]
    fn secret_values_are_redacted() {
        let s = scrub(r#"{"passwordToken":"abc123","dsid":"1","Cookie": "k=v; x=y"}"#, None);
        assert!(!s.contains("abc123") && !s.contains("k=v"), "{s}");
        assert!(s.contains(r#""dsid":"1""#), "{s}");
        assert_eq!(scrub("--password hunter2 rest", None), "--password *** rest");
        assert_eq!(scrub("auth-code=123456&x=1", None), "auth-code=***&x=1");
        assert_eq!(scrub("token check: network down", None), "token check: network down");
    }

    #[test]
    fn log_is_size_capped_with_one_old_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join(FILE);
        append(&p, "aaaaaaaaaa\n", 25).unwrap();
        append(&p, "bbbbbbbbbb\n", 25).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaaaaaaaaa\nbbbbbbbbbb\n");
        append(&p, "cccccccccc\n", 25).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "cccccccccc\n");
        assert_eq!(std::fs::read_to_string(t.path().join("app.log.1")).unwrap(), "aaaaaaaaaa\nbbbbbbbbbb\n");
        append(&p, "dddddddddd\n", 25).unwrap();
        append(&p, "eeeeeeeeee\n", 25).unwrap();
        assert_eq!(std::fs::read_to_string(t.path().join("app.log.1")).unwrap(), "cccccccccc\ndddddddddd\n");
    }
}
