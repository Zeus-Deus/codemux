//! Conservative native policy conversion shared by control and delegation.
use super::ControlError;
use crate::agent_provider::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ceiling {
    filesystem: u8,
    approvals: u8,
    pub plan: bool,
}
impl Ceiling {
    pub fn for_mode(provider: ProviderKind, mode: Option<&str>) -> Option<Self> {
        use ProviderKind::*;
        let (filesystem, approvals, plan) = match (provider, mode?) {
            (Claude, "plan") | (Codex, "plan" | "read-only") => (0, 0, true),
            (Cursor | Grok, "ask" | "plan") => (0, 0, false),
            (Claude, "default") => (1, 0, false),
            (Claude, "acceptEdits") | (Codex, "workspace-write") => (1, 1, false),
            (Claude, "bypassPermissions")
            | (Codex, "danger-full-access")
            | (Cursor | Grok, "agent") => (2, 2, false),
            (OpenCode | Hermes, "default") => (1, 0, false),
            _ => return None,
        };
        Some(Self {
            filesystem,
            approvals,
            plan,
        })
    }
    pub fn full_access(self) -> bool {
        self.filesystem == 2 && self.approvals == 2 && !self.plan
    }
    pub fn intersection(self, other: Self) -> Self {
        Self {
            filesystem: self.filesystem.min(other.filesystem),
            approvals: self.approvals.min(other.approvals),
            plan: self.plan || other.plan,
        }
    }
    pub fn permits(self, target: Self) -> bool {
        target.filesystem <= self.filesystem
            && target.approvals <= self.approvals
            && (!self.plan || target.plan)
    }
    pub fn inherited(self, provider: ProviderKind) -> Result<String, ControlError> {
        use ProviderKind::*;
        let mode = match provider {
            Claude if self.plan => "plan",
            Claude if self.filesystem == 0 => "plan",
            Claude if self.filesystem == 2 && self.approvals == 2 => "bypassPermissions",
            Claude if self.approvals >= 1 => "acceptEdits",
            Claude => "default",
            Codex if self.plan => "read-only",
            Codex if self.filesystem == 2 && self.approvals == 2 => "danger-full-access",
            Codex if self.filesystem >= 1 && self.approvals >= 1 => "workspace-write",
            Codex => "read-only",
            _ => {
                return Err(ControlError::new(
                    "unsupported_permission_ceiling",
                    "This provider has no reviewed native child policy mapping",
                ))
            }
        };
        Ok(mode.into())
    }
}
