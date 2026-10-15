use std::path::Path;

use crate::utility_ai::UtilityModelSelection;

/// `utility_selection` is the frontend's resolved Utility agent. Naming runs
/// only on that model; `None` gets a random name instead of a default model.
#[tauri::command]
pub async fn generate_branch_name(
    prompt: String,
    project_path: String,
    utility_selection: Option<UtilityModelSelection>,
) -> Result<String, String> {
    Ok(crate::branch_name::generate_ai_name(
        &prompt,
        Path::new(&project_path),
        utility_selection.as_ref(),
    )
    .await)
}

#[tauri::command]
pub fn generate_random_branch_name(project_path: String) -> String {
    crate::branch_name::generate_random_name(Path::new(&project_path))
}

/// Human-readable sidebar title for a workspace, from its first message.
/// `None` when no Utility model is resolved or generation fails.
#[tauri::command]
pub async fn generate_workspace_title(
    prompt: String,
    utility_selection: Option<UtilityModelSelection>,
) -> Option<String> {
    crate::branch_name::generate_ai_title(&prompt, utility_selection.as_ref()).await
}
