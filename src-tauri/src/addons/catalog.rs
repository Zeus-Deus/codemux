//! Catalog traffic has no plugin, account, proxy, or credential authority.
use super::download::download;
#[cfg(test)]
use super::download::redirect_allowed;
use super::{
    manager::{Manager, Status},
    package::Package,
    permissions::Source,
    ErrorCode, ProtocolError, Result,
};
use codemux_addon_protocol::{
    catalog::{Capabilities, Catalog, MAX_BYTES},
    manifest::Platform,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
const CATALOG_URL: &str = "https://codemux.org/addons/catalog-v1.json";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub fetched_at: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Browse {
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    pub stale: bool,
}
fn storage_error() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on catalog cache is unavailable",
    )
}
pub fn platform() -> Result<Platform> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok(Platform::LinuxX64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok(Platform::WindowsX64)
    } else {
        Err(ProtocolError::new(
            ErrorCode::IncompatibleApi,
            "Add-ons are unsupported on this platform",
        ))
    }
}
impl Manager {
    pub fn catalog_snapshot(&self) -> Result<Option<Snapshot>> {
        let db = self.registry.lock().unwrap();
        let value = db
            .query_row(
                "SELECT value FROM metadata WHERE key='catalog'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| storage_error())?;
        value
            .map(|value| {
                if value.len() > MAX_BYTES + 256 {
                    return Err(storage_error());
                }
                let snapshot: Snapshot =
                    serde_json::from_str(&value).map_err(|_| storage_error())?;
                snapshot.catalog.validate()?;
                Ok(snapshot)
            })
            .transpose()
    }
    pub fn check_blocklist(&self, id: &str, digest: &str) -> Result<()> {
        if let Some(snapshot) = self.catalog_snapshot()? {
            if let Some(reason) = snapshot.catalog.blocked_reason(id, digest) {
                return Err(ProtocolError::new(
                    ErrorCode::PermissionDenied,
                    format!("This release is blocked: {reason}"),
                ));
            }
        }
        Ok(())
    }
    fn accept_catalog(&self, catalog: Catalog) -> Result<Snapshot> {
        if let Some(previous) = self.catalog_snapshot()? {
            catalog.check_successor(&previous.catalog)?;
        }
        let snapshot = Snapshot {
            catalog,
            fetched_at: chrono::Utc::now().timestamp(),
        };
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('catalog',?1)",
                [serde_json::to_string(&snapshot).unwrap()],
            )
            .map_err(|_| storage_error())?;
        Ok(snapshot)
    }
    pub async fn refresh_catalog(&self) -> Result<Snapshot> {
        let snapshot = {
            // Serialize validation and persistence, including the monotonic revision check.
            let _serial = self.catalog_serial.lock().await;
            let bytes = download(CATALOG_URL, MAX_BYTES, false).await?;
            self.accept_catalog(Catalog::parse(&bytes)?)?
        };
        for mut installation in self.list()? {
            if let Some(reason) = snapshot
                .catalog
                .blocked_reason(&installation.manifest.id, &installation.digest)
            {
                let operation = self.operation(&installation.manifest.id).await;
                let _lock = operation.lock().await;
                installation = self.installation(&installation.manifest.id)?;
                // A concurrent replacement must be evaluated against the current tuple.
                if snapshot
                    .catalog
                    .blocked_reason(&installation.manifest.id, &installation.digest)
                    .is_none()
                {
                    continue;
                }
                installation.desired_enabled = false;
                installation.status = Status::BlockedDisabled;
                installation.failure = Some(format!("Catalog block: {reason}"));
                self.save(&installation)?;
                self.stop(&installation.manifest.id, None).await;
            }
        }
        Ok(snapshot)
    }
    pub async fn browse(&self, refresh: bool) -> Result<Browse> {
        let cached = self.catalog_snapshot()?;
        let recent = cached.as_ref().is_some_and(|s| {
            let age = chrono::Utc::now().timestamp().saturating_sub(s.fetched_at);
            (0..86400).contains(&age)
        });
        if recent && !refresh {
            return Ok(Browse {
                snapshot: cached,
                error: None,
                stale: false,
            });
        }
        match self.refresh_catalog().await {
            Ok(snapshot) => Ok(Browse {
                snapshot: Some(snapshot),
                error: None,
                stale: false,
            }),
            Err(error) => Ok(Browse {
                snapshot: cached,
                error: Some(error.message),
                stale: true,
            }),
        }
    }
}
impl super::lifecycle::Reviews {
    pub async fn prepare_catalog(
        &self,
        manager: &Arc<Manager>,
        target: &str,
    ) -> Result<super::lifecycle::Review> {
        // An install never falls back to a cached trust decision on fetch failure.
        let snapshot = manager.refresh_catalog().await?;
        let (plugin, release) = snapshot.catalog.select(target, platform()?)?;
        let bytes = download(
            &release.download_url,
            release.compressed_bytes as usize,
            true,
        )
        .await?;
        if bytes.len() != release.compressed_bytes as usize {
            return Err(ProtocolError::invalid(
                "Release size does not match the catalog",
            ));
        }
        let package = Package::parse(bytes, Some(&release.sha256))?;
        if package.manifest.id != plugin.id
            || package.manifest.version != release.version
            || package.manifest.api != release.api
            || package.manifest.platforms != release.platforms
            || package.manifest.repository != plugin.repository
            || package.manifest.license != release.license
            || Capabilities::from_manifest(&package.manifest).normalized()
                != release.capabilities.normalized()
        {
            return Err(ProtocolError::invalid(
                "Parsed package metadata differs from the reviewed release",
            ));
        }
        self.prepare(
            manager,
            package,
            Source::Catalog {
                publisher: plugin.publisher.clone(),
                repository: plugin.repository.clone(),
            },
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_redirects_have_no_wildcards_or_credentials() {
        assert!(redirect_allowed(
            &url::Url::parse("https://release-assets.githubusercontent.com/asset?signature=opaque")
                .unwrap()
        ));
        for url in [
            "http://release-assets.githubusercontent.com/asset",
            "https://raw.githubusercontent.com/asset",
            "https://release-assets.githubusercontent.com.evil.test/asset",
            "https://user@objects.githubusercontent.com/asset",
            "https://objects.githubusercontent.com:8443/asset",
            "https://127.0.0.1/asset",
        ] {
            assert!(!redirect_allowed(&url::Url::parse(url).unwrap()), "{url}")
        }
    }
    #[test]
    fn cached_revision_and_blocklist_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let mut catalog=Catalog::parse(br#"{"schemaVersion":1,"revision":7,"generatedAt":"2026-09-18T00:00:00Z","plugins":[],"blocked":[{"pluginId":"test.blocked","sha256":null,"reason":"Revoked","date":"2026-09-18T00:00:00Z"}]}"#).unwrap();
        manager.accept_catalog(catalog.clone()).unwrap();
        assert!(manager.check_blocklist("test.blocked", "x").is_err());
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        catalog.revision = 6;
        assert!(manager.accept_catalog(catalog).is_err());
        assert_eq!(
            manager
                .catalog_snapshot()
                .unwrap()
                .unwrap()
                .catalog
                .revision,
            7
        );
    }
}
