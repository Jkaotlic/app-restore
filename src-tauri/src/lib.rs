mod accounts;
mod app_link;
mod commands;
mod catalog;
mod device;
mod dropped;
mod errors;
mod ios_compat;
mod history;
mod history_store;
mod ipatool;
mod library;
mod log;
mod owned_cache;
mod lookup;
mod queue;
mod releases;
mod removed_catalog;
mod retry;
mod passphrase;
mod pending_jobs;
mod update_check;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        // первым: второй экземпляр выходит до `setup` (уборки .part и миграции), окно первого — вперёд
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        // уведомления шлёт только Rust (`commands::notify`): в capabilities интерфейса их нет
        .plugin(tauri_plugin_notification::init())
        // «История покупок Apple»: единственная команда, доступная странице reportaproblem.apple.com
        // (capabilities/history.json), — в отдельном плагине, чтобы остальные команды ей были закрыты
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("history")
                .invoke_handler(tauri::generate_handler![commands::history::items])
                .build(),
        )
        .manage(commands::history::HistoryRun::default())
        .setup(|app| {
            // macOS: ~/Library/Logs/<identifier>/, Windows: %LOCALAPPDATA%\<identifier>\logs
            if let Ok(dir) = app.path().app_log_dir() {
                log::init(dir);
            }
            log::write("start", &format!("v{} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
            // паника — ещё и в журнал: «программа закрылась сама» иначе следов не оставляет
            let default_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                log::write("panic", &info.to_string());
                default_hook(info);
            }));
            commands::setup(app.handle());
            fit_main_window(app);
            Ok(())
        })
        // файлы, перетащенные в окно: пути приходят сюда, интерфейсу доступ к ним не нужен
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                commands::on_drop(window.app_handle(), paths.clone());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::login,
            commands::session_info,
            commands::logout,
            commands::refresh_catalog,
            commands::release_badges,
            commands::open_in_store,
            commands::load_catalog_rows,
            commands::load_purchases,
            commands::installed_apps,
            commands::phone_account,
            commands::switch_account,
            commands::clear_installed,
            commands::check_owned,
            commands::owned_visible,
            commands::check_catalog,
            commands::add_by_link,
            commands::links_in_text,
            commands::search_store,
            commands::app_icons,
            commands::start,
            commands::app_versions,
            commands::find_version,
            commands::run_info,
            commands::list_phones,
            commands::choose_phone,
            commands::retry,
            commands::pause_queue,
            commands::resume_queue,
            commands::queue_paused,
            commands::pending_jobs,
            commands::resume_pending,
            commands::forget_pending,
            commands::check_update,
            commands::open_library,
            commands::open_log,
            commands::list_accounts,
            commands::use_account,
            commands::forget_account,
            commands::all_copies,
            commands::pick_ipa,
            commands::install_copy,
            commands::copy_account_match,
            commands::set_source_phone,
            commands::report_to_catalog,
            commands::history::open_history,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // выход — в журнал: по нему видно, закрыли программу или она пропала без следа
        .run(|_app, e| {
            if let tauri::RunEvent::Exit = e {
                log::write("exit", "программа закрыта");
            }
        });
}

/// Размер окна, который помещается в рабочую область `area`: не больше её и не меньше `min`.
pub fn fit_window(want: (f64, f64), area: (f64, f64), min: (f64, f64)) -> (f64, f64) {
    (want.0.min(area.0).max(min.0), want.1.min(area.1).max(min.1))
}

/// Заголовок окна поверх внутреннего размера (логические px): macOS ~28, Windows ~32.
const WINDOW_FRAME: f64 = 40.0;

/// Окно 1200×780 не влезает в рабочую область монитора (1366×768, 1080p при 150 %): уменьшить до неё,
/// но не меньше минимума из `tauri.conf.json`, и поставить по центру — иначе панель действий уходит за край.
fn fit_main_window(app: &tauri::App) {
    use tauri::Manager;
    let Some(w) = app.get_webview_window("main") else { return };
    let Ok(Some(m)) = w.current_monitor() else { return };
    let cfg = app.config().app.windows.first();
    let want = cfg.map_or((1200.0, 780.0), |c| (c.width, c.height));
    let min = cfg.map_or((960.0, 620.0), |c| (c.min_width.unwrap_or(960.0), c.min_height.unwrap_or(620.0)));
    let s = m.scale_factor();
    let a = m.work_area().size;
    let area = (a.width as f64 / s, a.height as f64 / s - WINDOW_FRAME);
    let fit = fit_window(want, area, min);
    if fit != want {
        let _ = w.set_size(tauri::LogicalSize::new(fit.0, fit.1));
        let _ = w.center();
    }
}

#[cfg(test)]
mod window_tests {
    use super::fit_window;

    #[test]
    fn fits_the_work_area_but_not_below_the_minimum() {
        let (want, min) = ((1200.0, 780.0), (960.0, 620.0));
        // большой экран — как есть
        assert_eq!(fit_window(want, (1920.0, 1040.0), min), want);
        // 1366×768 без заголовка и панели задач
        assert_eq!(fit_window(want, (1366.0, 688.0), min), (1200.0, 688.0));
        // 1080p при 150 %: ≈ 1280×680
        assert_eq!(fit_window(want, (1280.0, 680.0), min), (1200.0, 680.0));
        // меньше минимума не делаем
        assert_eq!(fit_window(want, (800.0, 500.0), min), (960.0, 620.0));
    }
}
