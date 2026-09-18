use crate::{ErrorCode, ProtocolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub api: String,
    pub entry: String,
    pub platforms: Vec<Platform>,
    pub author: Author,
    pub repository: String,
    pub license: String,
    pub permissions: Vec<Permission>,
    pub http: Vec<HttpGrant>,
    pub credentials: Vec<Credential>,
    pub contributes: Contributions,
    pub settings: Vec<Setting>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    pub url: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Platform {
    #[serde(rename = "linux-x64")]
    LinuxX64,
    #[serde(rename = "windows-x64")]
    WindowsX64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Permission {
    #[serde(rename = "workspace.read")]
    WorkspaceRead,
    #[serde(rename = "git.read")]
    GitRead,
    #[serde(rename = "composer.append")]
    ComposerAppend,
    #[serde(rename = "external.open")]
    ExternalOpen,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    PATCH,
    DELETE,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpGrant {
    pub origin: String,
    pub methods: Vec<HttpMethod>,
    pub credential: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub id: String,
    pub label: String,
    pub origin: String,
    #[serde(rename = "type")]
    pub kind: CredentialType,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum CredentialType {
    #[serde(rename = "bearer")]
    Bearer,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Contributions {
    pub commands: Vec<Command>,
    pub panels: Vec<View>,
    pub composer_actions: Vec<View>,
    pub composer_views: Vec<View>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Command {
    pub id: String,
    pub title: String,
    pub requires_workspace: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub id: String,
    pub title: String,
    pub icon: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "lowercase")]
pub enum Setting {
    Boolean {
        id: String,
        label: String,
        default: bool,
    },
    String {
        id: String,
        label: String,
        #[serde(default)]
        default: String,
    },
    Integer {
        id: String,
        label: String,
        default: i64,
        min: i64,
        max: i64,
    },
    Enum {
        id: String,
        label: String,
        default: String,
        values: Vec<String>,
    },
}
impl Setting {
    pub fn id(&self) -> &str {
        match self {
            Self::Boolean { id, .. }
            | Self::String { id, .. }
            | Self::Integer { id, .. }
            | Self::Enum { id, .. } => id,
        }
    }
    pub fn default_value(&self) -> serde_json::Value {
        match self {
            Self::Boolean { default, .. } => (*default).into(),
            Self::String { default, .. } | Self::Enum { default, .. } => default.clone().into(),
            Self::Integer { default, .. } => (*default).into(),
        }
    }
    pub fn accepts(&self, value: &serde_json::Value) -> bool {
        match self {
            Self::Boolean { .. } => value.is_boolean(),
            Self::String { .. } => value.as_str().is_some_and(|s| s.len() <= 4096),
            Self::Integer { min, max, .. } => {
                value.as_i64().is_some_and(|v| v >= *min && v <= *max)
            }
            Self::Enum { values, .. } => value
                .as_str()
                .is_some_and(|v| values.iter().any(|s| s == v)),
        }
    }
}
pub const ICONS: &[&str] = &[
    "file-text",
    "git-branch",
    "github",
    "list",
    "check",
    "info",
    "settings",
    "book-open",
    "link",
    "refresh-cw",
    "plus",
    "circle-alert",
    "folder",
    "terminal",
    "code",
    "search",
];
pub fn local_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn plugin_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('.').collect();
    parts.len() == 2 && parts.iter().all(|p| p.len() >= 2 && local_id(p))
}
pub fn https_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
pub fn origin(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        https_url(value)
            && matches!(u.host(), Some(url::Host::Domain(_)))
            && u.port_or_known_default() == Some(443)
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
            && u.origin().ascii_serialization() == value
    })
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}
fn unique<T: Eq + std::hash::Hash>(values: impl IntoIterator<Item = T>) -> bool {
    let mut seen = HashSet::new();
    values.into_iter().all(|v| seen.insert(v))
}
impl Manifest {
    pub fn parse(bytes: &[u8], platform: Option<Platform>) -> Result<Self, ProtocolError> {
        if bytes.len() > crate::limits::MANIFEST {
            return Err(ProtocolError::invalid("Manifest exceeds 64 KiB"));
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| {
            ProtocolError::invalid("Manifest does not match the feature-plugin schema")
        })?;
        manifest.validate(platform)?;
        Ok(manifest)
    }
    pub fn validate(&self, platform: Option<Platform>) -> Result<(), ProtocolError> {
        let fail = |message| Err(ProtocolError::invalid(message));
        if self.format != "codemux.feature-plugin"
            || self.manifest_version != 1
            || self.entry != "plugin.js"
            || !plugin_id(&self.id)
        {
            return fail("Unsupported package format or identity");
        }
        if !bounded(&self.name, 80)
            || !bounded(&self.description, 240)
            || !bounded(&self.author.name, 80)
            || !bounded(&self.license, 100)
            || !https_url(&self.author.url)
            || !https_url(&self.repository)
        {
            return fail("Invalid package metadata");
        }
        if semver::Version::parse(&self.version).is_err() {
            return fail("Invalid package version");
        }
        let range = semver::VersionReq::parse(&self.api)
            .map_err(|_| ProtocolError::invalid("Invalid API range"))?;
        if !range.matches(&semver::Version::new(1, 0, 0)) {
            return Err(ProtocolError::new(
                ErrorCode::IncompatibleApi,
                "Requires an unsupported plugin API",
            ));
        }
        if self.platforms.is_empty()
            || !unique(&self.platforms)
            || platform.is_some_and(|p| !self.platforms.contains(&p))
        {
            return fail("Unsupported platform");
        }
        if !unique(&self.permissions)
            || self.http.len() > 20
            || self.credentials.len() > 20
            || !unique(self.http.iter().map(|h| &h.origin))
            || !unique(self.credentials.iter().map(|c| &c.id))
            || !unique(self.credentials.iter().map(|c| &c.origin))
        {
            return fail("Duplicate or excessive capabilities");
        }
        for grant in &self.http {
            if !origin(&grant.origin) || grant.methods.is_empty() || !unique(&grant.methods) {
                return fail("Invalid HTTP grant");
            }
            if let Some(id) = &grant.credential {
                if !self
                    .credentials
                    .iter()
                    .any(|c| c.id == *id && c.origin == grant.origin)
                {
                    return fail("HTTP grant references undeclared credential");
                }
            }
        }
        for c in &self.credentials {
            if !local_id(&c.id)
                || !bounded(&c.label, 80)
                || !origin(&c.origin)
                || !self
                    .http
                    .iter()
                    .any(|h| h.origin == c.origin && h.credential.as_ref() == Some(&c.id))
            {
                return fail("Invalid credential declaration");
            }
        }
        let c = &self.contributes;
        if c.commands.len() > 20
            || c.panels.len() > 8
            || c.composer_actions.len() > 8
            || c.composer_views.len() > 4
        {
            return fail("Too many contributions");
        }
        if !unique(c.commands.iter().map(|c| &c.id))
            || c.commands
                .iter()
                .any(|c| !local_id(&c.id) || !bounded(&c.title, 80))
        {
            return fail("Invalid command declaration");
        }
        for views in [&c.panels, &c.composer_actions, &c.composer_views] {
            if !unique(views.iter().map(|v| &v.id))
                || views.iter().any(|v| {
                    !local_id(&v.id) || !bounded(&v.title, 80) || !ICONS.contains(&v.icon.as_str())
                })
            {
                return fail("Invalid view declaration");
            }
        }
        if self.settings.len() > 50 || !unique(self.settings.iter().map(Setting::id)) {
            return fail("Duplicate or excessive settings");
        }
        for s in &self.settings {
            let label = match s {
                Setting::Boolean { label, .. }
                | Setting::String { label, .. }
                | Setting::Integer { label, .. }
                | Setting::Enum { label, .. } => label,
            };
            if !local_id(s.id()) || !bounded(label, 80) || !s.accepts(&s.default_value()) {
                return fail("Invalid setting or default");
            }
            if let Setting::Enum { values, .. } = s {
                if values.is_empty()
                    || values.len() > 50
                    || !unique(values)
                    || values.iter().any(|v| !bounded(v, 4096))
                {
                    return fail("Invalid enum setting");
                }
            }
        }
        Ok(())
    }
}
