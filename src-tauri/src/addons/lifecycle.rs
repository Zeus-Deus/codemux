//! Explicit review plus durable package/data tuple changes. Packages remain inert
//! until a host-owned review is accepted; no author install scripts exist.
use super::{
    catalog::Listing,
    manager::{Installation, Manager, Status},
    package::Package,
    permissions::{capability_digest, Grant, Source},
    storage::Storage,
    ErrorCode, Manifest, ProtocolError, Result,
};
use codemux_addon_protocol::manifest::{HttpMethod, Permission};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Previous {
    pub manifest: super::Manifest,
    pub source: Source,
    pub digest: String,
    pub data_generation: String,
    pub grant: Option<Grant>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub token: String,
    pub manifest: super::Manifest,
    pub digest: String,
    pub source: Source,
    pub replaces_source: bool,
    /// Set only when `added` is non-empty; removing access needs no new grant.
    pub expands_access: bool,
    pub compressed_bytes: usize,
    pub development: bool,
    pub retained_data: Option<RetainedData>,
    pub installed: Option<InstalledRelease>,
    pub added: Access,
    pub removed: Access,
    pub catalog: Option<Listing>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedData {
    pub version: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRelease {
    pub version: String,
    pub digest: String,
    pub source: Source,
    pub desired_enabled: bool,
    pub capabilities: serde_json::Value,
}
/// Access present in one manifest and missing from another: permissions,
/// methods per origin, and credentials by ID and origin.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    pub permissions: Vec<Permission>,
    pub http: Vec<HttpAccess>,
    pub credentials: Vec<CredentialAccess>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpAccess {
    pub origin: String,
    pub methods: Vec<HttpMethod>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialAccess {
    pub id: String,
    pub label: String,
    pub origin: String,
}
impl Access {
    pub fn is_empty(&self) -> bool {
        self.permissions.is_empty() && self.http.is_empty() && self.credentials.is_empty()
    }
    /// Access `next` declares beyond `base`; everything when there is no base.
    pub fn missing(base: Option<&Manifest>, next: &Manifest) -> Self {
        let mut permissions: Vec<Permission> = next
            .permissions
            .iter()
            .filter(|p| base.is_none_or(|b| !b.permissions.contains(p)))
            .copied()
            .collect();
        permissions.sort_by_key(name);
        let mut http: Vec<HttpAccess> = next
            .http
            .iter()
            .filter_map(|grant| {
                let known = base.and_then(|b| b.http.iter().find(|h| h.origin == grant.origin));
                let mut methods: Vec<HttpMethod> = grant
                    .methods
                    .iter()
                    .filter(|m| known.is_none_or(|k| !k.methods.contains(m)))
                    .cloned()
                    .collect();
                methods.sort_by_key(name);
                (!methods.is_empty()).then(|| HttpAccess {
                    origin: grant.origin.clone(),
                    methods,
                })
            })
            .collect();
        http.sort_by(|a, b| a.origin.cmp(&b.origin));
        let mut credentials: Vec<CredentialAccess> = next
            .credentials
            .iter()
            .filter(|c| {
                base.is_none_or(|b| {
                    !b.credentials
                        .iter()
                        .any(|o| o.id == c.id && o.origin == c.origin)
                })
            })
            .map(|c| CredentialAccess {
                id: c.id.clone(),
                label: c.label.clone(),
                origin: c.origin.clone(),
            })
            .collect();
        credentials.sort_by(|a, b| (&a.id, &a.origin).cmp(&(&b.id, &b.origin)));
        Self {
            permissions,
            http,
            credentials,
        }
    }
}
fn name(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap()
}
struct Pending {
    review: Review,
    package: Package,
    previous_digest: Option<String>,
    development_path: Option<std::path::PathBuf>,
    development_cancel: Option<tokio_util::sync::CancellationToken>,
    retained: Option<Installation>,
}
#[derive(Default)]
pub struct Reviews {
    pending: Mutex<HashMap<String, Pending>>,
    #[cfg(test)]
    interruption: Mutex<
        Option<(
            &'static str,
            Arc<tokio::sync::Notify>,
            Option<Arc<tokio::sync::Notify>>,
        )>,
    >,
    #[cfg(test)]
    fault: Mutex<Option<&'static str>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    old: Option<Installation>,
    candidate: Installation,
}
fn unavailable() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "The add-on transaction could not be saved",
    )
}
fn journal_error() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "An interrupted add-on update could not be recovered",
    )
}
/// The exact package/data tuple of a record, independent of its status.
fn same_tuple(a: &Installation, b: &Installation) -> bool {
    a.installation_id == b.installation_id && a.data_generation == b.data_generation
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(unavailable)?;
    std::fs::create_dir_all(parent).map_err(|_| unavailable())?;
    let tmp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|_| unavailable())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| unavailable())?;
    std::fs::rename(&tmp, path).map_err(|_| unavailable())?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| unavailable())?;
    Ok(())
}
impl Reviews {
    #[cfg(test)]
    async fn checkpoint(&self, stage: &'static str) {
        let notify = self
            .interruption
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(expected, _, _)| *expected == stage)
            .map(|(_, notify, resume)| (notify.clone(), resume.clone()));
        if let Some((notify, resume)) = notify {
            notify.notify_one();
            // The test drops this transaction future here, exactly as an
            // interrupted process would abandon it without error-path cleanup.
            if let Some(resume) = resume {
                resume.notified().await;
            } else {
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(test)]
    fn fault(&self, stage: &'static str) -> Result<()> {
        if *self.fault.lock().unwrap() == Some(stage) {
            return Err(ProtocolError::invalid("Injected candidate failure"));
        }
        Ok(())
    }
    pub fn prepare_local(&self, manager: &Manager, path: &Path) -> Result<Review> {
        let package = Package::read(path, None)?;
        self.prepare(
            manager,
            package,
            Source::Local {
                identity: Uuid::new_v4().to_string(),
            },
        )
    }
    pub(super) fn prepare(
        &self,
        manager: &Manager,
        package: Package,
        source: Source,
    ) -> Result<Review> {
        self.prepare_listed(manager, package, source, None)
    }
    pub(super) fn prepare_listed(
        &self,
        manager: &Manager,
        package: Package,
        source: Source,
        catalog: Option<Listing>,
    ) -> Result<Review> {
        manager.check_blocklist(&package.manifest.id, &package.digest)?;
        let old = manager
            .list()?
            .into_iter()
            .find(|i| i.manifest.id == package.manifest.id);
        let replaces_source = old.as_ref().is_some_and(|i| i.source != source);
        let retained = if old.is_none() {
            manager.retained_data(&source, &package.manifest.id, &package.digest)?
        } else {
            None
        };
        let added = Access::missing(old.as_ref().map(|i| &i.manifest), &package.manifest);
        let removed = old
            .as_ref()
            .map(|i| Access::missing(Some(&package.manifest), &i.manifest))
            .unwrap_or_default();
        let review = Review {
            token: Uuid::new_v4().to_string(),
            manifest: package.manifest.clone(),
            digest: package.digest.clone(),
            source,
            replaces_source,
            expands_access: !added.is_empty(),
            compressed_bytes: package.archive.len(),
            development: false,
            retained_data: retained.as_ref().map(|i| RetainedData {
                version: i.manifest.version.clone(),
            }),
            installed: old.as_ref().map(|i| InstalledRelease {
                version: i.manifest.version.clone(),
                digest: i.digest.clone(),
                source: i.source.clone(),
                desired_enabled: i.desired_enabled,
                capabilities: codemux_addon_protocol::catalog::Capabilities::from_manifest(
                    &i.manifest,
                )
                .normalized(),
            }),
            added,
            removed,
            catalog,
        };
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= 4 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Close an existing package review first",
            ));
        }
        pending.insert(
            review.token.clone(),
            Pending {
                review: review.clone(),
                package,
                previous_digest: old.map(|i| i.digest),
                development_path: None,
                development_cancel: None,
                retained,
            },
        );
        Ok(review)
    }
    pub fn prepare_development(&self, manager: &Manager, path: &Path) -> Result<Review> {
        let mut review = self.prepare_local(manager, path)?;
        review.development = true;
        let mut pending = self.pending.lock().unwrap();
        let pending = pending.get_mut(&review.token).unwrap();
        pending.review = review.clone();
        pending.development_path = Some(path.to_owned());
        Ok(review)
    }
    pub fn development_path(&self, token: &str) -> Option<std::path::PathBuf> {
        self.pending
            .lock()
            .unwrap()
            .get(token)
            .and_then(|p| p.development_path.clone())
    }
    pub(super) fn prepare_reload(
        &self,
        manager: &Manager,
        path: &Path,
        id: &str,
        source: Source,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Review> {
        let package = Package::read(path, None)?;
        if package.manifest.id != id || !matches!(source, Source::Local { .. }) {
            return Err(ProtocolError::invalid(
                "A watched package cannot change its installation identity",
            ));
        }
        let mut review = self.prepare(manager, package, source)?;
        review.development = true;
        let mut pending = self.pending.lock().unwrap();
        let pending = pending.get_mut(&review.token).unwrap();
        pending.review = review.clone();
        pending.development_cancel = Some(cancel);
        Ok(review)
    }
    pub fn cancel(&self, token: &str) {
        self.pending.lock().unwrap().remove(token);
    }
    pub async fn accept(
        &self,
        manager: &Arc<Manager>,
        token: &str,
        enable: bool,
        replace_source: bool,
    ) -> Result<Installation> {
        self.accept_with_data(manager, token, enable, replace_source, false)
            .await
    }
    pub async fn accept_with_data(
        &self,
        manager: &Arc<Manager>,
        token: &str,
        enable: bool,
        replace_source: bool,
        restore_data: bool,
    ) -> Result<Installation> {
        let pending =
            self.pending.lock().unwrap().remove(token).ok_or_else(|| {
                ProtocolError::new(ErrorCode::ContextStale, "Package review expired")
            })?;
        let active_review = || -> Result<()> {
            if pending
                .development_cancel
                .as_ref()
                .is_some_and(|c| c.is_cancelled())
            {
                Err(ProtocolError::invalid(
                    "Development watch stopped; review this package again",
                ))
            } else {
                Ok(())
            }
        };
        active_review()?;
        let id = &pending.package.manifest.id;
        let operation = manager.operation(id).await;
        let _lock = operation.lock().await;
        let old = manager.list()?.into_iter().find(|i| &i.manifest.id == id);
        manager.check_blocklist(id, &pending.package.digest)?;
        if old.as_ref().is_some_and(|i| {
            semver::Version::parse(&i.manifest.version).unwrap()
                > semver::Version::parse(&pending.package.manifest.version).unwrap()
        }) {
            return Err(ProtocolError::invalid(
                "Downgrades require the recorded rollback action",
            ));
        }
        if old.as_ref().map(|i| &i.digest) != pending.previous_digest.as_ref() {
            return Err(ProtocolError::new(
                ErrorCode::ContextStale,
                "Installed version changed; review the package again",
            ));
        }
        if pending.review.replaces_source && !replace_source {
            return Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                "Replacing this package source needs explicit review",
            ));
        }
        if !pending.review.development
            && old.as_ref().is_some_and(|i| {
                i.manifest.version == pending.package.manifest.version
                    && i.digest != pending.package.digest
            })
        {
            return Err(ProtocolError::invalid(
                "A version cannot be reused with different package bytes",
            ));
        }
        let same_source = old
            .as_ref()
            .is_some_and(|i| i.source == pending.review.source);
        // Re-accepting the installed tuple would replace the real rollback
        // snapshot with a copy of the current release.
        if !pending.review.development
            && same_source
            && old
                .as_ref()
                .is_some_and(|i| i.digest == pending.package.digest)
        {
            return Err(ProtocolError::invalid("This release is already installed"));
        }
        let retained =
            if restore_data {
                Some(pending.retained.as_ref().ok_or_else(|| {
                    ProtocolError::invalid("No matching retained data is available")
                })?)
            } else {
                None
            };
        if let Some(retained) = retained {
            let current =
                manager.retained_data(&pending.review.source, id, &pending.package.digest)?;
            if current.as_ref().map(|i| &i.installation_id) != Some(&retained.installation_id) {
                return Err(ProtocolError::invalid(
                    "Retained data changed; review again",
                ));
            }
        }
        let source = retained
            .map(|i| i.source.clone())
            .unwrap_or_else(|| pending.review.source.clone());
        let installation_id = if same_source {
            old.as_ref().unwrap().installation_id.clone()
        } else {
            Uuid::new_v4().to_string()
        };
        // An update keeps the current enablement unless the user explicitly
        // enables it; an installation or source replacement uses only the choice.
        let desired = enable || (same_source && old.as_ref().is_some_and(|i| i.desired_enabled));
        let mut candidate = Installation {
            installation_id: installation_id.clone(),
            manifest: pending.package.manifest.clone(),
            source: source.clone(),
            digest: pending.package.digest.clone(),
            desired_enabled: desired,
            status: if desired {
                Status::EnabledIdle
            } else {
                Status::InstalledDisabled
            },
            data_generation: Uuid::new_v4().to_string(),
            grant: Some(Grant {
                installation_id,
                plugin_id: id.clone(),
                source,
                digest: pending.package.digest.clone(),
                capabilities: capability_digest(&pending.package.manifest),
            }),
            failure: None,
            previous: if same_source {
                old.as_ref().map(|old| Previous {
                    manifest: old.manifest.clone(),
                    source: old.source.clone(),
                    digest: old.digest.clone(),
                    data_generation: old.data_generation.clone(),
                    grant: old.grant.clone(),
                })
            } else {
                None
            },
        };
        let directory = manager
            .root
            .join("packages")
            .join(id)
            .join(&candidate.digest);
        if directory.exists() {
            Package::read(&directory.join("package.cmxaddon"), Some(&candidate.digest))?;
        }
        #[cfg(test)]
        self.checkpoint("package-write").await;
        if !directory.exists() {
            let staging = manager
                .root
                .join("staging")
                .join(Uuid::new_v4().to_string());
            std::fs::create_dir_all(&staging).map_err(|_| unavailable())?;
            // Production ignores source maps; package.cmxaddon keeps the reviewed
            // bytes for digest checks without extracting one beside the package.
            for (name, bytes) in pending
                .package
                .files
                .iter()
                .filter(|(name, _)| *name != "source.map")
            {
                write_atomic(&staging.join(name), bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        staging.join(name),
                        std::fs::Permissions::from_mode(0o400),
                    )
                    .map_err(|_| unavailable())?;
                }
            }
            write_atomic(&staging.join("package.cmxaddon"), &pending.package.archive)?;
            std::fs::create_dir_all(directory.parent().unwrap()).map_err(|_| unavailable())?;
            std::fs::rename(&staging, &directory).map_err(|_| unavailable())?;
        }
        #[cfg(test)]
        self.checkpoint("package-staged").await;
        let journal = manager
            .root
            .join("recovery")
            .join(Uuid::new_v4().to_string())
            .join("journal.json");
        write_atomic(
            &journal,
            &serde_json::to_vec(&Journal {
                old: old.clone(),
                candidate: candidate.clone(),
            })
            .unwrap(),
        )?;
        #[cfg(test)]
        self.checkpoint("journal-saved").await;
        // While paused the choice is saved; the first activation after Resume
        // starts the new release normally instead of probing it now.
        let live = || desired && !manager.paused();
        let mut queued = false;
        let result: Result<()> = async {
            active_review()?;
            manager.stop(id, None).await;
            if let Some(old) = &old {
                let mut updating = old.clone();
                updating.status = Status::Updating;
                manager.save(&updating)?;
            }
            let data = manager
                .root
                .join("state")
                .join(&candidate.installation_id)
                .join(&candidate.data_generation);
            std::fs::create_dir_all(&data).map_err(|_| unavailable())?;
            if let Some(old) = if same_source { old.as_ref() } else { retained } {
                let previous = manager
                    .root
                    .join("state")
                    .join(&old.installation_id)
                    .join(&old.data_generation)
                    .join("state.sqlite");
                if previous.exists() {
                    Storage::open(&previous)?.snapshot(&data.join("state.sqlite"))?;
                }
            }
            // The probe and the first activation read settings from the registry,
            // so restored values must be there before either starts. The orphan
            // keeps its own row until the completion transaction retires it.
            if let Some(retained) = retained {
                super::cleanup::restore_settings(
                    &manager.registry.lock().unwrap(),
                    retained,
                    &candidate,
                )?;
            }
            #[cfg(test)]
            self.checkpoint("data-snapshotted").await;
            if live() {
                manager
                    .activate(candidate.clone(), pending.package.source(), true)
                    .await?;
                manager.stop(id, None).await;
            }
            #[cfg(test)]
            self.checkpoint("candidate-probed").await;
            active_review()?;
            manager.commit_replacement(&candidate)?;
            #[cfg(test)]
            self.checkpoint("registry-switched").await;
            #[cfg(test)]
            self.fault("registry-switched")?;
            if live() {
                manager
                    .activate(candidate.clone(), pending.package.source(), false)
                    .await?;
                candidate.status = Status::EnabledRunning;
            }
            #[cfg(test)]
            self.checkpoint("activated").await;
            active_review()?;
            // Completion and retirement of a replaced source share a durable
            // commit marker; recovery must never delete restored credentials.
            let mut db = manager.registry.lock().unwrap();
            let tx = db.transaction().map_err(|_| unavailable())?;
            if let Some(old) = &old {
                if old.installation_id != candidate.installation_id {
                    super::cleanup::queue(&tx, old, false)?;
                    queued = true;
                }
            }
            if let Some(retained) = retained {
                tx.execute(
                    "DELETE FROM metadata WHERE key=?1",
                    [format!("orphan:{}", retained.installation_id)],
                )
                .map_err(|_| unavailable())?;
                super::cleanup::queue(&tx, retained, false)?;
                queued = true;
            }
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,'complete')",
                [format!("transaction:{}", candidate.data_generation)],
            )
            .map_err(|_| unavailable())?;
            tx.commit().map_err(|_| unavailable())?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            manager.stop(id, None).await;
            manager.restore(old.as_ref(), &candidate, journal.parent().unwrap());
            return Err(error);
        }
        #[cfg(test)]
        self.checkpoint("completion-committed").await;
        // The update is committed. If the journal cannot be removed now, the
        // completion marker lets startup recovery discard it instead.
        let removed = std::fs::remove_dir_all(journal.parent().unwrap()).is_ok();
        #[cfg(test)]
        self.checkpoint("journal-removed").await;
        if removed {
            let _ = manager.registry.lock().unwrap().execute(
                "DELETE FROM metadata WHERE key=?1",
                [format!("transaction:{}", candidate.data_generation)],
            );
        }
        // Delete the replaced installation's credentials and files now rather
        // than leaving a cleanup warning until a manual retry. Cleanup takes
        // the per-plugin operation lock itself.
        drop(_lock);
        if queued {
            let _ = manager.retry_cleanup().await;
        }
        Ok(candidate)
    }
}
impl Manager {
    fn retained_data(
        &self,
        source: &Source,
        id: &str,
        digest: &str,
    ) -> Result<Option<Installation>> {
        let db = self.registry.lock().unwrap();
        let mut query = db
            .prepare("SELECT value FROM metadata WHERE key LIKE 'orphan:%' ORDER BY rowid DESC LIMIT 1000")
            .map_err(|_| unavailable())?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| unavailable())?;
        for row in rows {
            let row = row.map_err(|_| unavailable())?;
            if row.len() > 128 * 1024 {
                return Err(unavailable());
            }
            let retained: Installation = serde_json::from_str(&row).map_err(|_| unavailable())?;
            retained.validate_record()?;
            let matches = retained.source == *source
                || (matches!(source, Source::Local { .. })
                    && matches!(retained.source, Source::Local { .. })
                    && retained.digest == digest);
            if retained.manifest.id == id && matches {
                return Ok(Some(retained));
            }
        }
        Ok(None)
    }
    /// Make `record` the plugin's only registry row. Deleting by plugin ID works
    /// before and after a source-replacement switch, whose rows differ by ID.
    fn commit_replacement(&self, record: &Installation) -> Result<()> {
        record.validate_record()?;
        let mut db = self.registry.lock().unwrap();
        let tx = db.transaction().map_err(|_| unavailable())?;
        tx.execute(
            "DELETE FROM installations WHERE plugin_id=?1",
            [&record.manifest.id],
        )
        .map_err(|_| unavailable())?;
        tx.execute(
            "INSERT INTO installations(id,plugin_id,record) VALUES(?1,?2,?3)",
            rusqlite::params![
                record.installation_id,
                record.manifest.id,
                serde_json::to_string(record).unwrap()
            ],
        )
        .map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        let _ = self.events.send(super::manager::UiEvent::Inventory);
        Ok(())
    }
    /// Failure path of an update or rollback: put back the previous tuple and
    /// its enablement, or nothing for a new installation, and drop settings
    /// restored for a new installation ID. The journal is removed only once
    /// that is durable; otherwise startup recovery retries.
    fn restore(&self, old: Option<&Installation>, candidate: &Installation, journal: &Path) {
        let restored = match old {
            Some(old) => {
                let mut restored = old.clone();
                restored.status = if restored.desired_enabled {
                    Status::EnabledIdle
                } else {
                    Status::InstalledDisabled
                };
                self.commit_replacement(&restored)
            }
            None => self
                .registry
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM installations WHERE id=?1",
                    [&candidate.installation_id],
                )
                .map(|_| ())
                .map_err(|_| unavailable()),
        }
        .and_then(|()| {
            super::cleanup::discard_candidate_settings(
                &self.registry.lock().unwrap(),
                &candidate.installation_id,
            )
        });
        if restored.is_ok() {
            let _ = std::fs::remove_dir_all(journal);
        }
    }
    pub fn recover(&self) -> Result<()> {
        let root = self.root.join("recovery");
        if !root.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(root)
            .map_err(|_| journal_error())?
            .take(1000)
        {
            let entry = entry.map_err(|_| journal_error())?;
            let path = entry.path().join("journal.json");
            if entry.file_type().map_err(|_| journal_error())?.is_dir() && !path.exists() {
                // Interrupted atomic journal write: no registry switch could
                // have happened before this file was durably renamed.
                std::fs::remove_dir_all(entry.path()).map_err(|_| journal_error())?;
                continue;
            }
            if !entry.file_type().map_err(|_| journal_error())?.is_dir()
                || path
                    .symlink_metadata()
                    .map_err(|_| journal_error())?
                    .file_type()
                    .is_symlink()
            {
                return Err(journal_error());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|_| journal_error())?
                .take(512 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| journal_error())?;
            if bytes.len() > 512 * 1024 {
                return Err(journal_error());
            }
            let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| journal_error())?;
            journal.candidate.validate_record()?;
            if let Some(old) = &journal.old {
                old.validate_record()?;
            }
            let current = self
                .list()?
                .into_iter()
                .find(|i| i.manifest.id == journal.candidate.manifest.id);
            let complete = self
                .registry
                .lock()
                .unwrap()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM metadata WHERE key=?1 AND value='complete')",
                    [format!("transaction:{}", journal.candidate.data_generation)],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|_| unavailable())?;
            if complete {
                std::fs::remove_dir_all(entry.path()).map_err(|_| journal_error())?;
                self.registry
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM metadata WHERE key=?1",
                        [format!("transaction:{}", journal.candidate.data_generation)],
                    )
                    .map_err(|_| unavailable())?;
                continue;
            }
            // A journal remains until normal activation has completed. A crash
            // at any earlier point restores the previous package/data/grant tuple.
            // Only the exact candidate tuple is rolled back: a missing record or
            // a later tuple means a newer removal or update already decided it,
            // and this journal is stale (for example, its removal failed).
            if current
                .as_ref()
                .is_some_and(|i| same_tuple(i, &journal.candidate))
            {
                if let Some(old) = &journal.old {
                    self.commit_replacement(old)?;
                } else {
                    self.registry
                        .lock()
                        .unwrap()
                        .execute(
                            "DELETE FROM installations WHERE id=?1",
                            [&journal.candidate.installation_id],
                        )
                        .map_err(|_| unavailable())?;
                }
            }
            super::cleanup::discard_candidate_settings(
                &self.registry.lock().unwrap(),
                &journal.candidate.installation_id,
            )?;
            std::fs::remove_dir_all(entry.path()).map_err(|_| journal_error())?;
        }
        Ok(())
    }
    /// Commit the removal tombstone: queue cleanup, keep an orphan record for
    /// retained data, and delete the installation, all in one transaction.
    fn commit_removal(&self, installation: &Installation, keep_data: bool) -> Result<()> {
        let mut db = self.registry.lock().unwrap();
        let tx = db.transaction().map_err(|_| unavailable())?;
        super::cleanup::queue(&tx, installation, keep_data)?;
        if keep_data {
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,?2)",
                rusqlite::params![
                    format!("orphan:{}", installation.installation_id),
                    serde_json::to_string(installation).unwrap()
                ],
            )
            .map_err(|_| unavailable())?;
        }
        tx.execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!("removing:{}", installation.installation_id)],
        )
        .map_err(|_| unavailable())?;
        tx.execute(
            "DELETE FROM installations WHERE id=?1",
            [&installation.installation_id],
        )
        .map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())
    }
    /// Startup: finish removals interrupted before their tombstone commit,
    /// using the data choice saved with the removing status.
    pub(super) fn finish_removals(&self) -> Result<()> {
        for installation in self.list()? {
            if !matches!(installation.status, Status::Removing) {
                continue;
            }
            let keep = self
                .registry
                .lock()
                .unwrap()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM metadata WHERE key=?1 AND value='keep')",
                    [format!("removing:{}", installation.installation_id)],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|_| unavailable())?;
            self.commit_removal(&installation, keep)?;
        }
        Ok(())
    }
    pub async fn remove(&self, id: &str, keep_data: bool) -> Result<Vec<String>> {
        let operation = self.operation(id).await;
        let _lock = operation.lock().await;
        let mut installation = self.installation(id)?;
        installation.desired_enabled = false;
        installation.status = Status::Removing;
        // Save the data choice before the removing status, so an interrupted
        // removal finishes the same way on the next launch.
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,?2)",
                [
                    format!("removing:{}", installation.installation_id),
                    if keep_data { "keep" } else { "delete" }.into(),
                ],
            )
            .map_err(|_| unavailable())?;
        self.save(&installation)?;
        self.stop(id, None).await;
        self.commit_removal(&installation, keep_data)?;
        let _ = self.events.send(super::manager::UiEvent::Inventory);
        // The retry path takes the same operation lock; release this completed
        // removal before attempting any OS or filesystem cleanup.
        drop(_lock);
        self.retry_cleanup().await
    }

    pub async fn rollback(self: &Arc<Self>, id: &str) -> Result<()> {
        let operation = self.operation(id).await;
        let _lock = operation.lock().await;
        let old = self.installation(id)?;
        let previous = old
            .previous
            .clone()
            .ok_or_else(|| ProtocolError::invalid("No rollback snapshot is available"))?;
        self.check_blocklist(id, &previous.digest)?;
        let path = self
            .root
            .join("packages")
            .join(id)
            .join(&previous.digest)
            .join("package.cmxaddon");
        let package = Package::read(&path, Some(&previous.digest))?;
        let mut candidate = old.clone();
        candidate.manifest = previous.manifest;
        candidate.source = previous.source;
        candidate.digest = previous.digest;
        // Never let a probe or a failed normal activation mutate the recorded
        // snapshot. Rollback gets a new writable generation copied from it.
        candidate.data_generation = Uuid::new_v4().to_string();
        candidate.grant = previous.grant;
        candidate.previous = None;
        candidate.status = if candidate.desired_enabled {
            Status::EnabledIdle
        } else {
            Status::InstalledDisabled
        };
        candidate.failure = None;
        let journal = self
            .root
            .join("recovery")
            .join(Uuid::new_v4().to_string())
            .join("journal.json");
        write_atomic(
            &journal,
            &serde_json::to_vec(&Journal {
                old: Some(old.clone()),
                candidate: candidate.clone(),
            })
            .unwrap(),
        )?;
        let live = || candidate.desired_enabled && !self.paused();
        let result: Result<()> = async {
            self.stop(id, None).await;
            let mut updating = old.clone();
            updating.status = Status::Updating;
            self.save(&updating)?;
            let data = self
                .root
                .join("state")
                .join(&candidate.installation_id)
                .join(&candidate.data_generation);
            std::fs::create_dir_all(&data).map_err(|_| unavailable())?;
            let snapshot = self
                .root
                .join("state")
                .join(&old.installation_id)
                .join(&previous.data_generation)
                .join("state.sqlite");
            if snapshot.exists() {
                Storage::open(&snapshot)?.snapshot(&data.join("state.sqlite"))?;
            }
            if live() {
                self.activate(candidate.clone(), package.source(), true)
                    .await?;
                self.stop(id, None).await;
            }
            self.commit_replacement(&candidate)?;
            if live() {
                self.activate(candidate.clone(), package.source(), false)
                    .await?;
            }
            self.registry
                .lock()
                .unwrap()
                .execute(
                    "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,'complete')",
                    [format!("transaction:{}", candidate.data_generation)],
                )
                .map_err(|_| unavailable())?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.stop(id, None).await;
            self.restore(Some(&old), &candidate, journal.parent().unwrap());
            return Err(error);
        }
        if std::fs::remove_dir_all(journal.parent().unwrap()).is_ok() {
            let _ = self.registry.lock().unwrap().execute(
                "DELETE FROM metadata WHERE key=?1",
                [format!("transaction:{}", candidate.data_generation)],
            );
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn installation() -> Installation {
        let manifest = super::super::Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        Installation {
            installation_id: Uuid::new_v4().to_string(),
            manifest,
            source: Source::Local {
                identity: Uuid::new_v4().to_string(),
            },
            digest: "a".repeat(64),
            desired_enabled: true,
            status: Status::EnabledIdle,
            data_generation: Uuid::new_v4().to_string(),
            grant: None,
            failure: None,
            previous: None,
        }
    }
    #[tokio::test]
    async fn sqlite_full_during_update_preserves_the_previous_release_and_state() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let package = Package::parse(super::super::package::fixture_archive(), None).unwrap();
        let first = reviews
            .prepare(
                &manager,
                package,
                Source::Local {
                    identity: Uuid::new_v4().to_string(),
                },
            )
            .unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let state = manager
            .root
            .join("state")
            .join(&old.installation_id)
            .join(&old.data_generation)
            .join("state.sqlite");
        Storage::open(&state)
            .unwrap()
            .set("global", "value", &serde_json::json!("original"))
            .unwrap();
        let package = Package::parse(super::super::package::fixture_archive(), None).unwrap();
        let mut files = package.files;
        let mut manifest = package.manifest;
        manifest.version = "2.0.0".into();
        manifest
            .settings
            .push(codemux_addon_protocol::manifest::Setting::String {
                id: "large-default".into(),
                label: "Fixture".into(),
                default: "x".repeat(4096),
            });
        files.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        let candidate = Package::parse(tar.into_inner().unwrap().finish().unwrap(), None).unwrap();
        let review = reviews
            .prepare(&manager, candidate, old.source.clone())
            .unwrap();
        {
            let db = manager.registry.lock().unwrap();
            let pages: u64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
            // SQLite now returns the real SQLITE_FULL error when this larger
            // record requires another page. No production fault bypass exists.
            db.pragma_update(None, "max_page_count", pages).unwrap();
        }
        let error = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .err()
            .expect("update should hit SQLITE_FULL");
        assert_eq!(error.data.code, ErrorCode::StorageUnavailable);
        let restored = manager.installation(&old.manifest.id).unwrap();
        assert_eq!(restored.digest, old.digest);
        assert_eq!(restored.data_generation, old.data_generation);
        assert_eq!(
            Storage::open(&state)
                .unwrap()
                .get("global", "value")
                .unwrap(),
            serde_json::json!("original")
        );
        drop(manager);
        let reopened = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        assert_eq!(
            reopened.installation(&old.manifest.id).unwrap().digest,
            old.digest
        );
    }
    #[tokio::test]
    async fn rollback_copies_matching_data_and_preserves_snapshot_on_activation_failure() {
        let root = tempfile::tempdir().unwrap();
        let manager =
            Manager::open(root.path().into(), "missing-host-for-failure-test".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, super::super::package::fixture_archive()).unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let first = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let state = |generation: &str| {
            root.path()
                .join("state")
                .join(&first.installation_id)
                .join(generation)
                .join("state.sqlite")
        };
        Storage::open(&state(&first.data_generation))
            .unwrap()
            .set("global", "fixture", &serde_json::json!("before update"))
            .unwrap();
        let mut current = first.clone();
        current.previous = Some(Previous {
            manifest: first.manifest.clone(),
            source: first.source.clone(),
            digest: first.digest.clone(),
            data_generation: first.data_generation.clone(),
            grant: first.grant.clone(),
        });
        current.data_generation = Uuid::new_v4().to_string();
        std::fs::create_dir_all(state(&current.data_generation).parent().unwrap()).unwrap();
        Storage::open(&state(&current.data_generation))
            .unwrap()
            .set("global", "fixture", &serde_json::json!("after update"))
            .unwrap();
        current.desired_enabled = true;
        current.status = Status::EnabledIdle;
        manager.save(&current).unwrap();
        assert!(manager.rollback(&first.manifest.id).await.is_err());
        let failed = manager.installation(&first.manifest.id).unwrap();
        assert_eq!(failed.data_generation, current.data_generation);
        assert!(failed.previous.is_some());
        assert_eq!(
            Storage::open(&state(&first.data_generation))
                .unwrap()
                .get("global", "fixture")
                .unwrap(),
            serde_json::json!("before update")
        );
        current.desired_enabled = false;
        current.status = Status::InstalledDisabled;
        manager.save(&current).unwrap();
        manager.rollback(&first.manifest.id).await.unwrap();
        let rolled = manager.installation(&first.manifest.id).unwrap();
        assert_ne!(rolled.data_generation, first.data_generation);
        assert_ne!(rolled.data_generation, current.data_generation);
        assert!(rolled.previous.is_none());
        assert_eq!(
            Storage::open(&state(&rolled.data_generation))
                .unwrap()
                .get("global", "fixture")
                .unwrap(),
            serde_json::json!("before update")
        );
        drop(manager);
        let reopened = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert_eq!(
            reopened
                .installation(&first.manifest.id)
                .unwrap()
                .data_generation,
            rolled.data_generation
        );
    }
    #[tokio::test]
    async fn retained_data_requires_matching_source_and_explicit_confirmation_without_reusing_credentials(
    ) {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, with_setting().archive).unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        manager
            .set_settings(
                &old.manifest.id,
                serde_json::json!({"include-files": false}),
            )
            .await
            .unwrap();
        let old_db = root
            .path()
            .join("state")
            .join(&old.installation_id)
            .join(&old.data_generation)
            .join("state.sqlite");
        Storage::open(&old_db)
            .unwrap()
            .set("global", "retained", &serde_json::json!({"private":true}))
            .unwrap();
        manager.remove(&old.manifest.id, true).await.unwrap();
        assert!(old_db.exists());
        let foreign = reviews
            .prepare(
                &manager,
                Package::read(&file, None).unwrap(),
                Source::Catalog {
                    publisher: "different".into(),
                    repository: old.manifest.repository.clone(),
                },
            )
            .unwrap();
        assert!(foreign.retained_data.is_none());
        assert!(reviews
            .accept_with_data(&manager, &foreign.token, false, false, true)
            .await
            .is_err());
        let review = reviews.prepare_local(&manager, &file).unwrap();
        assert!(review.retained_data.is_some());
        let restored = reviews
            .accept_with_data(&manager, &review.token, false, false, true)
            .await
            .unwrap();
        assert_ne!(restored.installation_id, old.installation_id);
        assert_eq!(restored.source, old.source);
        let db = root
            .path()
            .join("state")
            .join(&restored.installation_id)
            .join(&restored.data_generation)
            .join("state.sqlite");
        assert_eq!(
            Storage::open(&db)
                .unwrap()
                .get("global", "retained")
                .unwrap(),
            serde_json::json!({"private":true})
        );
        assert_eq!(
            manager.settings(&restored).unwrap(),
            serde_json::json!({"include-files": false})
        );
        // The retired orphan was cleaned up by the accept itself.
        assert!(manager.cleanup_warnings().unwrap().is_empty());
        assert!(!old_db.exists());
        assert_eq!(settings_rows(&manager, &old.installation_id), 0);
        assert!(manager.retry_cleanup().await.unwrap().is_empty());
        assert!(!old_db.exists());
        assert!(db.exists());
        // Removing without keeping data also removes the settings.
        manager.remove(&restored.manifest.id, false).await.unwrap();
        assert_eq!(settings_rows(&manager, &restored.installation_id), 0);
    }
    fn with_setting() -> Package {
        package_with(|manifest| {
            manifest
                .settings
                .push(codemux_addon_protocol::manifest::Setting::Boolean {
                    id: "include-files".into(),
                    label: "Include changed filenames".into(),
                    default: true,
                })
        })
    }
    fn settings_rows(manager: &Manager, installation: &str) -> i64 {
        manager
            .registry
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE installation=?1",
                [installation],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn recovery_entries(root: &Path) -> usize {
        std::fs::read_dir(root.join("recovery"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
    fn journal_candidate(root: &Path) -> Installation {
        let entry = std::fs::read_dir(root.join("recovery"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let journal: Journal =
            serde_json::from_slice(&std::fs::read(entry.path().join("journal.json")).unwrap())
                .unwrap();
        journal.candidate
    }
    #[tokio::test]
    async fn restored_settings_precede_the_candidate_and_leave_with_a_failed_restore() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        let manager = Manager::open(private.clone(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, with_setting().archive).unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let chosen = serde_json::json!({"include-files": false});
        manager
            .set_settings(&old.manifest.id, chosen.clone())
            .await
            .unwrap();
        manager.remove(&old.manifest.id, true).await.unwrap();
        // The probe and the first activation read settings while the candidate
        // is not yet installed, so the restored values must already be there.
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *reviews.interruption.lock().unwrap() =
            Some(("data-snapshotted", reached.clone(), Some(resume.clone())));
        let candidate = {
            let restore = reviews.accept_with_data(&manager, &review.token, false, false, true);
            tokio::pin!(restore);
            tokio::select! {
                result = &mut restore => panic!("restore escaped checkpoint: {}", result.is_ok()),
                _ = reached.notified() => {},
            }
            let candidate = journal_candidate(&private);
            assert_ne!(candidate.installation_id, old.installation_id);
            assert_eq!(manager.settings(&candidate).unwrap(), chosen);
            // A failed restore drops the candidate's copy; the orphan keeps its own.
            *reviews.fault.lock().unwrap() = Some("registry-switched");
            resume.notify_one();
            assert!(restore.await.is_err());
            candidate
        };
        *reviews.fault.lock().unwrap() = None;
        assert_eq!(settings_rows(&manager, &candidate.installation_id), 0);
        assert_eq!(settings_rows(&manager, &old.installation_id), 1);
        // An interruption at the same point is undone by startup recovery.
        let review = reviews.prepare_local(&manager, &file).unwrap();
        assert!(review.retained_data.is_some());
        *reviews.interruption.lock().unwrap() = Some(("candidate-probed", reached.clone(), None));
        tokio::select! {
            result = reviews.accept_with_data(&manager, &review.token, false, false, true) => panic!("restore completed before interruption: {}", result.is_ok()),
            _ = reached.notified() => {},
        }
        let candidate = journal_candidate(&private);
        assert_eq!(settings_rows(&manager, &candidate.installation_id), 1);
        drop(manager);
        let manager = Manager::open(private, "unused".into()).unwrap();
        assert!(manager.list().unwrap().is_empty());
        assert_eq!(settings_rows(&manager, &candidate.installation_id), 0);
        assert_eq!(settings_rows(&manager, &old.installation_id), 1);
        let review = Reviews::default().prepare_local(&manager, &file).unwrap();
        assert!(
            review.retained_data.is_some(),
            "retained data is still offered"
        );
    }
    #[tokio::test]
    async fn failed_source_replacement_restores_the_installed_tuple_without_a_stale_journal() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        let manager = Manager::open(private.clone(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, super::super::package::fixture_archive()).unwrap();
        let first = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let unchanged = |manager: &Manager| {
            let current = manager.installation(&old.manifest.id).unwrap();
            assert_eq!(current.installation_id, old.installation_id);
            assert_eq!(current.data_generation, old.data_generation);
            assert_eq!(current.digest, old.digest);
            assert_eq!(current.source, old.source);
            assert!(!current.desired_enabled);
            assert!(matches!(current.status, Status::InstalledDisabled));
            assert_eq!(manager.list().unwrap().len(), 1);
        };
        // Before the switch: the candidate probe cannot start its host.
        let second = reviews.prepare_local(&manager, &file).unwrap();
        assert!(second.replaces_source);
        let error = reviews
            .accept(&manager, &second.token, true, true)
            .await
            .err()
            .unwrap();
        assert_ne!(
            error.message,
            unavailable().message,
            "candidate failure is reported"
        );
        unchanged(&manager);
        assert_eq!(recovery_entries(&private), 0);
        // After the switch: the candidate row already replaced the old one.
        let third = reviews.prepare_local(&manager, &file).unwrap();
        *reviews.fault.lock().unwrap() = Some("registry-switched");
        let error = reviews
            .accept(&manager, &third.token, false, true)
            .await
            .err()
            .unwrap();
        assert_eq!(error.message, "Injected candidate failure");
        unchanged(&manager);
        assert_eq!(recovery_entries(&private), 0);
        drop(manager);
        let reopened = Manager::open(private, "unused".into()).unwrap();
        unchanged(&reopened);
    }
    #[test]
    fn stale_journals_never_resurrect_a_removal_or_revert_a_later_update() {
        let root = tempfile::tempdir().unwrap();
        let journal = |old: &Installation, candidate: &Installation| {
            write_atomic(
                &root
                    .path()
                    .join("recovery")
                    .join(Uuid::new_v4().to_string())
                    .join("journal.json"),
                &serde_json::to_vec(&Journal {
                    old: Some(old.clone()),
                    candidate: candidate.clone(),
                })
                .unwrap(),
            )
            .unwrap();
        };
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let old = installation();
        manager.save(&old).unwrap();
        let mut replacement = old.clone();
        replacement.installation_id = Uuid::new_v4().to_string();
        replacement.data_generation = Uuid::new_v4().to_string();
        replacement.source = Source::Local {
            identity: Uuid::new_v4().to_string(),
        };
        replacement.digest = "b".repeat(64);
        journal(&old, &replacement);
        // The user removed the add-on after the failed replacement.
        manager
            .registry
            .lock()
            .unwrap()
            .execute("DELETE FROM installations", [])
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert!(manager.list().unwrap().is_empty(), "no resurrection");
        assert_eq!(recovery_entries(root.path()), 0);
        // A later successful update of the same installation.
        manager.save(&old).unwrap();
        let mut failed = old.clone();
        failed.data_generation = Uuid::new_v4().to_string();
        failed.digest = "c".repeat(64);
        let mut later = old.clone();
        later.data_generation = Uuid::new_v4().to_string();
        later.digest = "d".repeat(64);
        later.manifest.version = "1.2.0".into();
        manager.commit_replacement(&later).unwrap();
        journal(&old, &failed);
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let current = manager.installation(&old.manifest.id).unwrap();
        assert_eq!(
            current.data_generation, later.data_generation,
            "no reversion"
        );
        assert_eq!(current.digest, later.digest);
        assert_eq!(recovery_entries(root.path()), 0);
    }
    #[tokio::test]
    async fn updates_keep_enablement_and_paused_installs_start_after_resume() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        manager.pause_all().await;
        // Paused: the choice is saved; no probe starts (the host cannot start).
        let review = reviews
            .prepare(
                &manager,
                package_version("1.0.0"),
                Source::Local {
                    identity: Uuid::new_v4().to_string(),
                },
            )
            .unwrap();
        assert!(review.installed.is_none());
        let installed = reviews
            .accept(&manager, &review.token, true, false)
            .await
            .unwrap();
        assert!(installed.desired_enabled);
        assert!(matches!(installed.status, Status::EnabledIdle));
        let update = |version: &str, enable: bool| {
            let reviews = &reviews;
            let manager = &manager;
            let source = installed.source.clone();
            let version = version.to_owned();
            async move {
                let review = reviews
                    .prepare(manager, package_version(&version), source)
                    .unwrap();
                let current = review.installed.clone().unwrap();
                let result = reviews
                    .accept(manager, &review.token, enable, false)
                    .await
                    .unwrap();
                (current, result)
            }
        };
        // An enabled add-on stays enabled when the choice is left unchecked.
        let (current, updated) = update("2.0.0", false).await;
        assert!(current.desired_enabled);
        assert!(updated.desired_enabled);
        // A disabled add-on stays disabled...
        let mut disabled = manager.installation(&installed.manifest.id).unwrap();
        disabled.desired_enabled = false;
        disabled.status = Status::InstalledDisabled;
        manager.save(&disabled).unwrap();
        let (current, updated) = update("3.0.0", false).await;
        assert!(!current.desired_enabled);
        assert!(!updated.desired_enabled);
        assert!(matches!(updated.status, Status::InstalledDisabled));
        // ...unless the user explicitly enables it.
        let (_, updated) = update("4.0.0", true).await;
        assert!(updated.desired_enabled);
        assert!(manager.ensure_active(&installed.manifest.id).await.is_err());
    }
    #[tokio::test]
    async fn update_marks_the_installed_record_updating_until_it_finishes() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let source = Source::Local {
            identity: Uuid::new_v4().to_string(),
        };
        let first = reviews
            .prepare(&manager, package_version("1.0.0"), source.clone())
            .unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let review = reviews
            .prepare(&manager, package_version("2.0.0"), source)
            .unwrap();
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *reviews.interruption.lock().unwrap() =
            Some(("data-snapshotted", reached.clone(), Some(resume.clone())));
        let update = reviews.accept(&manager, &review.token, false, false);
        tokio::pin!(update);
        tokio::select! {
            result = &mut update => panic!("update escaped checkpoint: {}", result.is_ok()),
            _ = reached.notified() => {},
        }
        assert!(matches!(
            manager.installation(&old.manifest.id).unwrap().status,
            Status::Updating
        ));
        resume.notify_one();
        let updated = update.await.unwrap();
        let current = manager.installation(&old.manifest.id).unwrap();
        assert!(matches!(current.status, Status::InstalledDisabled));
        assert_eq!(current.digest, updated.digest);
    }
    #[tokio::test]
    async fn reaccepting_the_installed_release_keeps_the_rollback_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let source = Source::Local {
            identity: Uuid::new_v4().to_string(),
        };
        let first = reviews
            .prepare(&manager, package_version("1.0.0"), source.clone())
            .unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let second = reviews
            .prepare(&manager, package_version("2.0.0"), source.clone())
            .unwrap();
        let updated = reviews
            .accept(&manager, &second.token, false, false)
            .await
            .unwrap();
        let again = reviews
            .prepare(&manager, package_version("2.0.0"), source)
            .unwrap();
        assert!(!again.expands_access && again.added.is_empty() && again.removed.is_empty());
        assert!(reviews
            .accept(&manager, &again.token, false, false)
            .await
            .err()
            .unwrap()
            .message
            .contains("already installed"));
        let current = manager.installation(&old.manifest.id).unwrap();
        assert_eq!(current.data_generation, updated.data_generation);
        assert_eq!(
            current.previous.unwrap().data_generation,
            old.data_generation
        );
    }
    #[tokio::test]
    async fn local_reimport_needs_explicit_source_replacement_and_gets_a_new_identity() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, super::super::package::fixture_archive()).unwrap();
        let first = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let second = reviews.prepare_local(&manager, &file).unwrap();
        assert!(second.replaces_source);
        let error = reviews
            .accept(&manager, &second.token, false, false)
            .await
            .err()
            .unwrap();
        assert_eq!(error.data.code, ErrorCode::PermissionDenied);
        let current = manager.installation(&old.manifest.id).unwrap();
        assert_eq!(current.installation_id, old.installation_id);
        assert_eq!(current.source, old.source);
        let third = reviews.prepare_local(&manager, &file).unwrap();
        let replaced = reviews
            .accept(&manager, &third.token, false, true)
            .await
            .unwrap();
        assert_ne!(replaced.installation_id, old.installation_id);
        assert_ne!(replaced.source, old.source);
        assert!(matches!(replaced.source, Source::Local { .. }));
        assert_eq!(
            replaced.grant.as_ref().unwrap().installation_id,
            replaced.installation_id
        );
        assert!(replaced.previous.is_none());
        assert_eq!(manager.list().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn source_replacement_deletes_the_retired_installation_without_a_manual_retry() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let file = root.path().join("fixture.cmxaddon");
        std::fs::write(&file, super::super::package::fixture_archive()).unwrap();
        let first = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let key = super::super::credentials::Credentials::key("token", "https://api.example.com");
        manager
            .credentials
            .set(&old.installation_id, &key, "synthetic-session".into(), true)
            .await
            .unwrap();
        let state = root.path().join("state").join(&old.installation_id);
        assert!(state.exists());
        let second = reviews.prepare_local(&manager, &file).unwrap();
        reviews
            .accept(&manager, &second.token, false, true)
            .await
            .unwrap();
        assert!(manager.cleanup_warnings().unwrap().is_empty());
        assert!(!state.exists());
        assert_eq!(
            manager
                .credentials
                .get(&old.installation_id, &key)
                .await
                .unwrap(),
            None
        );
    }
    #[tokio::test]
    async fn review_access_changes_count_only_additions_as_expansion() {
        use codemux_addon_protocol::manifest::{Credential, CredentialType, HttpGrant};
        let origin = "https://api.example.com";
        let base = package_with(|m| {
            m.permissions = vec![Permission::WorkspaceRead];
            m.http = vec![HttpGrant {
                origin: origin.into(),
                methods: vec![HttpMethod::GET],
                credential: None,
            }];
        });
        let wider = package_with(|m| {
            m.version = "2.0.0".into();
            m.permissions = vec![Permission::GitRead, Permission::WorkspaceRead];
            m.http = vec![HttpGrant {
                origin: origin.into(),
                methods: vec![HttpMethod::POST, HttpMethod::GET],
                credential: Some("token".into()),
            }];
            m.credentials = vec![Credential {
                id: "token".into(),
                label: "API token".into(),
                origin: origin.into(),
                kind: CredentialType::Bearer,
            }];
        });
        let added = Access::missing(Some(&base.manifest), &wider.manifest);
        assert_eq!(added.permissions, vec![Permission::GitRead]);
        assert_eq!(added.http.len(), 1);
        assert_eq!(added.http[0].methods, vec![HttpMethod::POST]);
        assert_eq!(added.credentials[0].id, "token");
        assert!(Access::missing(Some(&wider.manifest), &base.manifest).is_empty());
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let source = Source::Local {
            identity: Uuid::new_v4().to_string(),
        };
        let first = reviews.prepare(&manager, base, source.clone()).unwrap();
        assert!(first.expands_access && first.installed.is_none());
        reviews
            .accept(&manager, &first.token, false, false)
            .await
            .unwrap();
        let expanded = reviews.prepare(&manager, wider, source.clone()).unwrap();
        assert!(expanded.expands_access);
        assert_eq!(expanded.installed.as_ref().unwrap().version, "1.0.0");
        assert!(expanded.removed.is_empty());
        reviews.cancel(&expanded.token);
        // Removing access is not an expansion and needs no new review.
        let narrower = package_with(|m| m.version = "2.0.0".into());
        let reduced = reviews.prepare(&manager, narrower, source).unwrap();
        assert!(!reduced.expands_access && reduced.added.is_empty());
        assert_eq!(reduced.removed.permissions, vec![Permission::WorkspaceRead]);
        assert_eq!(reduced.removed.http[0].methods, vec![HttpMethod::GET]);
    }
    #[tokio::test]
    async fn cancelled_development_review_cannot_change_an_installed_tuple() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let reviews = Reviews::default();
        let path = root.path().join("development.cmxaddon");
        std::fs::write(&path, super::super::package::fixture_archive()).unwrap();
        let review = reviews.prepare_development(&manager, &path).unwrap();
        assert!(review.development);
        let installed = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let review = reviews
            .prepare_reload(
                &manager,
                &path,
                &installed.manifest.id,
                installed.source.clone(),
                cancel.clone(),
            )
            .unwrap();
        cancel.cancel();
        assert!(reviews
            .accept(&manager, &review.token, false, false)
            .await
            .is_err());
        assert_eq!(
            manager
                .installation(&installed.manifest.id)
                .unwrap()
                .data_generation,
            installed.data_generation
        );
    }
    pub(crate) fn package_version(version: &str) -> Package {
        package_with(|manifest| manifest.version = version.into())
    }
    pub(crate) fn package_with(edit: impl FnOnce(&mut super::super::Manifest)) -> Package {
        package_from(super::super::package::fixture_archive(), edit)
    }
    pub(crate) fn package_with_source(
        source: &[u8],
        edit: impl FnOnce(&mut super::super::Manifest),
    ) -> Package {
        package_from(
            super::super::package::fixture_archive_with_source(source),
            edit,
        )
    }
    fn package_from(archive: Vec<u8>, edit: impl FnOnce(&mut super::super::Manifest)) -> Package {
        let package = Package::parse(archive, None).unwrap();
        let mut files = package.files;
        let mut manifest = package.manifest;
        edit(&mut manifest);
        files.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        Package::parse(archive.into_inner().unwrap().finish().unwrap(), None).unwrap()
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "Requires a bounded disposable CI tmpfs; scripts/addons/full-filesystem.sh"]
    async fn full_filesystem_preserves_release_data_and_grant_tuple() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(
            std::env::var("RUNNER_ENVIRONMENT").as_deref(),
            Ok("github-hosted")
        );
        let base = std::path::PathBuf::from(std::env::var_os("CODEMUX_TEST_FULL_FS").unwrap());
        let path = std::ffi::CString::new(base.as_os_str().as_bytes()).unwrap();
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        assert_eq!(unsafe { libc::statfs(path.as_ptr(), fs.as_mut_ptr()) }, 0);
        let fs = unsafe { fs.assume_init() };
        assert_eq!(fs.f_type, libc::TMPFS_MAGIC);
        assert!(
            fs.f_blocks * fs.f_bsize as u64 <= 32 * 1024 * 1024,
            "Refuse to fill a filesystem larger than the dedicated CI fixture"
        );
        for stage in [
            "package-write",
            "package-staged",
            "journal-saved",
            "data-snapshotted",
            "registry-switched",
            "activated",
        ] {
            let root = tempfile::tempdir_in(&base).unwrap();
            let manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let reviews = Reviews::default();
            let first = reviews
                .prepare(
                    &manager,
                    package_version("1.0.0"),
                    Source::Local {
                        identity: Uuid::new_v4().to_string(),
                    },
                )
                .unwrap();
            let old = reviews
                .accept(&manager, &first.token, false, false)
                .await
                .unwrap();
            let state_path = |record: &Installation| {
                root.path()
                    .join("private/state")
                    .join(&record.installation_id)
                    .join(&record.data_generation)
                    .join("state.sqlite")
            };
            Storage::open(&state_path(&old))
                .unwrap()
                .set(
                    "global",
                    "fixture",
                    &serde_json::json!({"value":"original"}),
                )
                .unwrap();
            let review = reviews
                .prepare(&manager, package_version("2.0.0"), old.source.clone())
                .unwrap();
            let reached = Arc::new(tokio::sync::Notify::new());
            let resume = Arc::new(tokio::sync::Notify::new());
            *reviews.interruption.lock().unwrap() =
                Some((stage, reached.clone(), Some(resume.clone())));
            let completed = {
                let transaction = reviews.accept(&manager, &review.token, false, false);
                tokio::pin!(transaction);
                tokio::select! {
                    result = &mut transaction => panic!("{stage}: completed before fill: {}", result.is_ok()),
                    _ = reached.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("{stage}: checkpoint not reached"),
                }
                let filler_path = root.path().join("fill.bin");
                let mut filler = std::fs::File::create(&filler_path).unwrap();
                let block = [0u8; 4096];
                loop {
                    match filler.write_all(&block) {
                        Ok(()) => {}
                        Err(error) => {
                            assert_eq!(
                                error.raw_os_error(),
                                Some(libc::ENOSPC),
                                "{stage}: {error}"
                            );
                            break;
                        }
                    }
                }
                drop(filler);
                resume.notify_one();
                let completed =
                    tokio::time::timeout(std::time::Duration::from_secs(5), &mut transaction)
                        .await
                        .expect("full filesystem must not strand transaction")
                        .ok();
                std::fs::remove_file(filler_path).unwrap();
                completed
            };
            // The registry may reuse allocated pages even with zero free blocks.
            // Whether commit succeeds or fails, recovery must select one whole
            // release/data/grant tuple, never a mixture or a lost installation.
            drop(manager);
            let reopened = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let installed = reopened.installation(&old.manifest.id).unwrap();
            let expected = completed.as_ref().unwrap_or(&old);
            assert_eq!(installed.digest, expected.digest, "{stage}");
            assert_eq!(
                installed.data_generation, expected.data_generation,
                "{stage}"
            );
            assert_eq!(installed.installation_id, old.installation_id, "{stage}");
            assert_eq!(installed.source, old.source, "{stage}");
            assert_eq!(
                installed.grant.as_ref().unwrap().digest,
                installed.digest,
                "{stage}"
            );
            assert_eq!(
                Storage::open(&state_path(&installed))
                    .unwrap()
                    .get("global", "fixture")
                    .unwrap(),
                serde_json::json!({"value":"original"}),
                "{stage}"
            );
            println!(
                "{stage}: real ENOSPC; committed={}; tuple preserved",
                completed.is_some()
            );
        }
    }
    #[tokio::test]
    async fn interrupted_update_recovers_at_every_durable_transition() {
        for stage in [
            "package-staged",
            "journal-saved",
            "data-snapshotted",
            "candidate-probed",
            "registry-switched",
            "activated",
            "completion-committed",
            "journal-removed",
        ] {
            let root = tempfile::tempdir().unwrap();
            let manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let reviews = Reviews::default();
            let first = reviews
                .prepare(
                    &manager,
                    package_version("1.0.0"),
                    Source::Local {
                        identity: Uuid::new_v4().to_string(),
                    },
                )
                .unwrap();
            let old = reviews
                .accept(&manager, &first.token, false, false)
                .await
                .unwrap();
            let state_path = |record: &Installation| {
                root.path()
                    .join("private/state")
                    .join(&record.installation_id)
                    .join(&record.data_generation)
                    .join("state.sqlite")
            };
            Storage::open(&state_path(&old))
                .unwrap()
                .set(
                    "global",
                    "fixture",
                    &serde_json::json!({"value":"original"}),
                )
                .unwrap();
            let review = reviews
                .prepare(&manager, package_version("2.0.0"), old.source.clone())
                .unwrap();
            let reached = Arc::new(tokio::sync::Notify::new());
            *reviews.interruption.lock().unwrap() = Some((stage, reached.clone(), None));
            tokio::select! {
                result = reviews.accept(&manager, &review.token, false, false) => panic!("{stage}: transaction completed before interruption: {}", result.is_ok()),
                _ = reached.notified() => {},
                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("{stage}: checkpoint not reached"),
            }
            drop(manager);
            let reopened = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let installed = reopened.installation(&old.manifest.id).unwrap();
            let complete = matches!(stage, "completion-committed" | "journal-removed");
            assert_eq!(
                installed.digest,
                if complete {
                    review.digest
                } else {
                    old.digest.clone()
                },
                "{stage}"
            );
            assert_eq!(installed.installation_id, old.installation_id, "{stage}");
            assert_eq!(installed.source, old.source, "{stage}");
            assert!(!installed.desired_enabled, "{stage}");
            assert_eq!(
                Storage::open(&state_path(&installed))
                    .unwrap()
                    .get("global", "fixture")
                    .unwrap(),
                serde_json::json!({"value":"original"}),
                "{stage}"
            );
            assert_eq!(
                installed.grant.as_ref().unwrap().digest,
                installed.digest,
                "{stage}"
            );
            if complete {
                assert_eq!(
                    installed.previous.as_ref().unwrap().data_generation,
                    old.data_generation,
                    "{stage}"
                );
            } else {
                assert_eq!(installed.data_generation, old.data_generation, "{stage}");
            }
            assert_eq!(
                std::fs::read_dir(root.path().join("private/recovery"))
                    .unwrap()
                    .count(),
                0,
                "{stage}"
            );
        }
    }
    #[tokio::test]
    async fn uninstall_waits_for_update_and_removes_the_committed_candidate() {
        for stage in [
            "data-snapshotted",
            "registry-switched",
            "completion-committed",
        ] {
            let root = tempfile::tempdir().unwrap();
            let manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let reviews = Reviews::default();
            let first = reviews
                .prepare(
                    &manager,
                    package_version("1.0.0"),
                    Source::Local {
                        identity: Uuid::new_v4().to_string(),
                    },
                )
                .unwrap();
            let old = reviews
                .accept(&manager, &first.token, false, false)
                .await
                .unwrap();
            let review = reviews
                .prepare(&manager, package_version("2.0.0"), old.source.clone())
                .unwrap();
            let reached = Arc::new(tokio::sync::Notify::new());
            let resume = Arc::new(tokio::sync::Notify::new());
            *reviews.interruption.lock().unwrap() =
                Some((stage, reached.clone(), Some(resume.clone())));
            let update = reviews.accept(&manager, &review.token, false, false);
            tokio::pin!(update);
            tokio::select! {
                result = &mut update => panic!("{stage}: update escaped checkpoint: {}", result.is_ok()),
                _ = reached.notified() => {},
                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("{stage}: checkpoint not reached"),
            }
            let removal = manager.remove(&old.manifest.id, false);
            tokio::pin!(removal);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), &mut removal)
                    .await
                    .is_err(),
                "{stage}: removal overlapped update"
            );
            resume.notify_one();
            let (updated, removed) =
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    tokio::join!(&mut update, &mut removal)
                })
                .await
                .expect("serialized operations finish");
            assert_eq!(updated.unwrap().digest, review.digest, "{stage}");
            assert!(removed.unwrap().is_empty(), "{stage}");
            assert!(manager.list().unwrap().is_empty(), "{stage}");
            assert!(
                !manager
                    .root
                    .join("state")
                    .join(&old.installation_id)
                    .exists(),
                "{stage}"
            );
            assert!(
                !manager
                    .root
                    .join("packages")
                    .join(&old.manifest.id)
                    .join(&review.digest)
                    .exists(),
                "{stage}"
            );
            let reopened = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            assert!(reopened.list().unwrap().is_empty(), "{stage}");
        }
    }
    #[test]
    fn incomplete_update_restores_exact_previous_tuple_and_private_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let old = installation();
        manager.save(&old).unwrap();
        let mut candidate = old.clone();
        candidate.digest = "b".repeat(64);
        candidate.data_generation = Uuid::new_v4().to_string();
        candidate.manifest.version = "1.1.0".into();
        let journal = root
            .path()
            .join("recovery")
            .join(Uuid::new_v4().to_string())
            .join("journal.json");
        write_atomic(
            &journal,
            &serde_json::to_vec(&Journal {
                old: Some(old.clone()),
                candidate: candidate.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        manager.commit_replacement(&candidate).unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let restored = manager.installation(&old.manifest.id).unwrap();
        assert_eq!(restored.digest, old.digest);
        assert_eq!(restored.data_generation, old.data_generation);
        assert_eq!(restored.installation_id, old.installation_id);
        assert!(restored.desired_enabled);
        assert!(!journal.exists());
    }
    #[test]
    fn completed_update_survives_interruption_before_journal_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let old = installation();
        let mut candidate = old.clone();
        candidate.digest = "b".repeat(64);
        candidate.data_generation = Uuid::new_v4().to_string();
        let journal = root
            .path()
            .join("recovery")
            .join(Uuid::new_v4().to_string())
            .join("journal.json");
        write_atomic(
            &journal,
            &serde_json::to_vec(&Journal {
                old: Some(old),
                candidate: candidate.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        manager.save(&candidate).unwrap();
        manager
            .registry
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO metadata(key,value) VALUES(?1,'complete')",
                [format!("transaction:{}", candidate.data_generation)],
            )
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert_eq!(
            manager.installation(&candidate.manifest.id).unwrap().digest,
            candidate.digest
        );
        assert!(!journal.exists());
    }
    #[test]
    fn unclean_activation_pauses_next_launch_without_deleting_installations() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let mut installed = installation();
        installed.status = Status::EnabledRunning;
        manager.save(&installed).unwrap();
        manager
            .registry
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO metadata(key,value) VALUES('activation:example.hello','pending')",
                [],
            )
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert!(manager.paused());
        assert!(matches!(
            manager.installation(&installed.manifest.id).unwrap().status,
            Status::EnabledIdle
        ));
        assert!(manager
            .installation(&installed.manifest.id)
            .unwrap()
            .failure
            .is_some());
    }
    #[test]
    fn unclean_exit_names_only_the_add_on_that_was_starting() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let starting = installation();
        manager.save(&starting).unwrap();
        let mut running = installation();
        running.manifest.id = "example.running".into();
        running.status = Status::EnabledRunning;
        manager.save(&running).unwrap();
        manager
            .registry
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO metadata(key,value) VALUES('activation:example.hello','pending')",
                [],
            )
            .unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        assert!(manager.paused());
        let starting = manager.installation("example.hello").unwrap();
        assert!(matches!(starting.status, Status::EnabledIdle));
        assert!(starting.failure.unwrap().contains("starting"));
        let running = manager.installation("example.running").unwrap();
        assert!(matches!(running.status, Status::EnabledIdle));
        assert!(running.failure.is_none());
        assert_eq!(manager.interrupted_activations(), vec!["example.hello"]);
        manager.resume().unwrap();
        assert!(manager.interrupted_activations().is_empty());
    }
    #[test]
    fn interrupted_removal_finishes_on_next_launch_with_the_saved_data_choice() {
        for keep in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
            let mut removing = installation();
            removing.desired_enabled = false;
            removing.status = Status::Removing;
            if keep {
                manager
                    .registry
                    .lock()
                    .unwrap()
                    .execute(
                        "INSERT INTO metadata(key,value) VALUES(?1,'keep')",
                        [format!("removing:{}", removing.installation_id)],
                    )
                    .unwrap();
            }
            manager.save(&removing).unwrap();
            drop(manager);
            let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
            assert!(manager.list().unwrap().is_empty(), "keep={keep}");
            let db = manager.registry.lock().unwrap();
            let files: String = db
                .query_row(
                    "SELECT record FROM file_cleanup WHERE installation=?1",
                    [&removing.installation_id],
                    |r| r.get(0),
                )
                .unwrap();
            let files: serde_json::Value = serde_json::from_str(&files).unwrap();
            assert_eq!(files["state"], !keep, "keep={keep}");
            let orphaned: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM metadata WHERE key=?1)",
                    [format!("orphan:{}", removing.installation_id)],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(orphaned, keep);
            let intent: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM metadata WHERE key LIKE 'removing:%')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!intent);
        }
    }
    #[tokio::test]
    async fn removal_commits_inert_tombstone_and_retry_preserves_reinstalled_bytes() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let old = installation();
        manager.save(&old).unwrap();
        let package = root
            .path()
            .join("packages")
            .join(&old.manifest.id)
            .join(&old.digest);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("immutable"), "fixture").unwrap();
        let state = root.path().join("state").join(&old.installation_id);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("private"), "fixture").unwrap();
        {
            let mut db = manager.registry.lock().unwrap();
            let tx = db.transaction().unwrap();
            super::super::cleanup::queue(&tx, &old, false).unwrap();
            tx.execute("DELETE FROM installations", []).unwrap();
            tx.commit().unwrap();
        }
        let mut replacement = old.clone();
        replacement.installation_id = Uuid::new_v4().to_string();
        manager.save(&replacement).unwrap();
        assert!(manager.retry_cleanup().await.unwrap().is_empty());
        assert!(package.join("immutable").exists());
        assert!(!state.exists());
        assert_eq!(manager.list().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn stopping_a_blocked_installation_preserves_reason_and_status() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let mut installed = installation();
        installed.desired_enabled = false;
        installed.status = Status::BlockedDisabled;
        installed.failure = Some("Revoked".into());
        manager.save(&installed).unwrap();
        manager.stop(&installed.manifest.id, None).await;
        let stopped = manager.installation(&installed.manifest.id).unwrap();
        assert!(matches!(stopped.status, Status::BlockedDisabled));
        assert_eq!(stopped.failure.as_deref(), Some("Revoked"));
    }
}
