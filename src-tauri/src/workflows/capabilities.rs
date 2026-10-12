use super::{RouteSpec, TaskAccess};
use crate::agent_provider::{managed::ManagedCapabilities, ProviderKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowCapability {
    pub provider: String,
    pub live: bool,
    pub read_only: bool,
    pub write: bool,
    pub requires_model: bool,
    pub effort_supported: bool,
    pub reason: Option<String>,
}

pub fn provider_kind(value: &str) -> Result<ProviderKind, String> {
    match value.to_ascii_lowercase().as_str() {
        "claude" => Ok(ProviderKind::Claude),
        "codex" => Ok(ProviderKind::Codex),
        "cursor" => Ok(ProviderKind::Cursor),
        "grok" => Ok(ProviderKind::Grok),
        "hermes" => Ok(ProviderKind::Hermes),
        "opencode" => Ok(ProviderKind::OpenCode),
        _ => Err(format!("unknown workflow provider: {value}")),
    }
}

pub fn describe(kind: ProviderKind, capabilities: ManagedCapabilities) -> WorkflowCapability {
    let common = capabilities.scoped_tools
        && capabilities.native_fanout_disabled
        && capabilities.verified_stop;
    let read_only = common && capabilities.enforced_read_only;
    let write = common && capabilities.isolated_writes;
    WorkflowCapability {
        provider: serde_json::to_value(kind)
            .unwrap_or_default()
            .as_str()
            .unwrap_or("unknown")
            .into(),
        live: read_only || write,
        read_only,
        write,
        requires_model: kind == ProviderKind::OpenCode,
        effort_supported: kind != ProviderKind::Cursor,
        reason: if common && (read_only || write) {
            None
        } else {
            Some(match kind {
            ProviderKind::Hermes=>"Managed Hermes requires Linux and explicit prepared source/Python paths; API-key and local routes only",
            ProviderKind::Grok=>"Grok Build cannot yet verify its private tool catalog and disable independent fanout before inference",
            _=>"Managed tool isolation and process quiescence are unavailable on this platform",
        }.into())
        },
    }
}

/// Reject known incompatible selections before creating a durable live run.
/// Availability and credentials still require each adapter's readiness handshake.
pub fn validate_route(kind: ProviderKind, route: &RouteSpec) -> Result<(), String> {
    if kind == ProviderKind::OpenCode {
        let valid = route
            .model
            .as_deref()
            .and_then(|model| model.split_once('/'))
            .is_some_and(|(provider, model)| {
                !provider.is_empty()
                    && !model.is_empty()
                    && provider
                        .bytes()
                        .all(|value| value.is_ascii_alphanumeric() || b"._-".contains(&value))
                    && !model.chars().any(char::is_whitespace)
            });
        if !valid {
            return Err("OpenCode workflows require an explicit provider/model".into());
        }
    }
    if kind == ProviderKind::Cursor && route.effort.is_some() {
        return Err("Cursor SDK workflows require standalone effort to be unset; choose an SDK model variant instead".into());
    }
    Ok(())
}

pub fn require(capabilities: ManagedCapabilities, access: TaskAccess) -> Result<(), String> {
    if !capabilities.scoped_tools
        || !capabilities.native_fanout_disabled
        || !capabilities.verified_stop
    {
        return Err("provider does not support authenticated scoped tools, controlled native delegation, and verified stop".into());
    }
    match access {
        TaskAccess::ReadOnly if !capabilities.enforced_read_only => {
            Err("provider cannot enforce read-only workflow tools".into())
        }
        TaskAccess::Write if !capabilities.isolated_writes => {
            Err("provider cannot isolate workflow writes".into())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workflow_capabilities_fail_closed_without_each_guarantee() {
        let supported = ManagedCapabilities {
            scoped_tools: true,
            native_fanout_disabled: true,
            enforced_read_only: true,
            isolated_writes: true,
            verified_stop: true,
        };
        assert!(require(supported, TaskAccess::Write).is_ok());
        assert!(require(
            ManagedCapabilities {
                verified_stop: false,
                ..supported
            },
            TaskAccess::Write
        )
        .is_err());
        assert!(require(ManagedCapabilities::default(), TaskAccess::ReadOnly).is_err());
        assert!(!describe(ProviderKind::OpenCode, ManagedCapabilities::default()).live);
    }

    #[test]
    fn live_provider_selections_reject_known_incompatibilities() {
        let mut route = RouteSpec {
            id: "native".into(),
            provider: "opencode".into(),
            model: None,
            effort: None,
        };
        for model in [
            None,
            Some(""),
            Some("model"),
            Some("/model"),
            Some("provider/"),
            Some("provider/ "),
            Some("provider/model name"),
            Some("provider name/model"),
        ] {
            route.model = model.map(str::to_owned);
            assert!(
                validate_route(ProviderKind::OpenCode, &route).is_err(),
                "{model:?}"
            );
        }
        route.model = Some("openai/gpt-6.1".into());
        assert!(validate_route(ProviderKind::OpenCode, &route).is_ok());
        route.effort = Some("high".into());
        assert!(validate_route(ProviderKind::Cursor, &route).is_err());
        assert!(validate_route(ProviderKind::Claude, &route).is_ok());
        route.effort = None;
        assert!(validate_route(ProviderKind::Cursor, &route).is_ok());
        assert!(describe(ProviderKind::OpenCode, ManagedCapabilities::default()).requires_model);
        assert!(!describe(ProviderKind::Cursor, ManagedCapabilities::default()).effort_supported);
    }
}
