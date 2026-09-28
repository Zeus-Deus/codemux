use super::profile::Profile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub schema_version: u32,
    pub profile: Profile,
    pub thread_id: String,
    pub workspace_id: Option<String>,
    pub cwd: PathBuf,
    pub acp_session_id: Option<String>,
    pub current_native_id: Option<String>,
    pub root_native_id: Option<String>,
    /// None means profile default; never replace it with the resolved model.
    pub model_override: Option<String>,
    pub resolved_model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    pub compatibility: String,
    pub cleanup_pending: bool,
}

impl Binding {
    pub fn provenance(&mut self, value: &Value) -> Result<(), String> {
        let Some(p) = value.pointer("/_meta/hermes/sessionProvenance") else {
            return Ok(());
        };
        let id = p
            .get("acpSessionId")
            .and_then(Value::as_str)
            .ok_or("unsupported: invalid Hermes provenance")?;
        if self
            .acp_session_id
            .as_deref()
            .is_some_and(|expected| expected != id)
        {
            return Err(
                "repair_required: Hermes provenance belongs to a different conversation".into(),
            );
        }
        self.acp_session_id = Some(id.into());
        self.current_native_id = Some(
            p.get("currentHermesSessionId")
                .and_then(Value::as_str)
                .ok_or("unsupported: missing Hermes history head")?
                .into(),
        );
        self.root_native_id = Some(
            p.get("rootHermesSessionId")
                .and_then(Value::as_str)
                .ok_or("unsupported: missing Hermes history root")?
                .into(),
        );
        if self.current_native_id != self.acp_session_id {
            self.compatibility = "unsupported".into();
        }
        Ok(())
    }

    pub fn validate_cwd(&self) -> Result<(), String> {
        if !self.cwd.is_absolute()
            || !self.cwd.is_dir()
            || self.cwd.canonicalize().ok().as_ref() != Some(&self.cwd)
        {
            return Err("repair_required: Hermes workspace is missing or moved; restore the exact original directory before continuing".into());
        }
        Ok(())
    }
}

pub trait BindingStore: Send + Sync {
    fn load(&self, thread: &str) -> Result<Option<Binding>, String>;
    fn save(&self, binding: &Binding) -> Result<(), String>;
}

pub struct AppBindingStore<R: tauri::Runtime>(pub tauri::AppHandle<R>);
impl<R: tauri::Runtime> BindingStore for AppBindingStore<R> {
    fn load(&self, thread: &str) -> Result<Option<Binding>, String> {
        use tauri::Manager;
        self.0
            .state::<crate::database::DatabaseStore>()
            .hermes_binding(thread)
    }
    fn save(&self, binding: &Binding) -> Result<(), String> {
        use tauri::Manager;
        self.0
            .state::<crate::database::DatabaseStore>()
            .save_hermes_binding(binding)
    }
}
