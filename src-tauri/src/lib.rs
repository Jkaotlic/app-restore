mod accounts;
mod app_link;
mod commands;
mod catalog;
mod device;
mod dropped;
mod errors;
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
        .setup(|app| {
            // macOS: ~/Library/Logs/<identifier>/, Windows: %LOCALAPPDATA%\<identifier>\logs
            if let Ok(dir) = app.path().app_log_dir() {
                log::init(dir);
            }
            log::write("start", &format!("v{} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
            commands::setup(app.handle());
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
            commands::start,
            commands::run_info,
            commands::list_phones,
            commands::choose_phone,
            commands::retry,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
