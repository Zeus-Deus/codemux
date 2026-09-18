//! Reviewed, inert release metadata shared by the installer and publication tools.
use crate::{
    manifest::{self, Credential, HttpGrant, Permission, Platform},
    Manifest, ProtocolError,
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
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
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
    pub fn select(&self, target: &str, platform: Platform) -> Result<(&Plugin, &Release)> {
        let (id, version) = install_target(target)?;
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProtocolError::invalid("Add-on is not listed in the catalog"))?;
        let release = plugin
            .releases
            .iter()
            .filter(|r| {
                version.as_ref().is_none_or(|v| v == &r.version)
                    && r.platforms.contains(&platform)
                    && semver::VersionReq::parse(&r.api)
                        .is_ok_and(|v| v.matches(&semver::Version::new(1, 0, 0)))
                    && self.blocked_reason(&id, &r.sha256).is_none()
            })
            .max_by_key(|r| semver::Version::parse(&r.version).unwrap())
            .ok_or_else(|| {
                ProtocolError::invalid(
                    "The requested release is unavailable, blocked, or incompatible",
                )
            })?;
        Ok((plugin, release))
    }
}
pub fn install_target(target: &str) -> Result<(String, Option<String>)> {
    if manifest::plugin_id(target) {
        return Ok((target.into(), None));
    }
    let u = url::Url::parse(target).map_err(|_| invalid())?;
    if u.scheme() != "https"
        || u.host_str() != Some("codemux.org")
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return Err(ProtocolError::invalid(
            "Use a catalog ID or a codemux.org add-on install link",
        ));
    }
    let id = u
        .path()
        .strip_prefix("/addons/")
        .filter(|id| manifest::plugin_id(id))
        .ok_or_else(invalid)?;
    let query = u.query_pairs().collect::<Vec<_>>();
    if query.len() > 1
        || query
            .first()
            .is_some_and(|(k, v)| k != "version" || semver::Version::parse(v).is_err())
    {
        return Err(invalid());
    }
    Ok((id.into(), query.first().map(|(_, v)| v.to_string())))
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
}
