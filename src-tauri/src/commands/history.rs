//! «Найти всё, что вы скачивали»: окно `history` с reportaproblem.apple.com. Человек входит сам
//! (пароль программа не видит). Скрипт окна (`src/history-page.js`) читает ответы страницы
//! `purchase/search` и сам догружает пачки внутри страницы; сюда через `plugin:history|items`
//! приходят только номера и названия приложений. Токены, cookie и заголовки сайта страницу не
//! покидают: Rust их не читает, не пишет в журнал и в файлы. Хранилище окна — не сохраняемое
//! (`incognito`: nonPersistent на macOS), закрыли окно — сессии сайта нет.
//!
//! События (только главному окну): `history_progress` {found}, `history` {rows, found, error}.
//! Окно ходит только на https *.apple.com; окно авторизации использует ту же проверку переходов.
use super::AppState;
use super::logic::{RowDto, UiError, history_row};
use crate::history::{Collected, PageBatch, Problem, allowed_navigation, fresh, timed_out};
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Webview};

pub const LABEL: &str = "history";
const AUTH_LABEL: &str = "history-auth";
const URL: &str = "https://reportaproblem.apple.com/";
const SCRIPT: &str = include_str!("../../../src/history-page.js");
/// События истории — только сюда, не в окно Apple.
const MAIN: &str = "main";
struct Run {
    round: u64,
    /// Apple ID, при котором окно открыли: найденное запоминается только за ним.
    email: Option<String>,
    collected: Collected,
    opened: Instant,
    last_batch: Option<Instant>,
    ready: bool,
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
    email: Option<String>,
    rows: Vec<RowDto>,
    /// Сколько разных приложений iPhone нашлось в истории (и уже известных тоже).
    found: usize,
    error: Option<String>,
}

/// Открыть окно истории (или показать уже открытое).
#[tauri::command]
pub async fn open_history(app: AppHandle) -> Result<(), UiError> {
    if let Some(w) = app.get_webview_window(AUTH_LABEL).or_else(|| app.get_webview_window(LABEL)) {
        let _ = w.set_focus();
        return Ok(());
    }
    let hr = app.state::<HistoryRun>();
    let email = app.state::<AppState>().current_email();
    let round = {
        let mut run = hr.run.lock().unwrap();
        if run.is_some() {
            return Ok(());
        }
        let mut g = hr.round.lock().unwrap();
        *g += 1;
        *run = Some(Run { round: *g, email, collected: Collected::default(), opened: Instant::now(), last_batch: None, ready: false });
        *g
    };
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
            // У входа свой window.opener: страница входа может закрыть своё окно после авторизации.
            .on_new_window({
                let app = app.clone();
                move |url, features| {
                    if !allowed_navigation(&url) {
                        return NewWindowResponse::Deny;
                    }
                    let child = WebviewWindowBuilder::new(&app, AUTH_LABEL, WebviewUrl::External("about:blank".parse().unwrap()))
                        .window_features(features)
                        .title("Вход в Apple ID")
                        .incognito(true)
                        .on_navigation(allowed_navigation)
                        .on_new_window(|_, _| NewWindowResponse::Deny)
                        .build();
                    match child {
                        Ok(window) => NewWindowResponse::Create { window },
                        Err(_) => NewWindowResponse::Deny,
                    }
                }
            })
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
                    Some(r) if r.round == round && r.ready => None,
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
        r.ready = done && error.is_none();
        r.collected.add(items.0);
        r.last_batch = Some(Instant::now());
        (r.round, r.collected.items.len(), r.collected.full(), r.collected.over_limit())
    };
    let _ = app.emit_to(MAIN, "history_progress", Progress { found });
    let problem = error.as_deref().map(Problem::from_page);
    // пачек сверх MAX_BATCHES не читаем — и окно не держим открытым пустыми вызовами
    if full || over || problem.is_some() {
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
    if let Some(auth) = app.get_webview_window(AUTH_LABEL) {
        let _ = auth.close();
    }
    let user_closed = problem == Some(Problem::Closed);
    let problem = if user_closed && run.ready { None } else { problem };
    if !user_closed && let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
    }
    // итог — в главном окне: оно выходит вперёд, а не остаётся за другими программами
    if let Some(main) = app.get_webview_window(MAIN) {
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
    let found = run.collected.items.len();
    let st = app.state::<AppState>();
    let known = st.on_screen().await;
    let new = fresh(&run.collected.items, &known);
    // всё найденное теперь на «Выборе»: повтор не посчитает его новым, устаревшее перепроверится
    st.screen_extra.lock().unwrap().extend(run.collected.items.iter().map(|f| f.id));
    // и между запусками: за тем Apple ID, при котором окно открыли (сменили вход посреди — не пишем)
    if let (Some(at), Some(now)) = (run.email.as_deref(), st.current_email())
        && crate::accounts::same_id(at, &now)
    {
        st.history_store.remember(at, &run.collected.items);
    }
    // в журнал — только счётчики и вид ошибки, без названий и номеров
    crate::log::write("history", &format!("найдено {found}, новых {}, итог {problem:?}", new.len()));
    let rows = st.with_ctx(|ctx| run.collected.items.iter().map(|f| history_row(f, ctx)).collect()).await;
    let _ = app.emit_to(MAIN, "history", Outcome { email: run.email, rows, found, error: problem.map(|p| p.text(found)) });
}
