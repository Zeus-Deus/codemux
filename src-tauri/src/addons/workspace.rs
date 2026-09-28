use super::{ErrorCode, ProtocolError, Result};
use serde::Serialize;
use std::path::PathBuf;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub root_name: String,
    pub location: &'static str,
    #[serde(skip)]
    pub root: PathBuf,
}
pub fn current(state: &crate::state::AppStateStore, id: &str) -> Result<Workspace> {
    let workspace = state
        .snapshot()
        .workspaces
        .into_iter()
        .find(|w| w.workspace_id.0 == id)
        .ok_or_else(|| ProtocolError::new(ErrorCode::NoWorkspace, "No workspace is open"))?;
    if workspace.host_id.is_some() || workspace.remote_cwd.is_some() || workspace.attach_only {
        return Err(ProtocolError::new(
            ErrorCode::RemoteUnsupported,
            "Add-ons are unavailable in remote workspaces",
        ));
    }
    let root = std::fs::canonicalize(workspace.worktree_path.as_ref().unwrap_or(&workspace.cwd))
        .map_err(|_| {
            ProtocolError::new(ErrorCode::NoWorkspace, "Workspace directory is unavailable")
        })?;
    Ok(Workspace {
        id: id.into(),
        name: workspace.title,
        root_name: root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        location: "local",
        root,
    })
}
/// Private storage namespace for one project. AppState workspace IDs are
/// process counters that a later project can receive again, so the scope is
/// derived from the authorized canonical root instead. It never leaves Rust.
pub fn storage_scope(workspace: &Workspace) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "workspace-root:{:x}",
        Sha256::digest(workspace.root.as_os_str().as_encoded_bytes())
    )
}
