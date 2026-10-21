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
        let (filesystem, approvals, plan) = match (provider, mode) {
            (Claude, Some("plan")) | (Codex, Some("plan" | "read-only")) => (0, 0, true),
            (Cursor | Grok, Some("ask" | "plan")) => (0, 0, false),
            (Claude, Some("default")) => (1, 0, false),
            (Claude, Some("acceptEdits")) | (Codex, Some("workspace-write")) => (1, 1, false),
            (Claude, Some("bypassPermissions"))
            | (Codex, Some("danger-full-access"))
            | (Cursor | Grok, Some("agent")) => (2, 2, false),
            // OpenCode has no chat permission picker; its default requires
            // full access rather than implying a supervised sandbox.
            (OpenCode, None) => (2, 2, false),
            (OpenCode | Hermes, Some("default")) => (1, 0, false),
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

    /// Workspace startup also supports providers without native child tools.
    pub fn inherited_workspace(self, provider: ProviderKind) -> Result<Option<String>, ControlError> {
        use ProviderKind::*;
        let mode = match provider {
            Claude | Codex => Some(self.inherited(provider)?),
            Cursor | Grok => Some(if self.full_access() { "agent" } else { "ask" }.into()),
            OpenCode if self.full_access() => None,
            _ => return Err(ControlError::new("unsupported_permission_ceiling", "This provider has no workspace permission mapping within this chat's effective policy")),
        };
        if !Self::for_mode(provider, mode.as_deref()).is_some_and(|target| self.permits(target)) {
            return Err(ControlError::new("unsupported_permission_ceiling", "This provider has no workspace permission mapping within this chat's effective policy"));
        }
        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_launch_permission_mappings_never_exceed_the_caller() {
        use ProviderKind::*;
        for (provider, mode) in [(Claude, "default"), (Claude, "acceptEdits"), (Claude, "bypassPermissions"), (Codex, "workspace-write"), (Codex, "danger-full-access")] {
            let ceiling = Ceiling::for_mode(provider, Some(mode)).unwrap();
            for target in [Claude, Codex, Cursor, Grok] {
                let inherited = ceiling.inherited_workspace(target).unwrap();
                assert!(ceiling.permits(Ceiling::for_mode(target, inherited.as_deref()).unwrap()));
                if matches!(target, Cursor | Grok) {
                    assert_eq!(inherited.as_deref(), Some(if ceiling.full_access() { "agent" } else { "ask" }));
                }
            }
            assert_eq!(ceiling.inherited_workspace(OpenCode).is_ok(), ceiling.full_access());
            if ceiling.full_access() { assert_eq!(ceiling.inherited_workspace(OpenCode).unwrap(), None); }
            assert!(ceiling.inherited_workspace(Hermes).is_err());
        }
    }
}
