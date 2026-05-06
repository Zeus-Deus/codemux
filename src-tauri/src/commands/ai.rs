use std::path::Path;

#[tauri::command]
pub fn check_claude_available() -> bool {
    crate::ai::claude_available()
}

#[tauri::command]
pub async fn generate_ai_commit_message(
    path: String,
    cli: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    // Default to claude when the caller doesn't pass a CLI — keeps
    // the historical single-CLI behavior for any callers that haven't
    // been updated to pass the new arg yet.
    let cli = cli.as_deref().unwrap_or("claude");
    crate::ai::generate_commit_message(Path::new(&path), cli, model.as_deref()).await
}
