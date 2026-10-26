mod accounts;
mod app_link;
mod commands;
mod catalog;
mod device;
mod errors;
mod ipatool;
mod library;
mod lookup;
mod queue;
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
        .setup(|app| {
            commands::setup(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::login,
            commands::session_info,
            commands::logout,
            commands::refresh_catalog,
            commands::load_catalog_rows,
            commands::load_purchases,
            commands::installed_apps,
            commands::phone_account,
            commands::switch_account,
            commands::clear_installed,
            commands::check_owned,
            commands::add_by_link,
            commands::start,
            commands::run_info,
            commands::list_phones,
            commands::choose_phone,
            commands::retry,
            commands::check_update,
            commands::open_library,
            commands::list_accounts,
            commands::use_account,
            commands::forget_account,
            commands::all_copies,
            commands::pick_ipa,
            commands::install_copy,
            commands::copy_account_match,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
