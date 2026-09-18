//! Explicit review plus durable package/data tuple changes. Packages remain inert
//! until a host-owned review is accepted; no author install scripts exist.
use super::{
    manager::{Installation, Manager, Status},
    package::Package,
    permissions::{capability_digest, Grant, Source},
    storage::Storage,
    ErrorCode, ProtocolError, Result,
};
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
    pub expands_access: bool,
    pub compressed_bytes: usize,
    pub development: bool,
    pub retained_data: Option<RetainedData>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedData {
    pub version: String,
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
        let review = Review {
            token: Uuid::new_v4().to_string(),
            manifest: package.manifest.clone(),
            digest: package.digest.clone(),
            source,
            replaces_source,
            expands_access: old.as_ref().is_none_or(|i| {
                capability_digest(&i.manifest) != capability_digest(&package.manifest)
            }),
            compressed_bytes: package.archive.len(),
            development: false,
            retained_data: retained.as_ref().map(|i| RetainedData {
                version: i.manifest.version.clone(),
            }),
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
        let mut candidate = Installation {
            installation_id: installation_id.clone(),
            manifest: pending.package.manifest.clone(),
            source: source.clone(),
            digest: pending.package.digest.clone(),
            desired_enabled: enable,
            status: if enable {
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
        if !directory.exists() {
            let staging = manager
                .root
                .join("staging")
                .join(Uuid::new_v4().to_string());
            std::fs::create_dir_all(&staging).map_err(|_| unavailable())?;
            for (name, bytes) in &pending.package.files {
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
        let result: Result<()> = async {
            active_review()?;
            manager.stop(id, None).await;
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
            if enable {
                manager
                    .activate(candidate.clone(), pending.package.source(), true)
                    .await?;
                manager.stop(id, None).await;
            }
            active_review()?;
            manager.commit_replacement(old.as_ref(), &candidate)?;
            if enable {
                manager
                    .activate(candidate.clone(), pending.package.source(), false)
                    .await?;
                candidate.status = Status::EnabledRunning;
            }
            active_review()?;
            // Completion and retirement of a replaced source share a durable
            // commit marker; recovery must never delete restored credentials.
            let mut db = manager.registry.lock().unwrap();
            let tx = db.transaction().map_err(|_| unavailable())?;
            if let Some(old) = &old {
                if old.installation_id != candidate.installation_id {
                    super::cleanup::queue(&tx, old, false)?;
                }
            }
            if let Some(retained) = retained {
                tx.execute(
                    "DELETE FROM metadata WHERE key=?1",
                    [format!("orphan:{}", retained.installation_id)],
                )
                .map_err(|_| unavailable())?;
                super::cleanup::queue(&tx, retained, false)?;
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
            if let Some(old) = &old {
                let mut restored = old.clone();
                restored.status = if restored.desired_enabled {
                    Status::EnabledIdle
                } else {
                    Status::InstalledDisabled
                };
                manager.commit_replacement(Some(&candidate), &restored)?;
            } else {
                manager
                    .registry
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM installations WHERE id=?1",
                        [&candidate.installation_id],
                    )
                    .map_err(|_| unavailable())?;
            }
            // If restoration cannot be persisted, leave the journal for recovery.
            std::fs::remove_dir_all(journal.parent().unwrap()).map_err(|_| unavailable())?;
            return Err(error);
        }
        std::fs::remove_dir_all(journal.parent().unwrap()).map_err(|_| unavailable())?;

        manager
            .registry
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM metadata WHERE key=?1",
                [format!("transaction:{}", candidate.data_generation)],
            )
            .map_err(|_| unavailable())?;
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
    fn commit_replacement(
        &self,
        old: Option<&Installation>,
        candidate: &Installation,
    ) -> Result<()> {
        candidate.validate_record()?;
        let mut db = self.registry.lock().unwrap();
        let tx = db.transaction().map_err(|_| unavailable())?;
        if let Some(old) = old {
            tx.execute(
                "DELETE FROM installations WHERE id=?1",
                [&old.installation_id],
            )
            .map_err(|_| unavailable())?;
        }
        tx.execute(
            "INSERT INTO installations(id,plugin_id,record) VALUES(?1,?2,?3)",
            rusqlite::params![
                candidate.installation_id,
                candidate.manifest.id,
                serde_json::to_string(candidate).unwrap()
            ],
        )
        .map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        let _ = self.events.send(super::manager::UiEvent::Inventory);
        Ok(())
    }
    pub fn recover(&self) -> Result<()> {
        let root = self.root.join("recovery");
        if !root.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(root)
            .map_err(|_| unavailable())?
            .take(1000)
        {
            let entry = entry.map_err(|_| unavailable())?;
            let path = entry.path().join("journal.json");
            if entry.file_type().map_err(|_| unavailable())?.is_dir() && !path.exists() {
                // Interrupted atomic journal write: no registry switch could
                // have happened before this file was durably renamed.
                std::fs::remove_dir_all(entry.path()).map_err(|_| unavailable())?;
                continue;
            }
            if !entry.file_type().map_err(|_| unavailable())?.is_dir()
                || path
                    .symlink_metadata()
                    .map_err(|_| unavailable())?
                    .file_type()
                    .is_symlink()
            {
                return Err(unavailable());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|_| unavailable())?
                .take(512 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| unavailable())?;
            if bytes.len() > 512 * 1024 {
                return Err(unavailable());
            }
            let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
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
                std::fs::remove_dir_all(entry.path()).map_err(|_| unavailable())?;
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
            if let Some(old) = journal.old {
                self.commit_replacement(current.as_ref(), &old)?;
            } else if current
                .as_ref()
                .is_some_and(|i| i.installation_id == journal.candidate.installation_id)
            {
                self.registry
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM installations WHERE id=?1",
                        [&journal.candidate.installation_id],
                    )
                    .map_err(|_| unavailable())?;
            }
            std::fs::remove_dir_all(entry.path()).map_err(|_| unavailable())?;
        }
        Ok(())
    }
    pub async fn remove(&self, id: &str, keep_data: bool) -> Result<Vec<String>> {
        let operation = self.operation(id).await;
        let _lock = operation.lock().await;
        let mut installation = self.installation(id)?;
        installation.desired_enabled = false;
        installation.status = Status::Removing;
        self.save(&installation)?;
        self.stop(id, None).await;
        {
            let mut db = self.registry.lock().unwrap();
            let tx = db.transaction().map_err(|_| unavailable())?;
            super::cleanup::queue(&tx, &installation, keep_data)?;
            if keep_data {
                tx.execute(
                    "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,?2)",
                    rusqlite::params![
                        format!("orphan:{}", installation.installation_id),
                        serde_json::to_string(&installation).unwrap()
                    ],
                )
                .map_err(|_| unavailable())?;
            }
            tx.execute(
                "DELETE FROM installations WHERE id=?1",
                [&installation.installation_id],
            )
            .map_err(|_| unavailable())?;
            tx.commit().map_err(|_| unavailable())?;
        }
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
        let result: Result<()> = async {
            self.stop(id, None).await;
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
            if candidate.desired_enabled {
                self.activate(candidate.clone(), package.source(), true)
                    .await?;
                self.stop(id, None).await;
            }
            self.commit_replacement(Some(&old), &candidate)?;
            if candidate.desired_enabled {
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
            let mut restored = old;
            restored.status = if restored.desired_enabled {
                Status::EnabledIdle
            } else {
                Status::InstalledDisabled
            };
            self.commit_replacement(Some(&candidate), &restored)?;
            std::fs::remove_dir_all(journal.parent().unwrap()).map_err(|_| unavailable())?;
            return Err(error);
        }
        std::fs::remove_dir_all(journal.parent().unwrap()).map_err(|_| unavailable())?;
        self.registry
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM metadata WHERE key=?1",
                [format!("transaction:{}", candidate.data_generation)],
            )
            .map_err(|_| unavailable())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        std::fs::write(&file, super::super::package::fixture_archive()).unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &review.token, false, false)
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
        assert!(manager.retry_cleanup().await.unwrap().is_empty());
        assert!(!old_db.exists());
        assert!(db.exists());
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
        manager.commit_replacement(Some(&old), &candidate).unwrap();
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
