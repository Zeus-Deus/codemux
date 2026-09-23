//! Catalog traffic has no plugin, account, proxy, or credential authority.
use super::download::download;
#[cfg(test)]
use super::download::redirect_allowed;
use super::{
    manager::{Installation, Manager, Status, UiEvent},
    package::Package,
    permissions::Source,
    ErrorCode, ProtocolError, Result,
};
use codemux_addon_protocol::{
    catalog::{Capabilities, Catalog, Plugin, Release, Tier, MAX_BYTES},
    manifest::Platform,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
const CATALOG_URL: &str = "https://codemux.org/addons/catalog-v1.json";
const RECHECK_SECONDS: i64 = 24 * 60 * 60;
const RETRY_SECONDS: i64 = 60 * 60;
const STARTUP_DELAY: Duration = Duration::from_secs(30);
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
/// Reviewed catalog identity of a catalog-source installation or review.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub publisher: String,
    pub repository: String,
    pub tier: Option<Tier>,
    /// The cached catalog still lists this ID from the same publisher and repository.
    pub listed: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub up_to_date: bool,
    pub installed_version: String,
    pub available_version: Option<String>,
    pub review: Option<super::lifecycle::Review>,
}
#[cfg(test)]
pub(super) type Fetcher = Arc<dyn Fn(&str) -> Result<Vec<u8>> + Send + Sync>;
/// Parsed-snapshot cache and background lifetime. The fetcher and clock
/// replacements are compiled only into tests; no command can reach them.
#[derive(Default)]
pub struct CatalogState {
    cache: StdMutex<Option<Option<Arc<Snapshot>>>>,
    pub(super) stop: CancellationToken,
    #[cfg(test)]
    pub(super) fetch: StdMutex<Option<Fetcher>>,
    #[cfg(test)]
    pub(super) now: StdMutex<Option<i64>>,
}
fn storage_error() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on catalog cache is unavailable",
    )
}
fn damaged() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "The cached add-on catalog is damaged. Open Settings → Add-ons → Browse and choose Refresh.",
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
fn version(value: &str) -> semver::Version {
    semver::Version::parse(value).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}
impl Manager {
    fn now(&self) -> i64 {
        #[cfg(test)]
        if let Some(now) = *self.catalog.now.lock().unwrap() {
            return now;
        }
        chrono::Utc::now().timestamp()
    }
    async fn fetch(&self, url: &str, maximum: usize, assets: bool) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fetch) = self.catalog.fetch.lock().unwrap().clone() {
            return fetch(url);
        }
        download(url, maximum, assets).await
    }
    fn load_catalog(&self) -> Result<Option<Arc<Snapshot>>> {
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
                    return Err(damaged());
                }
                let snapshot: Snapshot = serde_json::from_str(&value).map_err(|_| damaged())?;
                snapshot.catalog.validate().map_err(|_| damaged())?;
                Ok(Arc::new(snapshot))
            })
            .transpose()
    }
    /// Parse and validate the stored snapshot once, not on every activation.
    fn cached_catalog(&self) -> Result<Option<Arc<Snapshot>>> {
        let mut cache = self.catalog.cache.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return Ok(cached.clone());
        }
        let loaded = self.load_catalog()?;
        *cache = Some(loaded.clone());
        Ok(loaded)
    }
    pub fn catalog_snapshot(&self) -> Result<Option<Snapshot>> {
        Ok(self.cached_catalog()?.map(|s| (*s).clone()))
    }
    /// The highest accepted revision, kept apart from the full snapshot so a
    /// damaged cache can be replaced without accepting a rollback.
    fn revision_floor(&self) -> Result<u64> {
        let db = self.registry.lock().unwrap();
        let stored = db
            .query_row(
                "SELECT value FROM metadata WHERE key='catalog-revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| storage_error())?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let raw = db
            .query_row(
                "SELECT value FROM metadata WHERE key='catalog'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| storage_error())?
            .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
            .and_then(|v| v["catalog"]["revision"].as_u64())
            .unwrap_or(0);
        Ok(stored.max(raw))
    }
    pub fn check_blocklist(&self, id: &str, digest: &str) -> Result<()> {
        // A damaged cache fails closed until a validated refresh replaces it.
        if let Some(snapshot) = self.cached_catalog()? {
            if let Some(reason) = snapshot.catalog.blocked_reason(id, digest) {
                return Err(ProtocolError::new(
                    ErrorCode::PermissionDenied,
                    format!("This release is blocked: {reason}"),
                ));
            }
        }
        Ok(())
    }
    fn accept_catalog(&self, catalog: Catalog) -> Result<Arc<Snapshot>> {
        match self.cached_catalog() {
            Ok(Some(previous)) => catalog.check_successor(&previous.catalog)?,
            // An absent or damaged snapshot cannot prove ownership history, but
            // its stored revision still refuses a rollback.
            _ => {
                if catalog.revision < self.revision_floor()? {
                    return Err(ProtocolError::invalid("Catalog revision rollback refused"));
                }
            }
        }
        let snapshot = Arc::new(Snapshot {
            catalog,
            fetched_at: self.now(),
        });
        {
            let mut db = self.registry.lock().unwrap();
            let tx = db.transaction().map_err(|_| storage_error())?;
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('catalog',?1)",
                [serde_json::to_string(&*snapshot).unwrap()],
            )
            .map_err(|_| storage_error())?;
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('catalog-revision',?1)",
                [snapshot.catalog.revision.to_string()],
            )
            .map_err(|_| storage_error())?;
            tx.commit().map_err(|_| storage_error())?;
        }
        *self.catalog.cache.lock().unwrap() = Some(Some(snapshot.clone()));
        Ok(snapshot)
    }
    /// Disable and stop installations the accepted catalog now blocks.
    async fn apply_blocks(&self, snapshot: &Snapshot) -> Result<()> {
        for installation in self.list()? {
            if snapshot
                .catalog
                .blocked_reason(&installation.manifest.id, &installation.digest)
                .is_none()
            {
                continue;
            }
            let operation = self.operation(&installation.manifest.id).await;
            let _lock = operation.lock().await;
            let Ok(mut installation) = self.installation(&installation.manifest.id) else {
                continue;
            };
            // A concurrent replacement must be evaluated against the current tuple.
            let Some(reason) = snapshot
                .catalog
                .blocked_reason(&installation.manifest.id, &installation.digest)
            else {
                continue;
            };
            if matches!(installation.status, Status::BlockedDisabled) {
                continue;
            }
            installation.desired_enabled = false;
            installation.status = Status::BlockedDisabled;
            installation.failure = Some(format!("Catalog block: {reason}"));
            self.save(&installation)?;
            self.stop(&installation.manifest.id, None).await;
        }
        Ok(())
    }
    pub async fn refresh_catalog(&self) -> Result<Arc<Snapshot>> {
        let snapshot = {
            // Serialize validation and persistence, including the monotonic revision check.
            let _serial = self.catalog_serial.lock().await;
            let bytes = self.fetch(CATALOG_URL, MAX_BYTES, false).await?;
            self.accept_catalog(Catalog::parse(&bytes)?)?
        };
        self.apply_blocks(&snapshot).await?;
        Ok(snapshot)
    }
    fn fresh(&self, snapshot: &Snapshot) -> bool {
        (0..RECHECK_SECONDS).contains(&self.now().saturating_sub(snapshot.fetched_at))
    }
    pub async fn browse(&self, refresh: bool) -> Result<Browse> {
        // A damaged cache is replaced by the next validated fetch.
        let cached = self.cached_catalog().ok().flatten();
        if !refresh && cached.as_ref().is_some_and(|s| self.fresh(s)) {
            return Ok(Browse {
                snapshot: cached.map(|s| (*s).clone()),
                error: None,
                stale: false,
            });
        }
        match self.refresh_catalog().await {
            Ok(snapshot) => Ok(Browse {
                snapshot: Some((*snapshot).clone()),
                error: None,
                stale: false,
            }),
            Err(error) => Ok(Browse {
                snapshot: cached.map(|s| (*s).clone()),
                error: Some(error.message),
                stale: true,
            }),
        }
    }
    /// One background pass. It never activates code, keeps the cached
    /// snapshot on failure and returns the seconds until the next pass.
    pub(super) async fn recheck_catalog(&self) -> i64 {
        // No catalog traffic for people who have not installed an add-on.
        if !self.list().is_ok_and(|installed| !installed.is_empty()) {
            return RETRY_SECONDS;
        }
        if let Ok(Some(snapshot)) = self.cached_catalog() {
            if self.fresh(&snapshot) {
                return RECHECK_SECONDS - self.now().saturating_sub(snapshot.fetched_at);
            }
        }
        match self.refresh_catalog().await {
            Ok(_) => {
                // Rows refresh their block reasons and update badges.
                let _ = self.events.send(UiEvent::Inventory);
                RECHECK_SECONDS
            }
            Err(_) => RETRY_SECONDS,
        }
    }
    /// Recheck revocations off the startup path. The task holds no strong
    /// reference to the manager and ends with it or on shutdown.
    pub fn start_catalog_recheck(self: &Arc<Self>) {
        if std::env::var_os("CODEMUX_DISABLE_ADDONS").is_some_and(|v| v == "1") {
            return;
        }
        let manager = Arc::downgrade(self);
        let stop = self.catalog.stop.clone();
        tauri::async_runtime::spawn(async move {
            let mut wait = STARTUP_DELAY;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(wait) => {}
                }
                let Some(manager) = manager.upgrade() else {
                    break;
                };
                wait = Duration::from_secs(manager.recheck_catalog().await.max(60) as u64);
            }
        });
    }
    /// Catalog identity and update availability from the cached snapshot only.
    /// This never downloads a package or activates code.
    pub fn catalog_status(&self, installation: &Installation) -> (Option<Listing>, Option<String>) {
        let Source::Catalog {
            publisher,
            repository,
        } = &installation.source
        else {
            return (None, None);
        };
        let snapshot = self.cached_catalog().ok().flatten();
        let plugin = snapshot.as_ref().and_then(|s| {
            s.catalog.plugins.iter().find(|p| {
                p.id == installation.manifest.id
                    && &p.publisher == publisher
                    && &p.repository == repository
            })
        });
        let update = snapshot
            .as_ref()
            .filter(|_| plugin.is_some())
            .zip(platform().ok())
            .and_then(|(s, platform)| s.catalog.select(&installation.manifest.id, platform).ok())
            .filter(|(_, release)| {
                version(&release.version) > version(&installation.manifest.version)
            })
            .map(|(_, release)| release.version.clone());
        (
            Some(Listing {
                publisher: publisher.clone(),
                repository: repository.clone(),
                tier: plugin.map(|p| p.tier.clone()),
                listed: plugin.is_some(),
            }),
            update,
        )
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
        let source = Source::Catalog {
            publisher: plugin.publisher.clone(),
            repository: plugin.repository.clone(),
        };
        if let Some(installed) = manager
            .list()?
            .into_iter()
            .find(|i| i.manifest.id == plugin.id && i.source == source)
        {
            if installed.digest == release.sha256 {
                return Err(ProtocolError::invalid(format!(
                    "{} {} is already installed",
                    installed.manifest.name, installed.manifest.version
                )));
            }
            if version(&release.version) < version(&installed.manifest.version) {
                return Err(ProtocolError::invalid(
                    "Downgrades require the recorded rollback action",
                ));
            }
        }
        self.prepare_release(manager, plugin, release).await
    }
    /// "Check for update" for a catalog installation. An installed release
    /// that is already the newest compatible one reports up to date.
    pub async fn check_update(&self, manager: &Arc<Manager>, id: &str) -> Result<UpdateCheck> {
        let installed = manager.installation(id)?;
        let Source::Catalog {
            publisher,
            repository,
        } = &installed.source
        else {
            return Err(ProtocolError::invalid(
                "Only catalog installations can check for catalog updates",
            ));
        };
        let snapshot = manager.refresh_catalog().await?;
        let (plugin, release) = snapshot.catalog.select(id, platform()?)?;
        if &plugin.publisher != publisher || &plugin.repository != repository {
            return Err(ProtocolError::invalid(
                "The catalog lists this add-on from a different source",
            ));
        }
        if version(&release.version) <= version(&installed.manifest.version) {
            return Ok(UpdateCheck {
                up_to_date: true,
                installed_version: installed.manifest.version,
                available_version: None,
                review: None,
            });
        }
        let review = self.prepare_release(manager, plugin, release).await?;
        Ok(UpdateCheck {
            up_to_date: false,
            installed_version: installed.manifest.version,
            available_version: Some(release.version.clone()),
            review: Some(review),
        })
    }
    async fn prepare_release(
        &self,
        manager: &Arc<Manager>,
        plugin: &Plugin,
        release: &Release,
    ) -> Result<super::lifecycle::Review> {
        let bytes = manager
            .fetch(
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
        self.prepare_listed(
            manager,
            package,
            Source::Catalog {
                publisher: plugin.publisher.clone(),
                repository: plugin.repository.clone(),
            },
            Some(Listing {
                publisher: plugin.publisher.clone(),
                repository: plugin.repository.clone(),
                tier: Some(plugin.tier.clone()),
                listed: true,
            }),
        )
    }
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::addons::lifecycle::Reviews;
    use codemux_addon_protocol::catalog::Blocked;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const REPOSITORY: &str = "https://github.com/example/hello";
    pub(crate) fn catalog(revision: u64, releases: &[&Package]) -> Catalog {
        let manifest = &releases[0].manifest;
        Catalog {
            schema_version: 1,
            revision,
            generated_at: "2026-09-18T00:00:00Z".into(),
            plugins: vec![Plugin {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                publisher: "Fixture publisher".into(),
                tier: Tier::Community,
                repository: REPOSITORY.into(),
                description: "Synthetic catalog fixture".into(),
                readme: "Fixture".into(),
                releases: releases
                    .iter()
                    .map(|package| Release {
                        version: package.manifest.version.clone(),
                        api: package.manifest.api.clone(),
                        platforms: package.manifest.platforms.clone(),
                        source_commit: "a".repeat(40),
                        download_url: format!(
                            "{REPOSITORY}/releases/download/v{}/hello.cmxaddon",
                            package.manifest.version
                        ),
                        sha256: package.digest.clone(),
                        compressed_bytes: package.archive.len() as u64,
                        published_at: "2026-09-18T00:00:00Z".into(),
                        license: package.manifest.license.clone(),
                        capabilities: serde_json::from_value(
                            Capabilities::from_manifest(&package.manifest).normalized(),
                        )
                        .unwrap(),
                    })
                    .collect(),
            }],
            blocked: vec![],
        }
    }
    /// Serve a catalog and its release assets from memory; count requests.
    pub(crate) fn serve(
        manager: &Manager,
        catalog: Option<&Catalog>,
        assets: &[&Package],
    ) -> Arc<AtomicUsize> {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let catalog = catalog.map(|c| serde_json::to_vec(c).unwrap());
        let assets: Vec<(String, Vec<u8>)> = assets
            .iter()
            .map(|p| {
                (
                    format!(
                        "{REPOSITORY}/releases/download/v{}/hello.cmxaddon",
                        p.manifest.version
                    ),
                    p.archive.clone(),
                )
            })
            .collect();
        *manager.catalog.fetch.lock().unwrap() = Some(Arc::new(move |url: &str| {
            counter.fetch_add(1, Ordering::SeqCst);
            let offline = || ProtocolError::new(ErrorCode::NetworkDenied, "Add-on download failed");
            if url == CATALOG_URL {
                return catalog.clone().ok_or_else(offline);
            }
            assets
                .iter()
                .find(|(asset, _)| asset == url)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(offline)
        }));
        calls
    }
    fn set_now(manager: &Manager, now: i64) {
        *manager.catalog.now.lock().unwrap() = Some(now);
    }
    fn package(version: &str) -> Package {
        crate::addons::lifecycle::tests::package_version(version)
    }
    async fn install_from_catalog(manager: &Arc<Manager>, reviews: &Reviews) -> Installation {
        let review = reviews
            .prepare_catalog(manager, "example.hello")
            .await
            .unwrap();
        reviews
            .accept(manager, &review.token, false, false)
            .await
            .unwrap()
    }
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
    #[tokio::test]
    async fn fresh_cache_is_served_offline_and_refresh_or_age_fetch_again() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let v1 = package("1.0.0");
        set_now(&manager, 1_000_000);
        let calls = serve(&manager, Some(&catalog(1, &[&v1])), &[]);
        let first = manager.browse(false).await.unwrap();
        assert!(first.snapshot.is_some() && !first.stale && first.error.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Younger than 24 h: served from cache, even with the network gone.
        serve(&manager, None, &[]);
        set_now(&manager, 1_000_000 + RECHECK_SECONDS - 1);
        let cached = manager.browse(false).await.unwrap();
        assert!(cached.snapshot.is_some() && !cached.stale && cached.error.is_none());
        // Refresh bypasses freshness; offline it keeps the cache marked stale.
        let offline = manager.browse(true).await.unwrap();
        assert!(offline.stale);
        assert_eq!(offline.snapshot.unwrap().catalog.revision, 1);
        assert!(offline.error.is_some());
        // Aged out: an ordinary browse refetches, and a newer revision lands.
        let calls = serve(&manager, Some(&catalog(2, &[&v1])), &[]);
        set_now(&manager, 1_000_000 + RECHECK_SECONDS);
        let refreshed = manager.browse(false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(refreshed.snapshot.unwrap().catalog.revision, 2);
        let forced = manager.browse(true).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!forced.stale);
    }
    #[tokio::test]
    async fn online_install_and_update_need_a_fresh_verified_catalog() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let v1 = package("1.0.0");
        serve(&manager, Some(&catalog(1, &[&v1])), &[&v1]);
        let installed = install_from_catalog(&manager, &reviews).await;
        assert!(matches!(installed.source, Source::Catalog { .. }));
        // A valid cache never substitutes for a failed fetch during an install.
        serve(&manager, None, &[&v1]);
        assert!(reviews
            .prepare_catalog(&manager, "example.hello")
            .await
            .is_err());
        assert!(reviews
            .check_update(&manager, "example.hello")
            .await
            .is_err());
        // Up to date: no review, no download, and the rollback target stays.
        let calls = serve(&manager, Some(&catalog(1, &[&v1])), &[&v1]);
        let check = reviews
            .check_update(&manager, "example.hello")
            .await
            .unwrap();
        assert!(check.up_to_date && check.review.is_none());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "only the catalog was fetched"
        );
        assert!(reviews
            .prepare_catalog(&manager, "example.hello")
            .await
            .err()
            .unwrap()
            .message
            .contains("already installed"));
        // A tampered asset or a size mismatch is refused before review.
        // Accepted releases are immutable, so each case lists a new version.
        let (v11, v12, v2) = (package("1.1.0"), package("1.2.0"), package("2.0.0"));
        let mut tampered = catalog(2, &[&v1, &v11]);
        tampered.plugins[0].releases[1].sha256 = "b".repeat(64);
        serve(&manager, Some(&tampered), &[&v11]);
        let error = reviews
            .check_update(&manager, "example.hello")
            .await
            .err()
            .unwrap();
        assert!(error.message.contains("digest"), "{}", error.message);
        let mut resized = catalog(3, &[&v1, &v11, &v12]);
        resized.plugins[0].releases[1] = tampered.plugins[0].releases[1].clone();
        resized.plugins[0].releases[2].compressed_bytes += 1;
        serve(&manager, Some(&resized), &[&v12]);
        let error = reviews
            .check_update(&manager, "example.hello")
            .await
            .err()
            .unwrap();
        assert!(error.message.contains("size"), "{}", error.message);
        // A same-permission catalog update keeps the installation identity.
        let mut current = catalog(4, &[&v1, &v11, &v12, &v2]);
        current.plugins[0].releases[1] = resized.plugins[0].releases[1].clone();
        current.plugins[0].releases[2] = resized.plugins[0].releases[2].clone();
        serve(&manager, Some(&current), &[&v2]);
        // The badge reads only the cached revision; it never fetches.
        let status = manager.catalog_status(&manager.installation("example.hello").unwrap());
        assert_eq!(status.1.as_deref(), Some("1.2.0"));
        let check = reviews
            .check_update(&manager, "example.hello")
            .await
            .unwrap();
        let status = manager.catalog_status(&manager.installation("example.hello").unwrap());
        assert_eq!(status.1.as_deref(), Some("2.0.0"));
        let listing = status.0.unwrap();
        assert!(listing.listed && matches!(listing.tier, Some(Tier::Community)));
        assert_eq!(listing.publisher, "Fixture publisher");
        assert!(!check.up_to_date);
        assert_eq!(check.available_version.as_deref(), Some("2.0.0"));
        let review = check.review.unwrap();
        assert_eq!(review.installed.as_ref().unwrap().version, "1.0.0");
        assert!(!review.expands_access);
        assert_eq!(
            review.catalog.as_ref().unwrap().publisher,
            "Fixture publisher"
        );
        let updated = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        assert_eq!(updated.installation_id, installed.installation_id);
        assert_ne!(updated.data_generation, installed.data_generation);
        assert_eq!(
            updated.previous.as_ref().unwrap().data_generation,
            installed.data_generation
        );
        assert_eq!(
            manager
                .catalog_status(&manager.installation("example.hello").unwrap())
                .1,
            None
        );
    }
    #[tokio::test]
    async fn refresh_blocks_an_installed_release_with_its_reason() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let v1 = package("1.0.0");
        serve(&manager, Some(&catalog(1, &[&v1])), &[&v1]);
        let mut installed = install_from_catalog(&manager, &reviews).await;
        installed.desired_enabled = true;
        installed.status = Status::EnabledIdle;
        manager.save(&installed).unwrap();
        let mut blocked = catalog(2, &[&v1]);
        blocked.blocked.push(Blocked {
            plugin_id: None,
            sha256: Some(v1.digest.clone()),
            reason: "Revoked for testing".into(),
            date: "2026-09-19T00:00:00Z".into(),
        });
        serve(&manager, Some(&blocked), &[]);
        manager.browse(true).await.unwrap();
        let disabled = manager.installation("example.hello").unwrap();
        assert!(matches!(disabled.status, Status::BlockedDisabled));
        assert!(!disabled.desired_enabled);
        assert_eq!(
            disabled.failure.as_deref(),
            Some("Catalog block: Revoked for testing")
        );
        assert!(manager.ensure_active("example.hello").await.is_err());
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reopened = manager.installation("example.hello").unwrap();
        assert!(matches!(reopened.status, Status::BlockedDisabled));
        assert_eq!(reopened.failure, disabled.failure);
        assert!(manager.ensure_active("example.hello").await.is_err());
    }
    #[tokio::test]
    async fn background_recheck_is_rate_limited_and_needs_an_installation() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let v1 = package("1.0.0");
        set_now(&manager, 5_000_000);
        let calls = serve(&manager, Some(&catalog(1, &[&v1])), &[&v1]);
        assert_eq!(manager.recheck_catalog().await, RETRY_SECONDS);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "nothing installed, no traffic"
        );
        let installed = install_from_catalog(&manager, &reviews).await;
        let calls = serve(&manager, Some(&catalog(1, &[&v1])), &[]);
        set_now(&manager, 5_000_000 + 100);
        assert_eq!(manager.recheck_catalog().await, RECHECK_SECONDS - 100);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "fresh cache, no traffic");
        // Offline after 24 h: the cache stays and the pass retries later.
        serve(&manager, None, &[]);
        set_now(&manager, 5_000_000 + RECHECK_SECONDS);
        assert_eq!(manager.recheck_catalog().await, RETRY_SECONDS);
        assert_eq!(
            manager
                .catalog_snapshot()
                .unwrap()
                .unwrap()
                .catalog
                .revision,
            1
        );
        // Back online: a new revocation disables the installation.
        let mut blocked = catalog(2, &[&v1]);
        blocked.blocked.push(Blocked {
            plugin_id: Some(installed.manifest.id.clone()),
            sha256: None,
            reason: "Withdrawn".into(),
            date: "2026-09-19T00:00:00Z".into(),
        });
        let calls = serve(&manager, Some(&blocked), &[]);
        assert_eq!(manager.recheck_catalog().await, RECHECK_SECONDS);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            manager.installation("example.hello").unwrap().status,
            Status::BlockedDisabled
        ));
    }
    #[tokio::test]
    async fn damaged_catalog_cache_is_replaced_without_accepting_a_rollback() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let v1 = package("1.0.0");
        manager.accept_catalog(catalog(5, &[&v1])).unwrap();
        // Simulate a row written by a different schema: parsing now fails.
        manager
            .registry
            .lock()
            .unwrap()
            .execute(
                "UPDATE metadata SET value=replace(value,'\"fetchedAt\"','\"unknownField\":1,\"fetchedAt\"') WHERE key='catalog'",
                [],
            )
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let error = manager
            .check_blocklist("example.hello", &v1.digest)
            .err()
            .unwrap();
        assert!(error.message.contains("Refresh"), "{}", error.message);
        assert!(manager.accept_catalog(catalog(4, &[&v1])).is_err());
        serve(&manager, Some(&catalog(6, &[&v1])), &[]);
        let browse = manager.browse(false).await.unwrap();
        assert_eq!(browse.snapshot.unwrap().catalog.revision, 6);
        manager
            .check_blocklist("example.hello", &v1.digest)
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert_eq!(
            manager
                .catalog_snapshot()
                .unwrap()
                .unwrap()
                .catalog
                .revision,
            6
        );
        assert!(manager.accept_catalog(catalog(5, &[&v1])).is_err());
    }
}
