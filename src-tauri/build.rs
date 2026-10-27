fn main() {
    // `history` — команда страницы reportaproblem.apple.com (remote-capability `history`):
    // отдельный плагин, чтобы у команд приложения не появился ACL-манифест.
    tauri_build::try_build(
        tauri_build::Attributes::new().plugin("history", tauri_build::InlinedPlugin::new().commands(&["items"])),
    )
    .expect("failed to run tauri-build");
}
