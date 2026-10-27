//! «Найти всё, что вы скачивали»: окно `history` с reportaproblem.apple.com. Человек входит сам
//! (пароль программа не видит). Скрипт окна (`src/history-page.js`) читает ответы страницы
//! `purchase/search` и сам догружает пачки внутри страницы; сюда через `plugin:history|items`
//! приходят только номера и названия приложений. Токены, cookie и заголовки сайта страницу не
//! покидают: Rust их не читает, не пишет в журнал и в файлы. Хранилище окна — не сохраняемое
//! (`incognito`: nonPersistent на macOS), закрыли окно — сессии сайта нет.
//!
//! События (только главному окну): `history_progress` {found}, `history` {rows, found, error}.
//! Окно ходит и открывает новые окна только на https *.apple.com.
use super::AppState;
use super::logic::{RowCtx, RowDto, UiError};
use crate::history::{Collected, Found, PageBatch, Problem, allowed_navigation, allowed_url, fresh, timed_out};
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Webview};

pub const LABEL: &str = "history";
const URL: &str = "https://reportaproblem.apple.com/";
const SCRIPT: &str = include_str!("../../../src/history-page.js");
/// События истории — только сюда, не в окно Apple.
const MAIN: &str = "main";
/// Сколько видна плашка «Готово, найдено N» перед закрытием окна.
const DONE_PAUSE: Duration = Duration::from_millis(1500);

struct Run {
    round: u64,
    collected: Collected,
    opened: Instant,
    last_batch: Option<Instant>,
}

/// Идущий заход (`None` — окна нет или итог уже отдан).
#[derive(Default)]
pub struct HistoryRun {
    run: Mutex<Option<Run>>,
    round: Mutex<u64>,
}

#[derive(Clone, Serialize)]
struct Progress {
    found: usize,
}

#[derive(Clone, Serialize)]
struct Outcome {
    rows: Vec<RowDto>,
    /// Сколько разных приложений iPhone нашлось в истории (и уже известных тоже).
    found: usize,
    error: Option<String>,
}

/// Строка «Из истории покупок»: права проверит `check_owned`, сама не отмечается.
fn history_row(f: &Found, ctx: &RowCtx) -> RowDto {
    let track_id = f.id;
    let available_version = ctx.latest.get(&track_id).cloned();
    let installed_version = ctx.installed.and_then(|list| crate::catalog::find_installed(list, track_id, "")).map(|a| a.version.clone());
    RowDto {
        track_id,
        has_saved_copy: crate::catalog::find_saved(ctx.saved, track_id, "").is_some(),
        bundle_id: String::new(),
        name: f.name.clone().unwrap_or_else(|| format!("Приложение {track_id}")),
        version: String::new(),
        removed_from_ru: false,
        owned: ctx.owned.get(&track_id).copied(),
        source: "history",
        letter: None,
        color: None,
        install_state: crate::catalog::install_state(installed_version.as_deref(), available_version.as_deref()),
        installed_version,
        available_version,
        checked: false,
        ipad_only: false,
        unknown_account: false,
        aliases: vec![],
        category: None,
        size_bytes: ctx.sizes.get(&track_id).copied(),
        other_account: false,
        purchaser_masked: None,
        min_ios: None,
    }
}

/// Открыть окно истории (или показать уже открытое).
#[tauri::command]
pub async fn open_history(app: AppHandle) -> Result<(), UiError> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_focus();
        return Ok(());
    }
    let hr = app.state::<HistoryRun>();
    let round = {
        let mut g = hr.round.lock().unwrap();
        *g += 1;
        *g
    };
    *hr.run.lock().unwrap() = Some(Run { round, collected: Collected::default(), opened: Instant::now(), last_batch: None });
    // хранилище окна не сохраняется (nonPersistent на macOS): закрыли — сессии сайта нет
    let built = URL.parse().map_err(|e| format!("{e}")).and_then(|url| {
        WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::External(url))
            .title("История покупок Apple — войдите своим Apple ID")
            .inner_size(960.0, 720.0)
            .incognito(true)
            .initialization_script(SCRIPT)
            // ссылки Apple наружу и редиректы: окно — не браузер, чужие сайты не открываются
            .on_navigation(|url| {
                let ok = allowed_navigation(url);
                if !ok {
                    crate::log::write("history", "переход вне apple.com отклонён");
                }
                ok
            })
            .on_new_window(|url, _| if allowed_url(&url) { NewWindowResponse::Allow } else { NewWindowResponse::Deny })
            .build()
            .map_err(|e| format!("{e}"))
    });
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            *hr.run.lock().unwrap() = None;
            crate::log::write("history", &format!("окно не открылось: {e}"));
            return Err(UiError { code: "history_window".into(), text: "Не удалось открыть окно истории покупок. Попробуйте ещё раз.".into(), retryable: true, ref_code: None });
        }
    };
    crate::log::write("history", "окно открыто");
    let h = app.clone();
    window.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            let h = h.clone();
            tauri::async_runtime::spawn(async move { finish(&h, round, Some(Problem::Closed)).await });
        }
    });
    // не вошли или пачки перестали идти — закрываем с понятной ошибкой
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let verdict = {
                let hr = h.state::<HistoryRun>();
                let run = hr.run.lock().unwrap();
                match run.as_ref() {
                    Some(r) if r.round == round => timed_out(r.opened.elapsed(), r.last_batch.map(|t| t.elapsed())),
                    _ => return,
                }
            };
            if let Some(p) = verdict {
                finish(&h, round, Some(p)).await;
                return;
            }
        }
    });
    Ok(())
}

/// Пачка от страницы окна `history` (remote-capability только для reportaproblem.apple.com).
/// `error` — код страницы (`api_changed`, `http`); любой другой — как `api_changed`.
#[tauri::command]
pub async fn items(app: AppHandle, webview: Webview, items: PageBatch, done: bool, error: Option<String>) -> Result<(), ()> {
    if webview.label() != LABEL {
        return Err(());
    }
    let hr = app.state::<HistoryRun>();
    let (round, found, full, over) = {
        let mut run = hr.run.lock().unwrap();
        let Some(r) = run.as_mut() else { return Ok(()) };
        r.collected.add(items.0);
        r.last_batch = Some(Instant::now());
        (r.round, r.collected.items.len(), r.collected.full(), r.collected.over_limit())
    };
    let _ = app.emit_to(MAIN, "history_progress", Progress { found });
    let problem = error.as_deref().map(Problem::from_page);
    // пачек сверх MAX_BATCHES не читаем — и окно не держим открытым пустыми вызовами
    if done || full || over || problem.is_some() {
        finish(&app, round, problem).await;
    }
    Ok(())
}

/// Итог захода: окно закрывается, на «Выбор» уходят строки, которых там ещё нет.
async fn finish(app: &AppHandle, round: u64, problem: Option<Problem>) {
    let run = {
        let hr = app.state::<HistoryRun>();
        let mut run = hr.run.lock().unwrap();
        if run.as_ref().map(|r| r.round) != Some(round) {
            return; // итог уже отдан (или это прежний заход)
        }
        run.take().unwrap()
    };
    if let Some(w) = app.get_webview_window(LABEL) {
        // человек сам закрыл окно — ждать нечего; иначе дать прочесть «Готово, найдено N»
        if problem != Some(Problem::Closed) {
            tokio::time::sleep(DONE_PAUSE).await;
        }
        let _ = w.close();
    }
    let found = run.collected.items.len();
    let st = app.state::<AppState>();
    let known = st.on_screen().await;
    let new = fresh(&run.collected.items, &known);
    // в журнал — только счётчики и вид ошибки, без названий и номеров
    crate::log::write("history", &format!("найдено {found}, новых {}, итог {problem:?}", new.len()));
    let rows = st.with_ctx(|ctx| new.iter().map(|f| history_row(f, ctx)).collect()).await;
    let _ = app.emit_to(MAIN, "history", Outcome { rows, found, error: problem.map(|p| p.text(found)) });
}
