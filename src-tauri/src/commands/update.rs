#[tauri::command]
pub fn get_package_format() -> String {
    if std::env::var("APPIMAGE").is_ok() {
        "appimage".to_string()
    } else {
        "other".to_string()
    }
}
