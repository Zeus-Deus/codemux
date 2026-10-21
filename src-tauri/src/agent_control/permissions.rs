//! Conservative native policy conversion shared by control and delegation.
use super::ControlError;
use crate::agent_provider::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ceiling {
    filesystem: u8,
    approvals: u8,
    pub plan: bool,
    #[serde(default)]
    opaque: Option<u64>,
}

impl Ceiling {
    pub fn for_mode(provider: ProviderKind, mode: Option<&str>) -> Option<Self> {
        use ProviderKind::*;
        let mode = match (provider, mode) {
            (OpenCode | Hermes, None) => "default",
            (Acp, None) => "supervised",
            (_, Some(mode)) => mode,
            _ => return None,
        };
        if provider == Acp && mode == "supervised" {
            return Some(Self {
                filesystem: 0,
                approvals: 0,
                plan: false,
                opaque: Some(0),
            });
        }
        let (filesystem, approvals, plan) = match (provider, mode) {
            (Claude, "plan") | (Codex, "plan" | "read-only") => (0, 0, true),
            (Cursor | Grok, "ask") => (0, 0, false),
            (Cursor | Grok, "plan") => (0, 0, true),
            (Claude, "default") => (1, 0, false),
            (Claude, "acceptEdits") | (Codex, "workspace-write") => (1, 1, false),
            (Claude, "bypassPermissions")
            | (Codex, "danger-full-access")
            | (Cursor | Grok, "agent") => (2, 2, false),
            // Hermes modes only choose file-edit approval; its terminal runs
            // any command not on its dangerous-pattern list in every mode.
            (Hermes, "default" | "accept_edits" | "dont_ask") => (2, 2, false),
            // OpenCode agents allow every tool by default and Codemux applies
            // no restriction of its own. Plan only denies edit tools; its
            // shell can still write, so plan keeps full reach plus intent.
            (OpenCode, "default") => (2, 2, false),
            (OpenCode, "plan") => (2, 2, true),
            _ => return None,
        };
        Some(Self {
            filesystem,
            approvals,
            plan,
            opaque: None,
        })
    }
    pub(crate) fn opaque_policy(binding: &crate::agent_provider::custom_acp::AcpBinding) -> Self {
        use sha2::{Digest, Sha256};
        let mut values: Vec<_> = binding
            .catalog
            .config_options
            .iter()
            .map(|o| (&o.id, &o.current_value))
            .collect();
        values.sort_by_key(|(id, _)| *id);
        let digest = Sha256::digest(
            serde_json::to_vec(&serde_json::json!([
                binding.agent_id,
                binding.revision,
                values
            ]))
            .unwrap_or_default(),
        );
        Self {
            filesystem: 0,
            approvals: 0,
            plan: false,
            opaque: Some(u64::from_le_bytes(digest[..8].try_into().unwrap()).max(1)),
        }
    }
    pub(crate) fn for_record(
        db: &crate::database::DatabaseStore,
        record: &crate::database::AgentChatSessionRecord,
        provider: ProviderKind,
    ) -> Option<Self> {
        if provider == ProviderKind::Acp {
            return db
                .acp_binding(&record.thread_id)
                .ok()
                .flatten()
                .map(|b| Self::opaque_policy(&b));
        }
        Self::for_mode(provider, record.permission_mode.as_deref())
    }
    pub fn full_access(self) -> bool {
        self.opaque.is_none() && self.filesystem == 2 && self.approvals == 2 && !self.plan
    }
    pub fn intersection(self, other: Self) -> Self {
        Self {
            filesystem: self.filesystem.min(other.filesystem),
            approvals: self.approvals.min(other.approvals),
            plan: self.plan || other.plan,
            opaque: self.opaque.or(other.opaque),
        }
    }
    pub fn permits(self, target: Self) -> bool {
        if self.opaque.is_some() || target.opaque.is_some() {
            return self
                .opaque
                .is_some_and(|id| id != 0 && target.opaque == Some(id));
        }
        target.filesystem <= self.filesystem
            && target.approvals <= self.approvals
            && (!self.plan || target.plan)
    }
    pub fn inherited(self, provider: ProviderKind) -> Result<String, ControlError> {
        use ProviderKind::*;
        if self.opaque.is_some() {
            return Err(ControlError::new(
                "unsupported_permission_ceiling",
                "Opaque ACP policy has no reviewed cross-provider mode mapping",
            ));
        }
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
            Cursor | Grok if self.plan => "plan",
            Cursor | Grok if self.filesystem == 2 && self.approvals == 2 => "agent",
            Cursor | Grok => "ask",
            Hermes if self.full_access() => "dont_ask",
            OpenCode if self.full_access() => "default",
            OpenCode if self.filesystem == 2 && self.approvals == 2 => "plan",
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
            Claude | Codex | Cursor | Grok | Hermes => Some(self.inherited(provider)?),
            // OpenCode has no chat permission picker; its default is unset.
            OpenCode => Some(self.inherited(provider)?).filter(|mode| mode != "default"),
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
            assert_eq!(ceiling.inherited_workspace(Hermes).ok().flatten().as_deref(), ceiling.full_access().then_some("dont_ask"));
        }
    }
}

#[cfg(test)]
mod all_provider_policy_tests {
    use super::*;
    #[test]
    fn all_provider_known_modes_are_bounded_and_unknown_modes_fail_closed() {
        for provider in [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::Grok,
            ProviderKind::Hermes,
            ProviderKind::OpenCode,
            ProviderKind::Acp,
        ] {
            assert!(Ceiling::for_mode(provider, Some("unknown-policy")).is_none());
            if provider == ProviderKind::Acp {
                assert!(Ceiling::for_mode(provider, None)
                    .unwrap()
                    .inherited(ProviderKind::Claude)
                    .is_err());
                continue;
            }
            for mode in [
                "plan",
                "default",
                "ask",
                "agent",
                "accept_edits",
                "dont_ask",
                "read-only",
                "workspace-write",
                "danger-full-access",
                "bypassPermissions",
            ] {
                let Some(parent) = Ceiling::for_mode(provider, Some(mode)) else {
                    continue;
                };
                for target in [
                    ProviderKind::Claude,
                    ProviderKind::Codex,
                    ProviderKind::Cursor,
                    ProviderKind::Grok,
                    ProviderKind::Hermes,
                    ProviderKind::OpenCode,
                ] {
                    if let Ok(inherited) = parent.inherited(target) {
                        assert!(
                            parent.permits(Ceiling::for_mode(target, Some(&inherited)).unwrap()),
                            "{provider:?} {mode} -> {target:?} {inherited}"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod provider_reach_tests {
    use super::*;
    use ProviderKind::*;

    fn mode(provider: ProviderKind, mode: &str) -> Ceiling {
        Ceiling::for_mode(provider, Some(mode)).unwrap()
    }

    // OpenCode 2.0.29 gives every agent `{"*": allow}` and its plan agent adds
    // only `edit: deny`, so the shell is open in both modes Codemux uses.
    // Hermes's ACP modes choose file-edit approval only; its terminal asks
    // just for dangerous-pattern commands (acp_adapter/server.py `_MODES`).
    const UNRESTRICTED_SHELL: [(ProviderKind, &str); 5] = [
        (OpenCode, "default"),
        (OpenCode, "plan"),
        (Hermes, "default"),
        (Hermes, "accept_edits"),
        (Hermes, "dont_ask"),
    ];

    #[test]
    fn restricted_callers_cannot_start_an_unrestricted_shell() {
        for (provider, parent) in [
            (Claude, "default"),
            (Claude, "acceptEdits"),
            (Claude, "plan"),
            (Codex, "read-only"),
            (Codex, "workspace-write"),
            (Cursor, "ask"),
            (Cursor, "plan"),
            (Grok, "ask"),
        ] {
            let caller = mode(provider, parent);
            for (target, target_mode) in UNRESTRICTED_SHELL {
                assert!(
                    !caller.permits(mode(target, target_mode)),
                    "{provider:?} {parent} must not start {target:?} {target_mode}"
                );
            }
            assert!(caller.inherited(OpenCode).is_err(), "{provider:?} {parent}");
            assert!(caller.inherited(Hermes).is_err(), "{provider:?} {parent}");
        }
    }

    #[test]
    fn full_access_callers_reach_every_unrestricted_shell() {
        for (provider, parent) in [
            (Claude, "bypassPermissions"),
            (Codex, "danger-full-access"),
            (Cursor, "agent"),
            (Grok, "agent"),
            (OpenCode, "default"),
            (Hermes, "default"),
        ] {
            let caller = mode(provider, parent);
            assert!(caller.full_access(), "{provider:?} {parent}");
            for (target, target_mode) in UNRESTRICTED_SHELL {
                assert!(caller.permits(mode(target, target_mode)));
            }
            assert_eq!(caller.inherited(OpenCode).unwrap(), "default");
            assert_eq!(caller.inherited(Hermes).unwrap(), "dont_ask");
        }
    }

    #[test]
    fn opencode_plan_keeps_planning_intent_without_understating_its_shell() {
        let plan = mode(OpenCode, "plan");
        assert!(!plan.full_access());
        assert_eq!(plan.inherited(OpenCode).unwrap(), "plan");
        assert_eq!(plan.inherited(Claude).unwrap(), "plan");
        assert!(plan.inherited(Hermes).is_err(), "Hermes has no plan mode");
        assert!(!plan.permits(mode(OpenCode, "default")));
        assert!(!mode(Claude, "plan").permits(plan));
    }
}
