use crate::{ErrorCode, ProtocolError};
use schemars::{
    gen::SchemaGenerator,
    schema::{InstanceType, Schema, SchemaObject},
    JsonSchema,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The plugin API version this host implements.
pub const API: semver::Version = semver::Version::new(1, 0, 0);
// ECMA-262 forms of the rules enforced by `Manifest::validate`, carried by the
// generated JSON Schema for editors and the author CLI. Validation stays in Rust.
const PLUGIN_ID: &str = r"^(?!(?:con|prn|aux|nul|com[0-9]|lpt[0-9])\.)[a-z][a-z0-9-]{1,39}\.(?!(?:con|prn|aux|nul|com[0-9]|lpt[0-9])$)[a-z][a-z0-9-]{1,39}$";
const LOCAL_ID: &str = r"^[a-z][a-z0-9-]{0,39}$";
const TEXT: &str = r"^[^\u0000-\u001F\u007F-\u009F]*$";
const HTTPS: &str = r"^https://";
const ORIGIN: &str = r"^https://(?!.*\.(?:[0-9]+|0x[0-9a-f]*)$)[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$";
const SEMVER: &str = r"^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*))*)?(?:\+[0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*)?$";
fn strings(values: &[&str]) -> Schema {
    SchemaObject {
        instance_type: Some(InstanceType::String.into()),
        enum_values: Some(values.iter().map(|v| (*v).into()).collect()),
        ..Default::default()
    }
    .into()
}
fn format_schema(_: &mut SchemaGenerator) -> Schema {
    strings(&["codemux.feature-plugin"])
}
fn entry_schema(_: &mut SchemaGenerator) -> Schema {
    strings(&["plugin.js"])
}
fn icon_schema(_: &mut SchemaGenerator) -> Schema {
    strings(ICONS)
}
fn unique_items<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = generator.subschema_for::<Vec<T>>().into_object();
    schema.array().unique_items = Some(true);
    schema.into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    #[schemars(schema_with = "format_schema")]
    pub format: String,
    #[schemars(range(min = 1, max = 1))]
    pub manifest_version: u32,
    #[schemars(regex = "PLUGIN_ID")]
    pub id: String,
    #[schemars(length(min = 1, max = 80), regex = "TEXT")]
    pub name: String,
    #[schemars(length(min = 1, max = 240), regex = "TEXT")]
    pub description: String,
    #[schemars(regex = "SEMVER")]
    pub version: String,
    pub api: String,
    #[schemars(schema_with = "entry_schema")]
    pub entry: String,
    #[schemars(schema_with = "unique_items::<Platform>", length(min = 1))]
    pub platforms: Vec<Platform>,
    pub author: Author,
    #[schemars(regex = "HTTPS")]
    pub repository: String,
    #[schemars(length(min = 1, max = 100), regex = "TEXT")]
    pub license: String,
    #[schemars(schema_with = "unique_items::<Permission>")]
    pub permissions: Vec<Permission>,
    #[schemars(length(max = 20))]
    pub http: Vec<HttpGrant>,
    #[schemars(length(max = 20))]
    pub credentials: Vec<Credential>,
    pub contributes: Contributions,
    #[schemars(length(max = 50))]
    pub settings: Vec<Setting>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Author {
    #[schemars(length(min = 1, max = 80), regex = "TEXT")]
    pub name: String,
    #[schemars(regex = "HTTPS")]
    pub url: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Platform {
    #[serde(rename = "linux-x64")]
    LinuxX64,
    #[serde(rename = "windows-x64")]
    WindowsX64,
}
impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Self::LinuxX64 => "Linux x64",
            Self::WindowsX64 => "Windows x64",
        }
    }
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
    #[schemars(regex = "ORIGIN")]
    pub origin: String,
    #[schemars(schema_with = "unique_items::<HttpMethod>", length(min = 1))]
    pub methods: Vec<HttpMethod>,
    #[serde(deserialize_with = "required_nullable_string")]
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub credential: Option<String>,
}
fn required_nullable_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn nullable_string_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    generator.subschema_for::<Option<String>>()
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    #[schemars(regex = "LOCAL_ID")]
    pub id: String,
    #[schemars(length(min = 1, max = 80), regex = "TEXT")]
    pub label: String,
    #[schemars(regex = "ORIGIN")]
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
    #[schemars(length(max = 20))]
    pub commands: Vec<Command>,
    #[schemars(length(max = 8))]
    pub panels: Vec<View>,
    #[schemars(length(max = 8))]
    pub composer_actions: Vec<View>,
    #[schemars(length(max = 4))]
    pub composer_views: Vec<View>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Command {
    #[schemars(regex = "LOCAL_ID")]
    pub id: String,
    #[schemars(length(min = 1, max = 80), regex = "TEXT")]
    pub title: String,
    pub requires_workspace: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct View {
    #[schemars(regex = "LOCAL_ID")]
    pub id: String,
    #[schemars(length(min = 1, max = 80), regex = "TEXT")]
    pub title: String,
    #[schemars(schema_with = "icon_schema")]
    pub icon: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "lowercase")]
pub enum Setting {
    Boolean {
        #[schemars(regex = "LOCAL_ID")]
        id: String,
        #[schemars(length(min = 1, max = 80), regex = "TEXT")]
        label: String,
        default: bool,
    },
    String {
        #[schemars(regex = "LOCAL_ID")]
        id: String,
        #[schemars(length(min = 1, max = 80), regex = "TEXT")]
        label: String,
        #[serde(default)]
        #[schemars(length(max = 4096))]
        default: String,
    },
    Integer {
        #[schemars(regex = "LOCAL_ID")]
        id: String,
        #[schemars(length(min = 1, max = 80), regex = "TEXT")]
        label: String,
        default: i64,
        min: i64,
        max: i64,
    },
    Enum {
        #[schemars(regex = "LOCAL_ID")]
        id: String,
        #[schemars(length(min = 1, max = 80), regex = "TEXT")]
        label: String,
        default: String,
        #[schemars(
            schema_with = "unique_items::<String>",
            length(min = 1, max = 50),
            inner(length(min = 1, max = 4096), regex = "TEXT")
        )]
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
/// Windows opens these names as devices even with an extension, and a plugin ID
/// names its package directory.
fn reserved_device(segment: &str) -> bool {
    matches!(segment, "con" | "prn" | "aux" | "nul")
        || (segment.len() == 4
            && (segment.starts_with("com") || segment.starts_with("lpt"))
            && segment.as_bytes()[3].is_ascii_digit())
}
pub fn plugin_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('.').collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| p.len() >= 2 && local_id(p) && !reserved_device(p))
}
pub fn https_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
/// A multi-label DNS name of lowercase letter-digit-hyphen labels. URL host
/// parsing alone admits `*`, `_`, a trailing dot, and single-label hosts.
fn dns_name(host: &str) -> bool {
    let labels = host.split('.').collect::<Vec<_>>();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
pub fn origin(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        https_url(value)
            && matches!(u.host(), Some(url::Host::Domain(host)) if dns_name(host))
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
        if !range.matches(&API) {
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
