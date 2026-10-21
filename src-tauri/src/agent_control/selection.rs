//! Resolve opaque composer instance IDs through host-owned configuration.
use super::{service, ControlError};
use crate::{
    agent_provider::{
        custom_acp::{AcpConfigOption, GenericAcpProvider},
        hermes::{profile::Profile, HermesProvider},
        ProviderChatCapabilities, ProviderKind,
    },
    database::DatabaseStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelection {
    #[serde(alias = "provider_instance_id")]
    pub instance_id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSelection {
    pub instance_id: String,
    pub provider: ProviderKind,
    pub revision: Option<String>,
    pub model: Option<String>,
    pub options: BTreeMap<String, Value>,
    pub extra: Value,
}

pub(crate) fn profiles(db: &DatabaseStore) -> Result<Vec<Profile>, String> {
    crate::agent_provider::hermes::profile::discover(
        &db.get_setting("hermes.installation")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "hermes".into()),
        &db.get_setting("hermes.root")
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::agent_provider::hermes::profile::default_root),
    )
}
pub(crate) fn current_instance(
    db: &DatabaseStore,
    thread: &str,
    provider: ProviderKind,
) -> Result<String, String> {
    Ok(match provider {
        ProviderKind::Acp => {
            let binding = db.acp_binding(thread)?.ok_or("ACP binding missing")?;
            let configured = db.acp_launch_config(&binding.agent_id)?;
            if configured.agent.revision != binding.revision {
                return Err("stale_caller: ACP instance configuration changed".into());
            }
            format!("acp:{}", binding.agent_id)
        }
        ProviderKind::Hermes => format!(
            "hermes:{}",
            db.hermes_binding(thread)?
                .ok_or("Hermes binding missing")?
                .profile
                .id
        ),
        kind if crate::provider_accounts::supports(kind) => {
            let account = db
                .provider_account_binding(thread, crate::commands::delegation::provider_id(kind))?
                .unwrap_or_else(|| "default".into());
            if account == "default" {
                crate::commands::delegation::provider_id(kind).into()
            } else {
                format!(
                    "{}:{account}",
                    crate::commands::delegation::provider_id(kind)
                )
            }
        }
        kind => crate::commands::delegation::provider_id(kind).into(),
    })
}

pub(crate) async fn resolve<R: Runtime>(
    app: &AppHandle<R>,
    requested: ProviderSelection,
    cwd: &str,
) -> Result<
    (
        ResolvedSelection,
        ProviderChatCapabilities,
        Vec<AcpConfigOption>,
    ),
    ControlError,
> {
    let db = app.state::<DatabaseStore>();
    service::identifier(&requested.instance_id, "instance_id", 256)?;
    let (kind, extra, revision, caps, descriptors) =
        if let Some(id) = requested.instance_id.strip_prefix("acp:") {
            let config = db.acp_launch_config(id)?;
            if !config.agent.enabled {
                return Err(ControlError::new(
                    "provider_not_configured",
                    "ACP instance is disabled",
                ));
            }
            let runtime = app
                .try_state::<std::sync::Arc<GenericAcpProvider>>()
                .ok_or_else(|| {
                    ControlError::new(
                        "provider_not_configured",
                        "Custom ACP runtime is unavailable",
                    )
                })?;
            let catalog = runtime
                .probe(id, cwd.into())
                .await
                .map_err(|e| ControlError::from(e.to_string()))?;
            let current = db.acp_launch_config(id)?;
            if current.agent.revision != config.agent.revision {
                return Err(ControlError::new(
                    "stale_provider_selection",
                    "ACP definition changed during discovery",
                ));
            }
            (
                ProviderKind::Acp,
                json!({"acp_agent_id":id,"acp_revision":config.agent.revision}),
                Some(config.agent.revision),
                catalog.capabilities,
                catalog.config_options,
            )
        } else if let Some(id) = requested.instance_id.strip_prefix("hermes:") {
            let profile = profiles(&db)?
                .into_iter()
                .find(|p| p.id == id)
                .ok_or_else(|| {
                    ControlError::new(
                        "provider_not_configured",
                        "Select an existing configured Hermes profile",
                    )
                })?;
            crate::commands::hermes::require_configured_installation(&db, &profile)?;
            let runtime = app
                .try_state::<std::sync::Arc<HermesProvider>>()
                .ok_or_else(|| {
                    ControlError::new("provider_not_configured", "Hermes runtime is unavailable")
                })?;
            let response = runtime
                .orchestration_catalog(profile.clone())
                .await
                .map_err(|e| ControlError::from(e.to_string()))?;
            crate::agent_provider::hermes::durable_catalog(&response)
                .map_err(|e| ControlError::from(e.to_string()))?;
            let mut catalog = crate::agent_provider::custom_acp::catalog::catalog_from(
                "hermes",
                "Hermes",
                &Default::default(),
                &response,
            )?;
            catalog.capabilities.permission_modes = response
                .pointer("/modes/availableModes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    Some(crate::agent_provider::PermissionModeOption {
                        value: m["id"].as_str()?.into(),
                        label: m["name"].as_str()?.into(),
                        description: m["description"].as_str().unwrap_or_default().into(),
                        is_default: response.pointer("/modes/currentModeId") == m.get("id"),
                    })
                })
                .collect();
            catalog.capabilities.default_permission_mode = response
                .pointer("/modes/currentModeId")
                .and_then(Value::as_str)
                .map(str::to_string);
            (
                ProviderKind::Hermes,
                json!({"hermes_profile":profile}),
                Some(profile.identity),
                catalog.capabilities,
                vec![],
            )
        } else {
            let (slug, account) = requested
                .instance_id
                .split_once(':')
                .map(|(a, b)| (a, Some(b)))
                .unwrap_or((&requested.instance_id, None));
            let kind = crate::commands::delegation::parse_provider(slug)
                .filter(|p| !matches!(p, ProviderKind::Acp | ProviderKind::Hermes))
                .ok_or_else(|| {
                    ControlError::new(
                        "provider_not_configured",
                        "Select an exact configured instance from orchestrator_capabilities",
                    )
                })?;
            let extra = if crate::provider_accounts::supports(kind) {
                let account = account.unwrap_or("default");
                if account != "default"
                    && !db
                        .provider_account(account)?
                        .is_some_and(|a| a.provider == slug)
                {
                    return Err(ControlError::new(
                        "provider_not_configured",
                        "Provider account is unavailable",
                    ));
                }
                json!({"provider_account_id":account})
            } else if account.is_some() {
                return Err(ControlError::new(
                    "provider_not_configured",
                    "This provider has no configured account selector",
                ));
            } else {
                json!({})
            };
            readiness(app, kind).await?;
            let caps = if let Some(account) = account {
                service::capabilities_for_account(app, kind, Some(account.to_string())).await?
            } else {
                service::capabilities(app, kind).await?
            };
            (kind, extra, None, caps, vec![])
        };
    if app
        .state::<crate::commands::agent_chat::ProviderRegistry>()
        .get(kind)
        .await
        .is_none()
    {
        return Err(ControlError::new(
            "provider_not_configured",
            "Selected runtime is unavailable",
        ));
    }
    validate_options(&requested, &caps, &descriptors)?;
    let mut resolved = ResolvedSelection {
        instance_id: requested.instance_id,
        provider: kind,
        revision,
        model: requested.model,
        options: requested.options,
        extra,
    };
    if kind == ProviderKind::Acp {
        resolved.extra["acp_options"] = json!(resolved.options);
    }
    let stamp = serde_json::to_value(&resolved).map_err(|e| ControlError::from(e.to_string()))?;
    resolved.extra["orchestration_selection"] = stamp;
    Ok((resolved, caps, descriptors))
}

pub(crate) fn validate_options(
    selection: &ProviderSelection,
    caps: &ProviderChatCapabilities,
    descriptors: &[AcpConfigOption],
) -> Result<(), ControlError> {
    let model = selection
        .model
        .as_ref()
        .map(|id| {
            caps.models.iter().find(|m| &m.id == id).ok_or_else(|| {
                ControlError::new(
                    "unsupported_model",
                    "Select an exact model from this instance's catalog",
                )
            })
        })
        .transpose()?;
    for (id, value) in &selection.options {
        if let Some(option) = descriptors.iter().find(|o| &o.id == id) {
            // Permission controls require a reviewed adapter policy mapping.
            if option.category.as_deref() == Some("mode")
                || matches!(id.as_str(), "mode" | "permission_mode")
            {
                return Err(ControlError::new(
                    "unsupported_permission_ceiling",
                    "Opaque ACP mode options cannot change orchestration permissions",
                ));
            }
            crate::agent_provider::custom_acp::catalog::validate_value(option, value)?;
        } else {
            let supported = match id.as_str() {
                "effort" => value
                    .as_str()
                    .is_some_and(|v| model.is_some_and(|m| m.effort_levels.iter().any(|e| e == v))),
                "service_tier" => value.as_str().is_some_and(|v| {
                    model.is_some_and(|m| m.service_tiers.iter().any(|tier| tier.value == v))
                }),
                "context_window" => value.as_str().is_some_and(|v| {
                    model.is_some_and(|m| m.context_window_options.iter().any(|o| o.value == v))
                }),
                "fast_mode" => value
                    .as_bool()
                    .is_some_and(|v| !v || model.is_some_and(|m| m.supports_fast_mode)),
                _ => false,
            };
            if !supported {
                return Err(ControlError::new(
                    "unsupported_model_option",
                    format!("Option {id} is not advertised for this instance and model"),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn verify<R: Runtime>(
    app: &AppHandle<R>,
    selection: &ResolvedSelection,
) -> Result<(), ControlError> {
    let db = app.state::<DatabaseStore>();
    if selection.provider == ProviderKind::Acp {
        let id = selection.extra["acp_agent_id"]
            .as_str()
            .ok_or_else(|| ControlError::new("stale_provider_selection", "ACP instance missing"))?;
        if Some(db.acp_launch_config(id)?.agent.revision) != selection.revision {
            return Err(ControlError::new(
                "stale_provider_selection",
                "ACP launch definition changed after admission",
            ));
        }
    }
    if selection.provider == ProviderKind::Hermes {
        let profile: Profile = serde_json::from_value(selection.extra["hermes_profile"].clone())
            .map_err(|e| ControlError::from(e.to_string()))?;
        crate::commands::hermes::require_configured_installation(&db, &profile)?;
        profile.validate()?;
    }
    if let Some(id) = selection.extra["provider_account_id"]
        .as_str()
        .filter(|id| *id != "default")
    {
        if !db.provider_account(id)?.is_some_and(|account| {
            account.provider == crate::commands::delegation::provider_id(selection.provider)
        }) {
            return Err(ControlError::new(
                "stale_provider_selection",
                "Provider account was removed",
            ));
        }
    }
    Ok(())
}

pub(crate) fn schema() -> Value {
    json!({"type":"object","properties":{"instance_id":{"type":"string","description":"Exact provider_instance_id from orchestrator_capabilities"},"model":{"type":["string","null"]},"options":{"type":"object"}},"required":["instance_id"],"additionalProperties":false})
}

#[cfg(test)]
mod all_provider_selection_tests {
    use super::*;
    #[test]
    fn all_provider_model_options_belong_to_the_selected_catalog() {
        let caps = crate::agent_provider::claude::capabilities::claude_fallback_capabilities();
        let model = caps.models.first().unwrap().id.clone();
        let mut selection = ProviderSelection {
            instance_id: "claude:account-a".into(),
            model: Some(model),
            options: BTreeMap::new(),
        };
        validate_options(&selection, &caps, &[]).unwrap();
        selection.model = Some("model-from-other-instance".into());
        assert!(validate_options(&selection, &caps, &[]).is_err());
        selection.model = None;
        selection
            .options
            .insert("effort".into(), json!("not-advertised"));
        assert!(validate_options(&selection, &caps, &[]).is_err());
        selection.options.clear();
        selection
            .options
            .insert("executable".into(), json!("/bin/sh"));
        assert!(validate_options(&selection, &caps, &[]).is_err());
        let descriptor:AcpConfigOption=serde_json::from_value(json!({"id":"custom-effort","name":"Reasoning","category":"thought_level","type":"select","current_value":"small","options":[{"value":"small","name":"Small","description":null}]})).unwrap();
        selection.options.clear();
        selection
            .options
            .insert("custom-effort".into(), json!("small"));
        validate_options(&selection, &caps, &[descriptor.clone()]).unwrap();
        selection
            .options
            .insert("custom-effort".into(), json!("large"));
        assert!(validate_options(&selection, &caps, &[descriptor]).is_err());
    }
}

pub(crate) async fn readiness<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
) -> Result<(), ControlError> {
    if !matches!(
        provider,
        ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::OpenCode
    ) {
        return Ok(());
    }
    #[cfg(test)]
    if app
        .try_state::<super::NativeControlState>()
        .is_some_and(|state| {
            state
                .fixture_capabilities
                .lock()
                .unwrap()
                .contains_key(&provider)
        })
    {
        return Ok(());
    }
    let _ = app;
    let health = crate::agent_provider::health::check_provider_health(provider).await;
    if health.status != crate::agent_provider::health::ProviderHealthStatus::Ready {
        return Err(ControlError::new(
            "provider_not_ready",
            health
                .message
                .unwrap_or_else(|| "Provider readiness is unconfirmed".into()),
        ));
    }
    Ok(())
}

pub(crate) fn bind_new_thread<R: Runtime>(
    app: &AppHandle<R>,
    selection: &ResolvedSelection,
    thread: &str,
) -> Result<(), ControlError> {
    verify(app, selection)?;
    if let Some(account) = selection.extra["provider_account_id"].as_str() {
        app.state::<DatabaseStore>().set_provider_account_binding(
            thread,
            crate::commands::delegation::provider_id(selection.provider),
            account,
        )?;
    }
    Ok(())
}
