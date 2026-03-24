use std::path::Path;

#[tauri::command]
pub fn check_claude_available() -> bool {
    crate::ai::claude_available()
}

#[tauri::command]
pub async fn generate_ai_commit_message(
    path: String,
    model: Option<String>,
) -> Result<String, String> {
    crate::ai::generate_commit_message(Path::new(&path), model.as_deref()).await
}
