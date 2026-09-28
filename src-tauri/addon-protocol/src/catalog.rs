//! Reviewed, inert release metadata shared by the installer and publication tools.
use crate::{
    manifest::{self, Credential, HttpGrant, Permission, Platform, API},
    ErrorCode, Manifest, ProtocolError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
type Result<T> = std::result::Result<T, ProtocolError>;
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub revision: u64,
    pub generated_at: String,
    pub plugins: Vec<Plugin>,
    pub blocked: Vec<Blocked>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub tier: Tier,
    pub repository: String,
    pub description: String,
    pub readme: String,
    pub releases: Vec<Release>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Official,
    Community,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub api: String,
    pub platforms: Vec<Platform>,
    pub source_commit: String,
    pub download_url: String,
    pub sha256: String,
    pub compressed_bytes: u64,
    pub published_at: String,
    pub license: String,
    pub capabilities: Capabilities,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub permissions: Vec<Permission>,
    pub http: Vec<HttpGrant>,
    pub credentials: Vec<Credential>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Blocked {
    pub plugin_id: Option<String>,
    pub sha256: Option<String>,
    pub reason: String,
    pub date: String,
}
fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid reviewed add-on catalog")
}
fn bounded(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}
pub fn hex(s: &str, length: usize) -> bool {
    s.len() == length
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn repository(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("github.com")
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && {
                let segments = u.path().split('/').skip(1).collect::<Vec<_>>();
                segments.len() == 2
                    && segments.iter().all(|s| {
                        !s.is_empty()
                            && *s != "."
                            && *s != ".."
                            && s.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    })
                    && !s.ends_with(".git")
            }
    })
}
pub fn asset_url(repository: &str, value: &str) -> bool {
    if !self::repository(repository) {
        return false;
    }
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("github.com")
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && {
                let prefix = format!(
                    "{}/releases/download/",
                    url::Url::parse(repository).unwrap().path()
                );
                u.path().strip_prefix(&prefix).is_some_and(|rest| {
                    let parts = rest.split('/').collect::<Vec<_>>();
                    parts.len() == 2
                        && parts
                            .iter()
                            .all(|p| !p.is_empty() && *p != "." && *p != "..")
                        && parts[1].ends_with(".cmxaddon")
                })
            }
    })
}
fn timestamp(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}
impl Capabilities {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            permissions: manifest.permissions.clone(),
            http: manifest.http.clone(),
            credentials: manifest.credentials.clone(),
        }
    }
    pub fn normalized(&self) -> serde_json::Value {
        let mut permissions = self
            .permissions
            .iter()
            .map(|v| serde_json::to_value(v).unwrap())
            .collect::<Vec<_>>();
        permissions.sort_by_key(|v| v.to_string());
        let mut http = self.http.clone();
        for grant in &mut http {
            grant
                .methods
                .sort_by_key(|v| serde_json::to_string(v).unwrap());
        }
        http.sort_by(|a, b| a.origin.cmp(&b.origin));
        let mut credentials = self.credentials.clone();
        credentials.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::json!({"permissions":permissions,"http":http,"credentials":credentials})
    }
}
impl Catalog {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid());
        }
        let catalog: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        catalog.validate()?;
        Ok(catalog)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || !timestamp(&self.generated_at)
            || self.plugins.len() > 5000
            || self.blocked.len() > 5000
        {
            return Err(invalid());
        }
        let mut ids = HashSet::new();
        let mut digests = BTreeMap::new();
        for plugin in &self.plugins {
            if !manifest::plugin_id(&plugin.id)
                || !ids.insert(&plugin.id)
                || !bounded(&plugin.name, 80)
                || !bounded(&plugin.publisher, 80)
                || !bounded(&plugin.description, 240)
                || plugin.readme.len() > 32 * 1024
                || !repository(&plugin.repository)
                || plugin.releases.is_empty()
                || plugin.releases.len() > 100
            {
                return Err(invalid());
            }
            let mut versions = HashSet::new();
            for release in &plugin.releases {
                let version = semver::Version::parse(&release.version).map_err(|_| invalid())?;
                if !version.pre.is_empty()
                    || !version.build.is_empty()
                    || !versions.insert(&release.version)
                    || semver::VersionReq::parse(&release.api).is_err()
                    || !hex(&release.sha256, 64)
                    || !hex(&release.source_commit, 40)
                    || release.compressed_bytes == 0
                    || release.compressed_bytes > 10 * 1024 * 1024
                    || !timestamp(&release.published_at)
                    || !asset_url(&plugin.repository, &release.download_url)
                {
                    return Err(invalid());
                }
                if let Some(owner) = digests.insert(&release.sha256, &plugin.id) {
                    if owner != &plugin.id {
                        return Err(invalid());
                    }
                }
                // Reuse strict capability validation independently of API compatibility.
                let mut manifest = Manifest::parse(include_bytes!("../fixtures/hello.json"), None)?;
                manifest.permissions = release.capabilities.permissions.clone();
                manifest.http = release.capabilities.http.clone();
                manifest.credentials = release.capabilities.credentials.clone();
                manifest.platforms = release.platforms.clone();
                manifest.license = release.license.clone();
                manifest.validate(None)?;
                if release.capabilities.normalized()
                    != serde_json::to_value(&release.capabilities).unwrap()
                {
                    return Err(ProtocolError::invalid(
                        "Catalog capabilities must be normalized",
                    ));
                }
            }
        }
        for blocked in &self.blocked {
            if blocked.plugin_id.is_none() == blocked.sha256.is_none()
                || blocked
                    .plugin_id
                    .as_ref()
                    .is_some_and(|id| !manifest::plugin_id(id))
                || blocked.sha256.as_ref().is_some_and(|h| !hex(h, 64))
                || !bounded(&blocked.reason, 240)
                || !timestamp(&blocked.date)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn blocked_reason(&self, id: &str, digest: &str) -> Option<&str> {
        self.blocked
            .iter()
            .find(|b| b.plugin_id.as_deref() == Some(id) || b.sha256.as_deref() == Some(digest))
            .map(|b| b.reason.as_str())
    }
    pub fn check_successor(&self, previous: &Self) -> Result<()> {
        if self.revision < previous.revision {
            return Err(ProtocolError::invalid("Catalog revision rollback refused"));
        }
        if self.revision == previous.revision
            && serde_json::to_value(self).unwrap() != serde_json::to_value(previous).unwrap()
        {
            return Err(ProtocolError::invalid(
                "Catalog revision was reused with different content",
            ));
        }
        for old in &previous.plugins {
            let new = self
                .plugins
                .iter()
                .find(|p| p.id == old.id)
                .ok_or_else(|| {
                    ProtocolError::invalid(
                        "Catalog must retain source ownership history; block withdrawn plugins",
                    )
                })?;
            {
                if new.publisher != old.publisher || new.repository != old.repository {
                    return Err(ProtocolError::invalid("Catalog source ownership changed"));
                }
                for release in &old.releases {
                    let next = new.releases.iter().find(|r| r.version == release.version).ok_or_else(|| {
                        ProtocolError::invalid("Catalog must retain immutable release history; block withdrawn releases")
                    })?;
                    {
                        if serde_json::to_value(next).unwrap()
                            != serde_json::to_value(release).unwrap()
                        {
                            return Err(ProtocolError::invalid("An accepted release is immutable"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// An ID selects the highest installable release; an explicit version resolves
    /// exactly. A refusal names its cause: a missing entry or version, a block, the
    /// platform, or the plugin API. Without a version, it reports the newest
    /// release that came closest to installable.
    pub fn select(&self, target: &str, platform: Platform) -> Result<(&Plugin, &Release)> {
        let (id, version) = install_target(target)?;
        let plugin = self.plugins.iter().find(|p| p.id == id).ok_or_else(|| {
            ProtocolError::invalid(format!("{id} is not listed in the add-on catalog"))
        })?;
        let mut candidates = plugin
            .releases
            .iter()
            .filter(|r| version.as_ref().is_none_or(|v| v == &r.version))
            .collect::<Vec<_>>();
        candidates
            .sort_by_cached_key(|r| std::cmp::Reverse(semver::Version::parse(&r.version).ok()));
        // 0: blocked, 1: other platform, 2: incompatible API.
        let mut closest: Option<(u8, &Release)> = None;
        for release in candidates {
            let stage = if self.blocked_reason(&id, &release.sha256).is_some() {
                0
            } else if !release.platforms.contains(&platform) {
                1
            } else if !semver::VersionReq::parse(&release.api).is_ok_and(|r| r.matches(&API)) {
                2
            } else {
                return Ok((plugin, release));
            };
            if closest.is_none_or(|(best, _)| stage > best) {
                closest = Some((stage, release));
            }
        }
        let incompatible = |message| Err(ProtocolError::new(ErrorCode::IncompatibleApi, message));
        match (closest, version) {
            (None, Some(version)) => Err(ProtocolError::invalid(format!(
                "Version {version} of {id} is not listed in the add-on catalog"
            ))),
            (None, None) => Err(ProtocolError::invalid(format!(
                "{id} has no releases in the add-on catalog"
            ))),
            (Some((0, release)), _) => Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                format!(
                    "This release is blocked: {}",
                    self.blocked_reason(&id, &release.sha256)
                        .unwrap_or_default()
                ),
            )),
            (Some((1, _)), Some(version)) => incompatible(format!(
                "Version {version} of {id} is not available for {}",
                platform.label()
            )),
            (Some((1, _)), None) => incompatible(format!(
                "No release of {id} is available for {}",
                platform.label()
            )),
            (Some((_, release)), Some(version)) => incompatible(format!(
                "Version {version} of {id} requires add-on API {}; this CodeMux provides {API}",
                release.api
            )),
            (Some((_, release)), None) => incompatible(format!(
                "No release of {id} supports add-on API {API}; version {} requires {}",
                release.version, release.api
            )),
        }
    }
}
fn target_error() -> ProtocolError {
    ProtocolError::invalid("Use a catalog ID or a codemux.org add-on install link")
}
pub fn install_target(target: &str) -> Result<(String, Option<String>)> {
    if manifest::plugin_id(target) {
        return Ok((target.into(), None));
    }
    let u = url::Url::parse(target).map_err(|_| target_error())?;
    if u.scheme() != "https"
        || u.host_str() != Some("codemux.org")
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return Err(target_error());
    }
    let id = u
        .path()
        .strip_prefix("/addons/")
        .filter(|id| manifest::plugin_id(id))
        .ok_or_else(target_error)?;
    let query = u.query_pairs().collect::<Vec<_>>();
    if query.len() > 1 || query.first().is_some_and(|(k, _)| k != "version") {
        return Err(target_error());
    }
    let version = query.first().map(|(_, v)| v.to_string());
    if let Some(version) = &version {
        let parsed = semver::Version::parse(version).map_err(|_| {
            ProtocolError::invalid("The install link version is not a semantic version")
        })?;
        // Echoed in later messages, so keep it to the bounded stable form.
        if !parsed.pre.is_empty() || !parsed.build.is_empty() {
            return Err(ProtocolError::invalid(
                "The add-on catalog lists only stable release versions",
            ));
        }
    }
    Ok((id.into(), version))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn empty() -> Catalog {
        Catalog {
            schema_version: 1,
            revision: 1,
            generated_at: "2026-09-18T00:00:00Z".into(),
            plugins: vec![],
            blocked: vec![],
        }
    }
    #[test]
    fn revisions_and_revocations_are_strict() {
        let old = empty();
        old.validate().unwrap();
        let mut new = old.clone();
        new.revision = 0;
        assert!(new.check_successor(&old).is_err());
        new.revision = 1;
        new.generated_at = "2026-09-19T00:00:00Z".into();
        assert!(new.check_successor(&old).is_err());
        new.revision = 2;
        assert!(new.check_successor(&old).is_ok());
        new.blocked.push(Blocked {
            plugin_id: Some("test.plugin".into()),
            sha256: None,
            reason: "Revoked".into(),
            date: new.generated_at.clone(),
        });
        assert_eq!(
            new.blocked_reason("test.plugin", "anything"),
            Some("Revoked")
        );
        new.blocked[0].sha256 = Some("a".repeat(64));
        assert!(new.validate().is_err());
    }
    #[test]
    fn withdrawing_entries_cannot_erase_source_or_release_history() {
        let manifest = Manifest::parse(include_bytes!("../fixtures/hello.json"), None).unwrap();
        let mut old = empty();
        old.plugins.push(Plugin {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            publisher: "Fixture".into(),
            tier: Tier::Community,
            repository: "https://github.com/example/plugins".into(),
            description: "Synthetic catalog fixture".into(),
            readme: "Fixture".into(),
            releases: vec![Release {
                version: "1.0.0".into(),
                api: "^1.0".into(),
                platforms: manifest.platforms.clone(),
                source_commit: "a".repeat(40),
                download_url:
                    "https://github.com/example/plugins/releases/download/v1/fixture.cmxaddon"
                        .into(),
                sha256: "a".repeat(64),
                compressed_bytes: 100,
                published_at: old.generated_at.clone(),
                license: "MIT".into(),
                capabilities: Capabilities::from_manifest(&manifest),
            }],
        });
        old.validate().unwrap();
        let mut new = old.clone();
        new.revision += 1;
        new.plugins.clear();
        assert!(new.check_successor(&old).is_err());
        new.plugins = old.plugins.clone();
        new.plugins[0].releases.clear();
        assert!(new.check_successor(&old).is_err());
        new.plugins = old.plugins.clone();
        new.plugins[0].publisher = "Another publisher".into();
        assert!(new.check_successor(&old).is_err());
        new.plugins = old.plugins.clone();
        new.plugins[0].releases[0].sha256 = "b".repeat(64);
        assert!(new.check_successor(&old).is_err());
        new.plugins = old.plugins.clone();
        new.blocked.push(Blocked {
            plugin_id: Some(manifest.id.clone()),
            sha256: None,
            reason: "Withdrawn".into(),
            date: new.generated_at.clone(),
        });
        assert!(new.check_successor(&old).is_ok());
        assert!(new.select(&manifest.id, Platform::LinuxX64).is_err());
    }
    #[test]
    fn handoff_and_asset_authorities_are_exact() {
        assert_eq!(
            install_target("https://codemux.org/addons/test.plugin?version=1.2.3").unwrap(),
            ("test.plugin".into(), Some("1.2.3".into()))
        );
        for value in [
            "https://evil.test/addons/test.plugin",
            "https://codemux.org@evil.test/addons/test.plugin",
            "https://codemux.org/addons/test.plugin?path=/tmp/code",
            "https://codemux.org/addons/test.plugin?version=1.2.3&version=2.0.0",
        ] {
            assert!(install_target(value).is_err(), "{value}");
        }
        let repo = "https://github.com/example/plugins";
        assert!(asset_url(
            repo,
            "https://github.com/example/plugins/releases/download/v1/test.cmxaddon"
        ));
        for value in [
            "https://github.com/other/plugins/releases/download/v1/test.cmxaddon",
            "https://github.com/example/plugins/raw/main/test.cmxaddon",
            "https://github.com/example/plugins/releases/download/v1/test.cmxaddon?token=secret",
        ] {
            assert!(!asset_url(repo, value));
        }
    }
    fn release(version: &str, platforms: &[Platform], api: &str, digest: char) -> Release {
        let manifest = Manifest::parse(include_bytes!("../fixtures/hello.json"), None).unwrap();
        Release {
            version: version.into(),
            api: api.into(),
            platforms: platforms.to_vec(),
            source_commit: "a".repeat(40),
            download_url: format!(
                "https://github.com/example/plugins/releases/download/v{version}/hello.cmxaddon"
            ),
            sha256: digest.to_string().repeat(64),
            compressed_bytes: 100,
            published_at: "2026-09-18T00:00:00Z".into(),
            license: "MIT".into(),
            capabilities: Capabilities::from_manifest(&manifest),
        }
    }
    fn listed(releases: Vec<Release>) -> Catalog {
        let mut catalog = empty();
        catalog.plugins.push(Plugin {
            id: "example.hello".into(),
            name: "Hello".into(),
            publisher: "Fixture".into(),
            tier: Tier::Community,
            repository: "https://github.com/example/plugins".into(),
            description: "Synthetic catalog fixture".into(),
            readme: "Fixture".into(),
            releases,
        });
        catalog
    }
    #[test]
    fn identity_versions_and_digests_are_unique() {
        const BOTH: &[Platform] = &[Platform::LinuxX64, Platform::WindowsX64];
        let good = listed(vec![release("1.0.0", BOTH, "^1.0", 'a')]);
        good.validate().unwrap();
        let mut bad = good.clone();
        bad.plugins[0]
            .releases
            .push(release("1.0.0", BOTH, "^1.0", 'b'));
        assert!(bad.validate().is_err(), "duplicate version");
        let mut bad = good.clone();
        bad.plugins.push(bad.plugins[0].clone());
        assert!(bad.validate().is_err(), "duplicate plugin ID");
        let mut bad = good.clone();
        let mut other = bad.plugins[0].clone();
        other.id = "example.other".into();
        bad.plugins.push(other);
        assert!(bad.validate().is_err(), "digest reused by another plugin");
        for version in ["1.1.0-beta.1", "1.1.0+build", "v1.1.0"] {
            let mut bad = good.clone();
            bad.plugins[0]
                .releases
                .push(release(version, BOTH, "^1.0", 'b'));
            assert!(bad.validate().is_err(), "{version}");
        }
        for id in ["con.tools", "example.nul", "Example.hello"] {
            let mut bad = good.clone();
            bad.plugins[0].id = id.into();
            assert!(bad.validate().is_err(), "{id}");
        }
        // Text limits count characters, as the manifest does.
        let mut wide = good.clone();
        wide.plugins[0].name = "\u{00e9}".repeat(80);
        wide.validate().unwrap();
        wide.plugins[0].name.push('\u{00e9}');
        assert!(wide.validate().is_err());
    }
    #[test]
    fn selection_is_exact_and_names_each_refusal() {
        use Platform::{LinuxX64 as Linux, WindowsX64 as Windows};
        let catalog = listed(vec![
            release("1.0.0", &[Linux, Windows], "^1.0", 'a'),
            release("1.1.0", &[Linux, Windows], "^1.0", 'b'),
            release("1.2.0", &[Linux], "^1.0", 'c'),
            release("1.3.0", &[Windows], "^1.0", 'd'),
            release("2.0.0", &[Linux, Windows], "^2.0", 'e'),
        ]);
        let mut catalog = catalog;
        catalog.blocked.push(Blocked {
            plugin_id: None,
            sha256: Some("b".repeat(64)),
            reason: "Revoked build".into(),
            date: "2026-09-18T00:00:00Z".into(),
        });
        catalog.validate().unwrap();
        let version = |target: &str, platform| {
            catalog
                .select(target, platform)
                .map(|(_, r)| r.version.clone())
        };
        let refusal = |target: &str, platform| catalog.select(target, platform).unwrap_err();
        assert_eq!(version("example.hello", Linux).unwrap(), "1.2.0");
        assert_eq!(version("example.hello", Windows).unwrap(), "1.3.0");
        let link = "https://codemux.org/addons/example.hello?version=";
        assert_eq!(version(&format!("{link}1.0.0"), Linux).unwrap(), "1.0.0");
        for (target, code, message) in [
            (
                format!("{link}9.9.9"),
                ErrorCode::InvalidMessage,
                "Version 9.9.9 of example.hello is not listed in the add-on catalog",
            ),
            (
                format!("{link}1.1.0"),
                ErrorCode::PermissionDenied,
                "This release is blocked: Revoked build",
            ),
            (
                format!("{link}1.3.0"),
                ErrorCode::IncompatibleApi,
                "Version 1.3.0 of example.hello is not available for Linux x64",
            ),
            (
                format!("{link}2.0.0"),
                ErrorCode::IncompatibleApi,
                "Version 2.0.0 of example.hello requires add-on API ^2.0; this CodeMux provides 1.0.0",
            ),
            (
                format!("{link}1.3.0-beta.1"),
                ErrorCode::InvalidMessage,
                "The add-on catalog lists only stable release versions",
            ),
            (
                format!("{link}latest"),
                ErrorCode::InvalidMessage,
                "The install link version is not a semantic version",
            ),
            (
                "example.other".into(),
                ErrorCode::InvalidMessage,
                "example.other is not listed in the add-on catalog",
            ),
            (
                "con.tools".into(),
                ErrorCode::InvalidMessage,
                "Use a catalog ID or a codemux.org add-on install link",
            ),
        ] {
            let error = refusal(&target, Linux);
            assert_eq!((error.data.code, error.message.as_str()), (code, message));
        }
        // Without a version, report the newest release that came closest.
        let only = |releases| {
            let mut catalog = listed(releases);
            catalog.blocked = vec![Blocked {
                plugin_id: None,
                sha256: Some("b".repeat(64)),
                reason: "Revoked build".into(),
                date: "2026-09-18T00:00:00Z".into(),
            }];
            catalog.select("example.hello", Linux).unwrap_err()
        };
        for (releases, code, message) in [
            (
                vec![release("1.0.0", &[Linux], "^1.0", 'b')],
                ErrorCode::PermissionDenied,
                "This release is blocked: Revoked build",
            ),
            (
                vec![
                    release("1.0.0", &[Linux], "^1.0", 'b'),
                    release("1.1.0", &[Windows], "^1.0", 'c'),
                ],
                ErrorCode::IncompatibleApi,
                "No release of example.hello is available for Linux x64",
            ),
            (
                vec![
                    release("1.0.0", &[Windows], "^1.0", 'a'),
                    release("2.0.0", &[Linux], "^2.0", 'c'),
                    release("3.0.0", &[Linux], "^3.0", 'b'),
                ],
                ErrorCode::IncompatibleApi,
                "No release of example.hello supports add-on API 1.0.0; version 2.0.0 requires ^2.0",
            ),
        ] {
            let error = only(releases);
            assert_eq!((error.data.code, error.message.as_str()), (code, message));
        }
    }
}
