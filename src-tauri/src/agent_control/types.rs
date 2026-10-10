use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAccess {
    ReadOnly,
    Supervised,
    FullAccess,
}

#[derive(Debug, Clone)]
pub struct ControlCaller {
    pub principal: String,
    pub access: ControlAccess,
    pub grant_id: Option<String>,
}

impl ControlCaller {
    /// Local socket access already confers application control authority.
    pub fn trusted() -> Self {
        Self { principal: "local".into(), access: ControlAccess::FullAccess, grant_id: None }
    }

    pub fn outside(grant_id: String, access: ControlAccess) -> Self {
        Self { principal: format!("mcp:{grant_id}"), access, grant_id: Some(grant_id) }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlError {
    pub code: &'static str,
    pub message: String,
}

impl ControlError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl From<String> for ControlError {
    fn from(message: String) -> Self { Self::new("native_control_failed", message) }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ControlError {}
