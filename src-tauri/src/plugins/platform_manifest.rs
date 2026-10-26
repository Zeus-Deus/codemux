//! Static v1 contracts. Inspection never imports a plugin or silently drops requirements.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

pub const MANIFEST_FILE: &str = "codemux-plugin.json";
pub const MAX_MANIFEST_BYTES: usize = 65_536;
pub const API_VERSION: u64 = 1;
pub const PLATFORM_CAPABILITIES: &[&str] = &[
    "storage",
    "settings",
    "threads.read",
    "threads.send",
    "threads.create",
    "events.run-finalized",
    "ui",
    "ui.sidebar-views",
    "ui.native-trees",
    "ui.conversation-cards",
    "ui.changes",
    "worktrees.create",
    "workflow.query",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformAction {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub placements: Vec<String>,
    #[serde(default)]
    pub agent_callable: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformView {
    pub id: String,
    pub title: String,
    pub entry: String,
    pub placement: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformStatus {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub view_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformNativeTree {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSetting {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub optional_capabilities: Vec<String>,
    #[serde(default)]
    pub actions: Vec<PlatformAction>,
    #[serde(default)]
    pub views: Vec<PlatformView>,
    #[serde(default, alias = "status")]
    pub thread_status: Vec<PlatformStatus>,
    #[serde(default)]
    pub native_trees: Vec<PlatformNativeTree>,
    #[serde(default)]
    pub sidebar_views: Vec<Value>,
    #[serde(default)]
    pub settings: Vec<PlatformSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
pub type PluginManifest = PlatformManifest;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCompatibility {
    pub available: bool,
    pub api_version: u64,
    pub missing_capabilities: Vec<String>,
    pub unsupported_optional_capabilities: Vec<String>,
    pub unsupported_requirements: Vec<String>,
}

pub fn valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
pub fn safe_package_path(directory: &Path, entry: &str) -> Result<PathBuf, String> {
    if !directory.is_absolute()
        || entry.is_empty()
        || entry.len() > 512
        || entry.contains(['\\', ':', '\0'])
        || entry
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || Path::new(entry)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err("Plugin entries must be safe relative paths".into());
    }
    Ok(directory.join(entry))
}
fn unique<'a>(values: impl IntoIterator<Item = &'a str>, max: usize) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !valid_identifier(value) || !seen.insert(value) || seen.len() > max {
            return Err("Invalid, duplicate or excessive plugin declarations".into());
        }
    }
    Ok(())
}
fn text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.chars().count() <= max
}
impl PlatformManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("Plugin manifest exceeds 64 KiB".into());
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid plugin manifest: {e}"))?;
        if !valid_identifier(&manifest.id)
            || !text(&manifest.name, 200)
            || manifest.version.len() > 128
            || semver::Version::parse(&manifest.version).is_err()
        {
            return Err("Plugin id, name or semantic version is invalid".into());
        }
        // Future schemas remain inspectable. Their declarations never become executable on v1.
        if manifest.api_version != API_VERSION {
            return Ok(manifest);
        }
        safe_package_path(
            Path::new(if cfg!(windows) {
                "C:/snapshot"
            } else {
                "/snapshot"
            }),
            &manifest.entry,
        )?;
        if !manifest.entry.ends_with(".js") && !manifest.entry.ends_with(".mjs") {
            return Err("Plugin entry must be JavaScript (.js or .mjs)".into());
        }
        unique(manifest.capabilities.iter().map(String::as_str), 64)?;
        unique(
            manifest.required_capabilities.iter().map(String::as_str),
            64,
        )?;
        unique(
            manifest.optional_capabilities.iter().map(String::as_str),
            64,
        )?;
        unique(manifest.actions.iter().map(|v| v.id.as_str()), 100)?;
        unique(manifest.views.iter().map(|v| v.id.as_str()), 30)?;
        unique(manifest.thread_status.iter().map(|v| v.id.as_str()), 4)?;
        unique(manifest.native_trees.iter().map(|v| v.id.as_str()), 2)?;
        unique(manifest.settings.iter().map(|v| v.key.as_str()), 100)?;
        unique(
            manifest
                .sidebar_views
                .iter()
                .map(|v| v.get("id").and_then(Value::as_str).unwrap_or("")),
            2,
        )?;
        for view in &manifest.sidebar_views {
            if !view
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|s| text(s, 40))
                || view.get("groupBy").and_then(Value::as_str) != Some("project")
                || view
                    .get("quietAfterDays")
                    .is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=365).contains(&n)))
                || view
                    .get("emptyProjects")
                    .is_some_and(|v| !v.as_str().is_some_and(|s| ["show", "hide"].contains(&s)))
            {
                return Err("Invalid sidebar view declaration".into());
            }
        }
        for a in &manifest.actions {
            if !text(&a.title, 200)
                || a.placements.len() > 3
                || a.placements
                    .iter()
                    .any(|p| !["palette", "thread-menu", "message-selection"].contains(&p.as_str()))
            {
                return Err("Invalid action declaration".into());
            }
        }
        for v in &manifest.views {
            safe_package_path(
                Path::new(if cfg!(windows) {
                    "C:/snapshot"
                } else {
                    "/snapshot"
                }),
                &v.entry,
            )?;
            if !text(&v.title, 200)
                || !v.entry.ends_with(".html")
                || !["page", "side-panel", "conversation"].contains(&v.placement.as_str())
            {
                return Err("Invalid view declaration".into());
            }
        }
        for s in &manifest.thread_status {
            if !text(&s.title, 200)
                || s.view_id.as_ref().is_some_and(|id| {
                    !manifest
                        .views
                        .iter()
                        .any(|v| &v.id == id && v.placement == "side-panel")
                })
            {
                return Err("Thread status must open a declared side-panel view".into());
            }
        }
        for s in &manifest.settings {
            if !text(&s.label, 200)
                || !["string", "number", "boolean", "secret"].contains(&s.kind.as_str())
                || s.default.as_ref().is_some_and(|v| match s.kind.as_str() {
                    "string" => !v.is_string(),
                    "number" => !v.is_number(),
                    "boolean" => !v.is_boolean(),
                    _ => true,
                })
            {
                return Err("Invalid setting type/default (secrets cannot have defaults)".into());
            }
        }
        let requested = manifest.requested_capabilities();
        for (declared, required) in [
            (!manifest.native_trees.is_empty(), "ui.native-trees"),
            (!manifest.sidebar_views.is_empty(), "ui.sidebar-views"),
            (
                manifest.views.iter().any(|v| v.placement == "conversation"),
                "ui.conversation-cards",
            ),
        ] {
            if declared && !requested.iter().any(|c| c == required) {
                return Err(format!("Contribution requires the {required} capability"));
            }
        }
        if manifest.native_trees.iter().any(|v| !text(&v.title, 40))
            || manifest.sidebar_views.len() > 2
        {
            return Err("Invalid tree/sidebar declaration".into());
        }
        Ok(manifest)
    }
    pub fn read(directory: &Path) -> Result<Self, String> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join(MANIFEST_FILE))
            .map_err(|e| e.to_string())?
            .take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::parse(&bytes)
    }
    pub fn requested_capabilities(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .chain(&self.required_capabilities)
            .chain(&self.optional_capabilities)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn compatibility(&self) -> PlatformCompatibility {
        self.compatibility_with(PLATFORM_CAPABILITIES)
    }
    pub fn compatibility_with(&self, supported: &[&str]) -> PlatformCompatibility {
        let mut unsupported_requirements = Vec::new();
        if let Some(requires) = &self.requires {
            // Unknown requirements are fail-closed, while unknown descriptive fields are retained.
            if let Some(map) = requires.as_object() {
                for (key, value) in map {
                    let satisfied = match key.as_str() {
                        "apiVersion" => value.as_u64() == Some(API_VERSION),
                        "hostVersion" | "codemux" => value
                            .as_str()
                            .and_then(|s| semver::VersionReq::parse(s).ok())
                            .is_some_and(|r| {
                                semver::Version::parse(env!("CARGO_PKG_VERSION"))
                                    .is_ok_and(|v| r.matches(&v))
                            }),
                        "capabilities" => value.as_array().is_some_and(|a| {
                            a.iter()
                                .all(|v| v.as_str().is_some_and(|s| supported.contains(&s)))
                        }),
                        _ => false,
                    };
                    if !satisfied {
                        unsupported_requirements.push(key.clone());
                    }
                }
            } else {
                unsupported_requirements.push("requires".into());
            }
        }
        let missing_capabilities: Vec<_> = self
            .capabilities
            .iter()
            .chain(&self.required_capabilities)
            .filter(|c| !supported.contains(&c.as_str()))
            .cloned()
            .collect();
        let unsupported_optional_capabilities = self
            .optional_capabilities
            .iter()
            .filter(|c| !supported.contains(&c.as_str()))
            .cloned()
            .collect();
        PlatformCompatibility {
            available: self.api_version == API_VERSION
                && missing_capabilities.is_empty()
                && unsupported_requirements.is_empty(),
            api_version: self.api_version,
            missing_capabilities,
            unsupported_optional_capabilities,
            unsupported_requirements,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(extra: Value) -> Result<PlatformManifest, String> {
        let mut value = serde_json::json!({"id":"example.test","name":"Test","version":"1.0.0","apiVersion":1,"entry":"main.mjs"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        PlatformManifest::parse(&serde_json::to_vec(&value).unwrap())
    }
    #[test]
    fn unknown_requirements_are_retained_and_block_execution() {
        let m = parse(serde_json::json!({"capabilities":["future.required"],"optionalCapabilities":["future.optional"],"futureField":{"x":1}})).unwrap();
        assert!(!m.compatibility().available);
        assert!(m.extra.contains_key("futureField"));
        assert_eq!(
            m.compatibility().unsupported_optional_capabilities,
            ["future.optional"]
        );
        assert!(
            !parse(serde_json::json!({"requires":{"futureRequirement":true}}))
                .unwrap()
                .compatibility()
                .available
        );
        assert!(
            !parse(serde_json::json!({"apiVersion":2}))
                .unwrap()
                .compatibility()
                .available
        );
        assert!(
            parse(serde_json::json!({"optionalCapabilities":["future.optional"]}))
                .unwrap()
                .compatibility()
                .available
        );
    }
    #[test]
    fn rejects_path_duplicates_and_secret_defaults_before_execution() {
        for bad in [
            serde_json::json!({"entry":"../escape.js"}),
            serde_json::json!({"capabilities":["storage","storage"]}),
            serde_json::json!({"settings":[{"key":"token","label":"Token","type":"secret","default":"oops"}]}),
        ] {
            assert!(parse(bad).is_err());
        }
        assert!(parse(serde_json::json!({"version":"01.0.0"})).is_err());
    }
}
