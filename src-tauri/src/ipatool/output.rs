
//! Разбор stdout ipatool (JSON-строки) в результат или понятную ошибку.
use crate::errors::AppError;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct OwnedApp {
    pub id: i64,
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IpaOutcome {
    LoggedIn { email: String, name: String },
    Purchases { apps: Vec<OwnedApp>, count: u32, total: u32, page: u32 },
    Downloaded { path: PathBuf },
    /// Успешный `list-versions`: права на приложение есть.
    Versions { ids: Vec<String> },
    /// Успешный `get-version-metadata`: версия для людей («12.15.0») и дата выхода
    /// (`releaseDate`, «2022-03-18T06:23:40Z»). Минимальной iOS ipatool 2.7.0 не отдаёт.
    VersionMetadata { display_version: String, release_date: Option<String> },
    Ok,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn status_of(e: &str) -> Option<u16> {
    let e = e.to_lowercase();
    let mut start = 0;
    while start < e.len() {
        let rest = &e[start..];
        let Some(off) = rest.find(|c: char| c.is_ascii_digit()) else { break };
        let b = start + off;
        let len = e[b..].find(|c: char| !c.is_ascii_digit()).unwrap_or(e.len() - b);
        if len == 3
            && let Ok(n) = e[b..b + 3].parse::<u16>()
                && (n == 429 || (500..=599).contains(&n)) {
                    let from = e[..b].char_indices().rev().nth(19).map(|(i, _)| i).unwrap_or(0);
                    let ctx = &e[from..b];
                    if ["status", "code", "http"].iter().any(|w| ctx.contains(w)) {
                        return Some(n);
                    }
                }
        start = b + len;
    }
    None
}

/// Дата из «invalid release date: 2022-04-04T11:59:04»: только вида `YYYY-MM-DD…`.
fn broken_date(raw: &str) -> Option<String> {
    const P: &str = "invalid release date:";
    let at = raw.find(P)? + P.len();
    let d: String = raw[at..].trim_start().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | 'T' | ':' | '.' | 'Z' | '+')).collect();
    let b = d.as_bytes();
    let ok = b.len() >= 10 && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    ok.then_some(d)
}

fn map_error(raw: &str) -> AppError {
    let e = raw.to_lowercase();
    let has = |p: &[&str]| p.iter().any(|x| e.contains(x));
    // метаданные версии не разобрались — это не сбой входа и не «Apple не отвечает»
    if has(&["failed to parse release date", "invalid release date", "failed to read remote file size"]) {
        return AppError::VersionMetaBroken { released: broken_date(raw) };
    }
    if has(&["auth code is required", "2fa code is required"]) {
        AppError::AuthCodeRequired
    } else if has(&["2fa", "verification", "auth code", "authcode"]) {
        AppError::CodeRejected
    } else if has(&["keyring", "not logged in"]) {
        AppError::NotLoggedIn
    } else if has(&["license is required"]) {
        AppError::NotPurchased
    } else if has(&["account is disabled", "account has been disabled"]) {
        AppError::AccountDisabled
    } else if has(&["something went wrong", "unsupported authentication redirect"]) {
        AppError::AppleChangedLogin
    } else if has(&["platform version lookup returned no app", "catalog lookup returned http 404"]) {
        // до `status_of`: пропавшее у Apple — не «серверы не отвечают», даже с кодом в тексте
        AppError::AppGone
    } else if let Some(st) = status_of(&e) {
        AppError::AppleUnavailable { status: Some(st) }
    } else if has(&["too many requests", "service unavailable"]) {
        AppError::AppleUnavailable { status: None }
    } else if has(&["password is required", "invalid credentials", "incorrect", "password"]) {
        AppError::WrongCredentials
    } else {
        AppError::Unknown { detail: raw.to_string() }
    }
}

fn success_outcome(v: &Value) -> IpaOutcome {
    if let Some(ids) = v.get("externalVersionIdentifiers").and_then(Value::as_array) {
        IpaOutcome::Versions { ids: ids.iter().filter_map(|x| x.as_str().map(String::from)).collect() }
    } else if v.get("displayVersion").is_some() {
        let date = s(v, "releaseDate");
        IpaOutcome::VersionMetadata { display_version: s(v, "displayVersion"), release_date: (!date.trim().is_empty()).then_some(date) }
    } else if let Some(out) = v.get("output").and_then(Value::as_str) {
        IpaOutcome::Downloaded { path: PathBuf::from(out) }
    } else if v.get("email").is_some() {
        IpaOutcome::LoggedIn { email: s(v, "email"), name: s(v, "name") }
    } else if v.get("apps").is_some() {
        purchases(v)
    } else {
        IpaOutcome::Ok
    }
}

fn purchases(v: &Value) -> IpaOutcome {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    let apps = v
        .get("apps")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| OwnedApp {
                    id: x.get("id").and_then(Value::as_i64).unwrap_or(0),
                    bundle_id: s(x, "bundleID"),
                    name: s(x, "name"),
                    version: s(x, "version"),
                    platforms: x
                        .get("platforms")
                        .and_then(Value::as_array)
                        .map(|p| p.iter().filter_map(|y| y.as_str().map(String::from)).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    IpaOutcome::Purchases { apps, count: n("count"), total: n("totalCount"), page: n("page") }
}

/// Берёт последнюю JSON-строку с полем `success`. Без неё (ipatool так отвечает
/// на запрос 2FA и на list-purchases) ищет список приложений или просьбу о коде.
/// Код выхода не решает: AuthCodeRequired приходит с exit 0.
pub fn parse_output(stdout: &str, exit_code: i32) -> Result<IpaOutcome, AppError> {
    let lines: Vec<Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(Value::is_object)
        .collect();

    if let Some(v) = lines.iter().rev().find(|v| v.get("success").is_some()) {
        return if v.get("success").and_then(Value::as_bool) == Some(true) {
            Ok(success_outcome(v))
        } else {
            Err(map_error(&s(v, "error")))
        };
    }
    if let Some(v) = lines.iter().rev().find(|v| v.get("apps").is_some()) {
        return Ok(purchases(v));
    }
    if lines.iter().any(|v| s(v, "message").to_lowercase().contains("2fa code is required")) {
        return Err(AppError::AuthCodeRequired);
    }
    let detail = stdout.trim();
    Err(AppError::Unknown {
        detail: if detail.is_empty() {
            format!("ipatool завершился с кодом {exit_code} без вывода")
        } else {
            detail.chars().take(300).collect()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::AppError;

    fn fx(name: &str) -> (String, i32) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ipatool/");
        let out = std::fs::read_to_string(format!("{dir}{name}.jsonl")).unwrap();
        let code = std::fs::read_to_string(format!("{dir}{name}.exit")).unwrap().trim().parse().unwrap();
        (out, code)
    }
    fn parse_fx(name: &str) -> Result<IpaOutcome, AppError> {
        let (o, c) = fx(name);
        parse_output(&o, c)
    }
    fn err_line(e: &str) -> String {
        format!("{{\"level\":\"error\",\"error\":{e:?},\"success\":false}}\n")
    }

    #[test]
    fn auth_code_required_even_with_exit_0() {
        for n in ["login-no-code", "login-no-code-2", "login-wrong-password"] {
            assert_eq!(parse_fx(n), Err(AppError::AuthCodeRequired), "{n}");
        }
        assert_eq!(parse_output(&err_line("auth code is required"), 1), Err(AppError::AuthCodeRequired));
    }

    #[test]
    fn wrong_credentials() {
        assert_eq!(parse_fx("login-no-password"), Err(AppError::WrongCredentials));
        assert_eq!(parse_output(&err_line("invalid credentials"), 1), Err(AppError::WrongCredentials));
    }

    #[test]
    fn code_rejected() {
        assert_eq!(parse_fx("login-wrong-code"), Err(AppError::CodeRejected));
        assert_eq!(
            parse_output(&err_line("2FA code must contain exactly six digits"), 1),
            Err(AppError::CodeRejected)
        );
    }

    #[test]
    fn not_purchased_and_logged_out() {
        assert_eq!(parse_fx("download-not-owned"), Err(AppError::NotPurchased));
        assert_eq!(parse_fx("purchases-logged-out"), Err(AppError::NotLoggedIn));
    }

    #[test]
    fn account_disabled() {
        assert_eq!(parse_output(&err_line("account is disabled"), 1), Err(AppError::AccountDisabled));
    }

    #[test]
    fn apple_unavailable_with_status() {
        for n in [429u16, 500, 503] {
            let l = err_line(&format!("request failed: unexpected status code {n} from store"));
            assert_eq!(parse_output(&l, 1), Err(AppError::AppleUnavailable { status: Some(n) }));
        }
    }

    #[test]
    fn bare_port_number_is_not_a_status() {
        let l = err_line("dial tcp 198.51.100.1 port 500 refused");
        assert!(matches!(parse_output(&l, 1), Err(AppError::Unknown { .. })));
    }

    #[test]
    fn apple_changed_login() {
        for t in ["something went wrong", "unsupported authentication redirect status"] {
            assert_eq!(parse_output(&err_line(t), 1), Err(AppError::AppleChangedLogin), "{t}");
        }
    }

    #[test]
    fn success_cases() {
        match parse_fx("login-ok") {
            Ok(IpaOutcome::LoggedIn { email, name }) => {
                assert_eq!(email, "user@example.com");
                assert_eq!(name, "Test User");
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(parse_fx("auth-info"), Ok(IpaOutcome::LoggedIn { .. })));
        assert_eq!(parse_fx("auth-revoke"), Ok(IpaOutcome::Ok));
        match parse_fx("purchases-p1") {
            Ok(IpaOutcome::Purchases { apps, count, total, page }) => {
                assert_eq!((count, total, page), (5, 741, 1));
                assert_eq!(apps.len(), 5);
                assert_eq!(apps[0].id, 492224193);
                assert_eq!(apps[0].bundle_id, "ru.sberbankmobile");
                assert_eq!(apps[0].platforms, vec!["iphone", "ipad", "macos"]);
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(parse_fx("purchases-p2"), Ok(IpaOutcome::Purchases { page: 2, .. })));
        match parse_fx("download-owned") {
            Ok(IpaOutcome::Downloaded { path }) => {
                assert_eq!(
                    path,
                    std::path::PathBuf::from("fixtures-raw/dl/net.hildebrand-it.amperfy-music_1530145038_2.1.1.ipa")
                )
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn list_versions_and_removed_app_fixtures() {
        match parse_fx("list-versions-owned") {
            Ok(IpaOutcome::Versions { ids }) => assert!(!ids.is_empty() && ids[0] == "5730143"),
            o => panic!("{o:?}"),
        }
        assert_eq!(parse_fx("list-versions-not-owned"), Err(AppError::NotPurchased));
        assert!(matches!(parse_fx("download-removed-by-bundle"), Err(AppError::Unknown { .. })));
        match parse_fx("download-by-id") {
            Ok(IpaOutcome::Downloaded { path }) => {
                assert_eq!(path, std::path::PathBuf::from("fixtures-raw/restore/492224193_12.15.0.ipa"))
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn app_gone_live_text_fixture() {
        // живой ответ 27.10 (owns 1466584968): HTTP 404 — не «Apple не отвечает» и не Unknown
        assert_eq!(parse_fx("owns-app-gone"), Err(AppError::AppGone));
        for t in ["platform version lookup returned no app", "iOS catalog lookup returned HTTP 404"] {
            assert_eq!(parse_output(&err_line(t), 1), Err(AppError::AppGone), "{t}");
        }
        // 5xx в том же тексте — всё равно приложение пропало, а не сбой Apple
        let l = err_line("platform version lookup returned no app; fallback: unexpected status code 503");
        assert_eq!(parse_output(&l, 1), Err(AppError::AppGone));
    }

    #[test]
    fn version_metadata_fixture() {
        assert_eq!(parse_fx("version-metadata"), Ok(IpaOutcome::VersionMetadata { display_version: "12.15.0".into(), release_date: Some("2022-03-18T06:23:40Z".into()) }));
        assert_eq!(
            parse_output("{\"displayVersion\":\"1.0\",\"success\":true}\n", 0),
            Ok(IpaOutcome::VersionMetadata { display_version: "1.0".into(), release_date: None })
        );
    }

    #[test]
    fn garbage_is_unknown() {
        assert!(matches!(parse_output("segfault: core dumped\n", 139), Err(AppError::Unknown { .. })));
        assert!(matches!(parse_output("", 1), Err(AppError::Unknown { .. })));
        assert!(matches!(parse_output(&err_line("weird new thing"), 1), Err(AppError::Unknown { .. })));
    }

    #[test]
    fn broken_version_metadata_is_its_own_error() {
        // живой журнал: ipatool не принимает дату без «Z» и не всегда может прочесть размер
        let date = err_line("failed to parse release date: invalid release date: 2022-04-04T11:59:04");
        assert_eq!(parse_output(&date, 1), Err(AppError::VersionMetaBroken { released: Some("2022-04-04T11:59:04".into()) }));
        let size = err_line("failed to read remote file size");
        assert_eq!(parse_output(&size, 1), Err(AppError::VersionMetaBroken { released: None }));
        let odd = err_line("failed to parse release date: invalid release date: soon");
        assert_eq!(parse_output(&odd, 1), Err(AppError::VersionMetaBroken { released: None }));
    }
}
