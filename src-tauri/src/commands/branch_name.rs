use std::path::Path;

#[tauri::command]
pub async fn generate_branch_name(
    prompt: String,
    project_path: String,
) -> Result<String, String> {
    Ok(crate::branch_name::generate_ai_name(&prompt, Path::new(&project_path)).await)
}

#[tauri::command]
pub fn generate_random_branch_name(project_path: String) -> String {
    crate::branch_name::generate_random_name(Path::new(&project_path))
}
