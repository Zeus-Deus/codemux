//! Capability metadata for the Hermes provider.
//!
//! Hermes splits its catalogue across two very different costs. The
//! permission modes are fixed and known statically — the agent reports the
//! same three every time — so they are declared here. The model list is
//! not: the authoritative one only comes back on `session/new`, which
//! boots a real agent and takes seconds.
//!
//! That gap is why this module has two halves:
//!
//! * **Seed** — [`seed_capabilities`] reads the profile's `config.yaml`
//!   off disk (no process) so the picker opens on the model the profile
//!   is actually configured for instead of an empty dropdown.
//! * **Refresh** — [`parse_session_catalog`] takes the `session/new`
//!   response and replaces the seed with the real 29-entry catalogue and
//!   the agent's own mode list.
//!
//! [`HermesCapabilityCache`] holds the result **per profile**, because
//! two profiles routinely point at different runtimes and sharing one
//! slot would show the wrong models. It is warmed lazily when the picker
//! rail opens, never eagerly at startup — a Hermes session start costs
//! seconds, and nothing about app launch is worth paying that for.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::discovery::{self, HermesProfile};
use crate::agent_provider::types::{
    ChatModelInfo, EffortGranularity, PermissionModeOption, ProviderChatCapabilities,
};

/// The mode a Hermes session launches in when the user has not chosen one.
///
/// Ask-first, unlike the other providers' full-access defaults, because a
/// Hermes ACP mode governs *edits* only — shell commands are already gated
/// outside the protocol by the profile's own `approvals.mode`. Launching
/// wide open here would hand away the one approval surface the protocol
/// actually offers.
///
/// Kept as a named const because `commands::agent_chat::fallback_permission_mode`
/// must return the same string, and the frontend's
/// `FALLBACK_DEFAULT_PERMISSION_MODE_BY_PROVIDER` must agree with both: the
/// picker seeds itself to the provider default, and a session started in a
/// different mode than the UI is displaying desyncs into prompting for
/// approvals the user believes they already granted.
pub const HERMES_DEFAULT_PERMISSION_MODE: &str = "default";

/// The mode a full-access selection carried over from another provider
/// canonicalizes onto — Hermes' most permissive edit mode.
///
/// Separate from [`HERMES_DEFAULT_PERMISSION_MODE`] deliberately: one is
/// what an unspecified session launches with, the other is how an explicit
/// "full access" choice the user already made under Claude/Codex/Cursor is
/// translated. Collapsing them would silently widen every unspecified
/// session.
pub const HERMES_FULL_ACCESS_PERMISSION_MODE: &str = "dont_ask";

/// The three ACP modes Hermes advertises, verbatim from the agent.
///
/// Note what these do NOT cover: they govern *file edits* only. Shell
/// commands are gated by the profile's own `approvals.mode`, outside the
/// protocol — a profile set to auto-approve runs shell commands without a
/// permission request at all. The descriptions stay narrow about edits so
/// the picker does not imply it is authorising more than it is.
pub fn hermes_permission_modes() -> Vec<PermissionModeOption> {
    vec![
        PermissionModeOption {
            value: HERMES_DEFAULT_PERMISSION_MODE.into(),
            label: "Ask".into(),
            description: "Ask before each file edit.".into(),
            is_default: true,
        },
        PermissionModeOption {
            value: "accept_edits".into(),
            label: "Accept Edits".into(),
            description: "Auto-allow workspace and temp-dir edits; still asks for sensitive paths."
                .into(),
            is_default: false,
        },
        PermissionModeOption {
            value: HERMES_FULL_ACCESS_PERMISSION_MODE.into(),
            label: "Don't Ask".into(),
            description: "Auto-allow file edits for this session except sensitive paths.".into(),
            is_default: false,
        },
    ]
}

/// Stage 1 capabilities bundle: real permission modes, no models.
///
/// An empty model list is honest rather than a failure — the picker falls
/// back to its maintained defaults for the model rail and still renders a
/// correct permission picker. Returning an error here instead would put a
/// red banner on a provider whose only missing piece is a catalogue that
/// costs a session to fetch.
///
/// Effort granularity is `PerSession` because Hermes carries reasoning
/// selection on the launch line rather than per prompt.
pub fn hermes_stage1_capabilities() -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: Vec::new(),
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: Default::default(),
        permission_modes: hermes_permission_modes(),
        default_permission_mode: Some(HERMES_DEFAULT_PERMISSION_MODE.to_string()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Why a capability lookup produced nothing usable.
///
/// The variants exist so the picker can print a sentence instead of an
/// empty dropdown: the frontend's `parseProviderError` keys on the
/// `<provider>_<reason>: hint` prefix, so the token strings below are a
/// contract with it, not free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarvestError {
    /// No `hermes` executable on PATH or in the known install locations.
    NotInstalled { hint: String },
    /// The profile exists but has no credentials — `hermes --setup` or a
    /// login has never been completed for it.
    NotAuthenticated { hint: String },
    /// Anything else: a missing profile, an unreadable root.
    HarvestFailed { message: String },
}

impl HarvestError {
    pub fn to_command_string(&self) -> String {
        match self {
            Self::NotInstalled { hint } => format!("hermes_not_installed: {hint}"),
            Self::NotAuthenticated { hint } => format!("hermes_not_authenticated: {hint}"),
            Self::HarvestFailed { message } => format!("hermes_harvest_failed: {message}"),
        }
    }
}

/// The model and mode catalogue carried on a `session/new` response.
///
/// Parsed rather than deserialized wholesale so a payload that grows a
/// key, or drops one, degrades to an empty list instead of failing the
/// session start that produced it.
#[derive(Debug, Clone, Default)]
pub struct HermesSessionCatalog {
    /// `models.availableModels`, in the order the agent listed them.
    pub models: Vec<ChatModelInfo>,
    /// `models.currentModelId`, in `provider:model` form.
    pub current_model_id: Option<String>,
    /// `modes.availableModes`.
    pub modes: Vec<PermissionModeOption>,
    /// `modes.currentModeId`.
    pub current_mode_id: Option<String>,
}

impl HermesSessionCatalog {
    /// True when the response carried nothing worth replacing a seed with.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.modes.is_empty()
    }
}

/// Read the catalogue out of a `session/new` response.
///
/// Never fails. An absent or unrecognised `models` / `modes` block
/// yields empty vectors, and the caller keeps the seed — a session that
/// otherwise started fine must not be torn down over a picker detail.
pub fn parse_session_catalog(response: &Value) -> HermesSessionCatalog {
    let payload: SessionNewPayload = match serde_json::from_value(response.clone()) {
        Ok(payload) => payload,
        Err(_) => return HermesSessionCatalog::default(),
    };

    let models_block = payload.models.unwrap_or_default();
    let models = models_block
        .available_models
        .into_iter()
        .filter(|model| !model.model_id.trim().is_empty())
        .map(|model| chat_model(model.model_id, model.name, model.description))
        .collect();

    let modes_block = payload.modes.unwrap_or_default();
    let current_mode_id = modes_block
        .current_mode_id
        .filter(|id| !id.trim().is_empty());
    let modes =
        permission_modes_from_agent(&modes_block.available_modes, current_mode_id.as_deref());

    HermesSessionCatalog {
        models,
        current_model_id: models_block
            .current_model_id
            .filter(|id| !id.trim().is_empty()),
        modes,
        current_mode_id,
    }
}

/// Build the picker bundle from a live `session/new` catalogue.
///
/// `supports_images` comes from the `initialize` response's
/// `promptCapabilities.image`, which is a property of the agent rather
/// than of any individual model — Hermes reports no per-model
/// multimodal flag, so the session-level answer is applied to all of
/// them or to none.
pub fn capabilities_from_catalog(
    catalog: &HermesSessionCatalog,
    supports_images: bool,
) -> ProviderChatCapabilities {
    let mut models = catalog.models.clone();
    for model in &mut models {
        model.supports_images = supports_images;
    }
    promote_current_model(&mut models, catalog.current_model_id.as_deref());

    // Fall back to the static table when the agent reported no modes at
    // all: the permission picker disappearing is a far more visible
    // regression than a mode list one release out of date.
    let modes = if catalog.modes.is_empty() {
        hermes_permission_modes()
    } else {
        catalog.modes.clone()
    };
    let default_permission_mode = modes
        .iter()
        .find(|mode| mode.is_default)
        .map(|mode| mode.value.clone())
        .or_else(|| Some(HERMES_DEFAULT_PERMISSION_MODE.to_string()));

    ProviderChatCapabilities {
        models,
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: Default::default(),
        permission_modes: modes,
        default_permission_mode,
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Seed the model list from a profile's `config.yaml`.
///
/// One or two entries — the configured model and, when present, the
/// configured fallback — both already in the `provider:model` form the
/// live catalogue uses, so the seeded id reconciles with the real list
/// the moment a session starts rather than briefly showing as unknown.
pub fn seed_models_from_profile(profile: &HermesProfile) -> Vec<ChatModelInfo> {
    let mut models = Vec::new();
    for id in [
        profile.default_model.clone(),
        profile.fallback_model.clone(),
    ]
    .into_iter()
    .flatten()
    {
        if models.iter().any(|model: &ChatModelInfo| model.id == id) {
            continue;
        }
        models.push(chat_model(id, None, None));
    }
    models
}

/// The picker bundle for a profile, built entirely from disk.
///
/// This is the pre-session state: real permission modes, and whatever
/// models `config.yaml` names. An empty model list here is normal for a
/// freshly created profile and is not an error — the live catalogue
/// replaces it on the first `session/new`.
pub fn seed_capabilities(profile: &HermesProfile) -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: seed_models_from_profile(profile),
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: Default::default(),
        permission_modes: hermes_permission_modes(),
        default_permission_mode: Some(HERMES_DEFAULT_PERMISSION_MODE.to_string()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Resolve a profile on disk and seed its capabilities, or say why not.
///
/// Deliberately process-free: this is what the picker calls when its
/// rail opens, and it must answer at click speed. The install check is
/// the one thing it cannot read out of the profile directory, so it is
/// passed in — the caller already holds it from the health probe.
pub fn seed_capabilities_for_profile(
    root: &Path,
    profile_id: &str,
    installed: bool,
) -> Result<ProviderChatCapabilities, HarvestError> {
    if !installed {
        return Err(HarvestError::NotInstalled {
            hint: "Install Hermes and make sure `hermes` is on your PATH.".into(),
        });
    }
    let profile =
        discovery::find_profile(root, profile_id).ok_or_else(|| HarvestError::HarvestFailed {
            message: format!(
                "No Hermes profile named `{profile_id}` under {}.",
                root.display()
            ),
        })?;
    if !profile.looks_authenticated() {
        return Err(HarvestError::NotAuthenticated {
            hint: format!(
                "Profile `{profile_id}` has no credentials yet. Run `hermes -p {profile_id} --setup` to sign in."
            ),
        });
    }
    Ok(seed_capabilities(&profile))
}

/// Per-profile capability cache.
///
/// Keyed by profile id because a profile IS the runtime selection in
/// Hermes: `coder` and `default` can point at different providers with
/// disjoint model lists, and one shared slot would show whichever was
/// asked for last.
#[derive(Default)]
pub struct HermesCapabilityCache {
    inner: Mutex<HashMap<String, ProviderChatCapabilities>>,
}

impl HermesCapabilityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Warm a profile from disk, or return what is already cached.
    ///
    /// No TTL: nothing here expires on its own, because the only two
    /// things that change the answer are an edit to `config.yaml` and a
    /// `session/new` — and the second one calls
    /// [`record_session_catalog`](Self::record_session_catalog)
    /// directly. A stale seed is corrected by the session it seeded.
    pub async fn get_or_seed(
        &self,
        root: &Path,
        profile_id: &str,
        installed: bool,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        let mut cached = self.inner.lock().await;
        if let Some(entry) = cached.get(profile_id) {
            return Ok(entry.clone());
        }
        let fresh = seed_capabilities_for_profile(root, profile_id, installed)?;
        cached.insert(profile_id.to_string(), fresh.clone());
        Ok(fresh)
    }

    /// Replace a profile's entry with the agent's own catalogue.
    ///
    /// Called by the session driver with the `session/new` response.
    /// Returns false — and leaves the seed alone — for a response that
    /// carried no catalogue, so a protocol change cannot empty a picker
    /// that was working a moment ago.
    pub async fn record_session_catalog(
        &self,
        profile_id: &str,
        catalog: &HermesSessionCatalog,
        supports_images: bool,
    ) -> bool {
        if catalog.is_empty() {
            return false;
        }
        self.inner.lock().await.insert(
            profile_id.to_string(),
            capabilities_from_catalog(catalog, supports_images),
        );
        true
    }

    /// What is cached for a profile, without warming it.
    pub async fn peek(&self, profile_id: &str) -> Option<ProviderChatCapabilities> {
        self.inner.lock().await.get(profile_id).cloned()
    }

    /// Drop one profile's entry, or every entry when given `None`.
    pub async fn invalidate(&self, profile_id: Option<&str>) {
        let mut cached = self.inner.lock().await;
        match profile_id {
            Some(id) => {
                cached.remove(id);
            }
            None => cached.clear(),
        }
    }
}

/// Build a [`ChatModelInfo`] from a `provider:model` id.
///
/// The provider half becomes `sub_provider` so the picker groups Hermes
/// models by upstream the way it already groups OpenCode's, and the
/// agent's own `name` ("Anthropic · claude-opus-5") is used verbatim
/// when it sent one.
fn chat_model(id: String, name: Option<String>, description: Option<String>) -> ChatModelInfo {
    let (sub_provider, bare_model) = match id.split_once(':') {
        // `split_once` on the FIRST colon: the model half legitimately
        // contains slashes and further punctuation
        // (`openrouter:anthropic/claude-sonnet-4`).
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
            (Some(provider.to_string()), model.to_string())
        }
        _ => (None, id.clone()),
    };
    let label = name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or(bare_model);

    ChatModelInfo {
        id,
        label,
        description: description
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        // Hermes carries reasoning effort in the profile's `agent`
        // config, not per prompt, so no effort rail is offered here.
        effort_levels: Vec::new(),
        default_effort: None,
        effort_descriptions: HashMap::new(),
        prompt_injected_effort_levels: Vec::new(),
        context_window_options: Vec::new(),
        supports_adaptive_thinking: false,
        supports_thinking_toggle: false,
        supports_fast_mode: false,
        // Set from the agent's session-level `promptCapabilities` by
        // `capabilities_from_catalog`; false is the safe seed value.
        supports_images: false,
        sub_provider,
        is_free: false,
        // The agent reports no per-model window; the context meter is
        // fed by the live `usage_update` push instead.
        max_context_tokens: None,
    }
}

/// Move the session's current model to the front of the list.
///
/// [`ProviderChatCapabilities`] has no "default model" field — order is
/// how a provider states its preference — so the model the agent says
/// it is on has to lead, or the picker highlights the wrong row.
fn promote_current_model(models: &mut Vec<ChatModelInfo>, current_model_id: Option<&str>) {
    let Some(current) = current_model_id else {
        return;
    };
    if let Some(index) = models.iter().position(|model| model.id == current) {
        let model = models.remove(index);
        models.insert(0, model);
    }
}

/// Project the agent's `availableModes` onto the picker's options.
///
/// The `is_default` flag marks the mode a *new* session should launch
/// in, which is not the same question as `currentModeId` (the mode the
/// session that reported this happens to be in). It stays pinned to
/// [`HERMES_DEFAULT_PERMISSION_MODE`] whenever the agent still offers
/// that mode, because `fallback_permission_mode` and the frontend's
/// default table both hard-code the same string — letting the flag
/// drift to whatever the last session was in would desync all three.
fn permission_modes_from_agent(
    modes: &[AvailableMode],
    current_mode_id: Option<&str>,
) -> Vec<PermissionModeOption> {
    let known: Vec<&AvailableMode> = modes
        .iter()
        .filter(|mode| !mode.id.trim().is_empty())
        .collect();
    let default_id = if known
        .iter()
        .any(|mode| mode.id == HERMES_DEFAULT_PERMISSION_MODE)
    {
        Some(HERMES_DEFAULT_PERMISSION_MODE.to_string())
    } else {
        current_mode_id
            .map(str::to_string)
            .or_else(|| known.first().map(|mode| mode.id.clone()))
    };

    known
        .into_iter()
        .map(|mode| PermissionModeOption {
            is_default: Some(&mode.id) == default_id.as_ref(),
            value: mode.id.clone(),
            label: mode
                .name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| mode.id.clone()),
            description: mode.description.clone().unwrap_or_default(),
        })
        .collect()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SessionNewPayload {
    models: Option<ModelsBlock>,
    modes: Option<ModesBlock>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ModelsBlock {
    current_model_id: Option<String>,
    available_models: Vec<AvailableModel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AvailableModel {
    model_id: String,
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ModesBlock {
    current_mode_id: Option<String>,
    available_modes: Vec<AvailableMode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AvailableMode {
    id: String,
    name: Option<String>,
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_one_of_the_advertised_modes_and_marked_default() {
        let caps = hermes_stage1_capabilities();
        assert_eq!(
            caps.default_permission_mode.as_deref(),
            Some(HERMES_DEFAULT_PERMISSION_MODE)
        );
        let marked: Vec<&str> = caps
            .permission_modes
            .iter()
            .filter(|mode| mode.is_default)
            .map(|mode| mode.value.as_str())
            .collect();
        assert_eq!(marked, vec![HERMES_DEFAULT_PERMISSION_MODE]);
    }

    #[test]
    fn full_access_target_is_advertised_and_is_not_the_launch_default() {
        // A cross-provider "full access" choice must land on a mode Hermes
        // actually accepts, and must stay distinct from the mode an
        // unspecified session launches with.
        assert_ne!(
            HERMES_FULL_ACCESS_PERMISSION_MODE,
            HERMES_DEFAULT_PERMISSION_MODE
        );
        assert!(hermes_permission_modes()
            .iter()
            .any(|mode| mode.value == HERMES_FULL_ACCESS_PERMISSION_MODE));
    }

    #[test]
    fn advertises_exactly_the_three_protocol_modes() {
        let values: Vec<String> = hermes_permission_modes()
            .into_iter()
            .map(|mode| mode.value)
            .collect();
        assert_eq!(values, vec!["default", "accept_edits", "dont_ask"]);
    }

    fn write(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("path has a parent")).expect("create dirs");
        std::fs::write(path, contents).expect("write fixture");
    }

    /// A root with one configured, signed-in profile plus the named
    /// `sidecar` profile that has neither.
    fn fixture_root() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(
            &root.join("config.yaml"),
            "model:\n  default: gpt-5.6-sol\n  provider: openai-codex\n",
        );
        write(&root.join("auth.json"), "{\"token\":\"x\"}");
        std::fs::create_dir_all(root.join("profiles/sidecar")).expect("create dirs");
        temp
    }

    #[test]
    fn the_picker_is_seeded_from_config_yaml_before_any_session_exists() {
        let temp = fixture_root();
        let caps = seed_capabilities_for_profile(temp.path(), "default", true)
            .expect("the default profile is configured and signed in");
        let model = caps.models.first().expect("a seeded model");
        assert_eq!(model.id, "openai-codex:gpt-5.6-sol");
        // The provider half drives the picker's grouping rail.
        assert_eq!(model.sub_provider.as_deref(), Some("openai-codex"));
        // No `name` on disk, so the bare model name is the label.
        assert_eq!(model.label, "gpt-5.6-sol");
        assert!(!caps.permission_modes.is_empty());
    }

    #[test]
    fn a_configured_fallback_model_seeds_a_second_entry_without_duplicating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile = HermesProfile {
            id: "default".into(),
            dir: temp.path().to_path_buf(),
            description: None,
            default_model: Some("openai-codex:gpt-5.6-sol".into()),
            fallback_model: Some("openai-codex:gpt-5.6-sol".into()),
            is_default: true,
        };
        let ids: Vec<String> = seed_models_from_profile(&profile)
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, vec!["openai-codex:gpt-5.6-sol"]);
    }

    #[test]
    fn empty_states_mint_the_tokens_the_picker_parses() {
        let temp = fixture_root();
        let not_installed = seed_capabilities_for_profile(temp.path(), "default", false)
            .expect_err("no binary means no capabilities");
        assert!(not_installed
            .to_command_string()
            .starts_with("hermes_not_installed: "));

        // `sidecar` exists on disk but has no auth.json.
        let not_authed = seed_capabilities_for_profile(temp.path(), "sidecar", true)
            .expect_err("an unauthenticated profile cannot list models");
        assert!(not_authed
            .to_command_string()
            .starts_with("hermes_not_authenticated: "));

        let missing =
            seed_capabilities_for_profile(temp.path(), "ghost", true).expect_err("no such profile");
        assert!(missing
            .to_command_string()
            .starts_with("hermes_harvest_failed: "));
    }

    fn session_new_response() -> Value {
        serde_json::json!({
            "sessionId": "ebd7fd38-0000-0000-0000-000000000000",
            "modes": {
                "currentModeId": "accept_edits",
                "availableModes": [
                    {"id": "default", "name": "Default", "description": "Ask before edits."},
                    {"id": "accept_edits", "name": "Accept Edits", "description": "Auto-allow workspace and /tmp edits."},
                    {"id": "dont_ask", "name": "Don't Ask", "description": "Auto-allow file edits."}
                ]
            },
            "models": {
                "currentModelId": "anthropic:claude-opus-5",
                "availableModels": [
                    {"modelId": "openai-codex:gpt-5.6-sol", "name": "OpenAI · gpt-5.6-sol", "description": "Provider: OpenAI"},
                    {"modelId": "anthropic:claude-opus-5", "name": "Anthropic · claude-opus-5", "description": "Provider: Anthropic"}
                ]
            }
        })
    }

    #[test]
    fn the_session_new_catalogue_replaces_the_seed_and_leads_with_the_current_model() {
        let catalog = parse_session_catalog(&session_new_response());
        assert_eq!(
            catalog.current_model_id.as_deref(),
            Some("anthropic:claude-opus-5")
        );
        assert_eq!(catalog.models.len(), 2);

        let caps = capabilities_from_catalog(&catalog, true);
        assert_eq!(caps.models[0].id, "anthropic:claude-opus-5");
        assert_eq!(caps.models[0].label, "Anthropic · claude-opus-5");
        assert_eq!(caps.models[0].sub_provider.as_deref(), Some("anthropic"));
        assert!(
            caps.models.iter().all(|model| model.supports_images),
            "image support is a session-level fact applied to every model"
        );
    }

    #[test]
    fn the_launch_default_mode_stays_pinned_when_the_session_is_in_another_one() {
        // currentModeId is accept_edits, but a NEW session must still
        // launch ask-first — the Rust fallback and the frontend default
        // table both hard-code that string.
        let caps =
            capabilities_from_catalog(&parse_session_catalog(&session_new_response()), false);
        assert_eq!(
            caps.default_permission_mode.as_deref(),
            Some(HERMES_DEFAULT_PERMISSION_MODE)
        );
        let marked: Vec<&str> = caps
            .permission_modes
            .iter()
            .filter(|mode| mode.is_default)
            .map(|mode| mode.value.as_str())
            .collect();
        assert_eq!(marked, vec![HERMES_DEFAULT_PERMISSION_MODE]);
    }

    #[test]
    fn an_unrecognised_session_new_payload_degrades_to_an_empty_catalogue() {
        for payload in [
            serde_json::json!({}),
            serde_json::json!({"models": null, "modes": null}),
            serde_json::json!({"models": {"availableModels": []}}),
            // A shape change upstream: models arrives as a bare list.
            serde_json::json!({"models": ["anthropic:claude-opus-5"]}),
        ] {
            let catalog = parse_session_catalog(&payload);
            assert!(catalog.is_empty(), "unexpected catalogue for {payload}");
        }

        // An empty catalogue still yields a usable picker: the static
        // modes survive so the permission control keeps rendering.
        let caps = capabilities_from_catalog(&HermesSessionCatalog::default(), false);
        assert!(caps.models.is_empty());
        assert_eq!(caps.permission_modes.len(), 3);
    }

    #[test]
    fn a_model_entry_without_a_name_falls_back_to_the_bare_model_id() {
        let catalog = parse_session_catalog(&serde_json::json!({
            "models": {"availableModels": [
                {"modelId": "openrouter:anthropic/claude-sonnet-4"},
                {"modelId": "  "},
                {"modelId": "bare-model-no-provider"}
            ]}
        }));
        assert_eq!(catalog.models.len(), 2, "the blank id is dropped");
        // Only the FIRST colon separates provider from model.
        assert_eq!(catalog.models[0].label, "anthropic/claude-sonnet-4");
        assert_eq!(
            catalog.models[0].sub_provider.as_deref(),
            Some("openrouter")
        );
        assert_eq!(catalog.models[1].label, "bare-model-no-provider");
        assert_eq!(catalog.models[1].sub_provider, None);
    }

    #[tokio::test]
    async fn the_cache_is_per_profile_and_warms_lazily() {
        let temp = fixture_root();
        write(
            &temp.path().join("profiles/sidecar/config.yaml"),
            "model:\n  default: claude-opus-5\n  provider: anthropic\n",
        );
        write(&temp.path().join("profiles/sidecar/auth.json"), "{\"t\":1}");

        let cache = HermesCapabilityCache::new();
        assert!(
            cache.peek("default").await.is_none(),
            "nothing is warmed until the rail opens"
        );

        let default = cache
            .get_or_seed(temp.path(), "default", true)
            .await
            .expect("seeded");
        let sidecar = cache
            .get_or_seed(temp.path(), "sidecar", true)
            .await
            .expect("seeded");
        assert_eq!(default.models[0].id, "openai-codex:gpt-5.6-sol");
        assert_eq!(sidecar.models[0].id, "anthropic:claude-opus-5");

        // A live catalogue replaces one profile's entry only.
        let catalog = parse_session_catalog(&session_new_response());
        assert!(
            cache
                .record_session_catalog("default", &catalog, true)
                .await
        );
        assert_eq!(
            cache.peek("default").await.expect("cached").models[0].id,
            "anthropic:claude-opus-5"
        );
        assert_eq!(
            cache.peek("sidecar").await.expect("cached").models[0].id,
            "anthropic:claude-opus-5",
            "sidecar was seeded with the same id, untouched by the default profile's harvest"
        );
        assert_eq!(cache.peek("sidecar").await.expect("cached").models.len(), 1);

        // An empty catalogue must never blank a working picker.
        assert!(
            !cache
                .record_session_catalog("default", &HermesSessionCatalog::default(), true)
                .await
        );
        assert_eq!(
            cache.peek("default").await.expect("cached").models.len(),
            2,
            "the harvested list survives an empty follow-up"
        );

        cache.invalidate(Some("default")).await;
        assert!(cache.peek("default").await.is_none());
        assert!(cache.peek("sidecar").await.is_some());
        cache.invalidate(None).await;
        assert!(cache.peek("sidecar").await.is_none());
    }
}
