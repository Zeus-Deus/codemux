//! Package review state is separate from plugin data and from the application's database.
use super::{
    platform_manifest::{valid_identifier, PlatformManifest},
    platform_package::{PlatformPackageSource, PlatformSnapshot},
    platform_store::{atomic_write, private_directory, InstallationRecord},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};

const MAX_MANAGEMENT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageReview {
    pub digest: String,
    pub manifest: PlatformManifest,
    pub source: PlatformPackageSource,
    #[serde(default)]
    pub kind: Option<String>,
}
impl From<PlatformSnapshot> for PackageReview {
    fn from(snapshot: PlatformSnapshot) -> Self {
        Self {
            digest: snapshot.digest,
            manifest: snapshot.manifest,
            source: snapshot.source,
            kind: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviousConfiguration {
    pub package: PackageReview,
    pub grants: super::platform_store::PlatformGrants,
    pub settings: std::collections::BTreeMap<String, serde_json::Value>,
    pub secrets: std::collections::BTreeMap<String, Vec<u8>>,
}
impl PreviousConfiguration {
    pub fn capture(package: PackageReview, record: &InstallationRecord) -> Self {
        Self {
            package,
            grants: record.grants.clone(),
            settings: record.settings.clone(),
            secrets: record.secrets.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagementRecord {
    pub schema_version: u32,
    pub installation_id: String,
    #[serde(default)]
    pub data_owner: Option<String>,
    pub current: PackageReview,
    pub pending_update: Option<PackageReview>,
    #[serde(default)]
    pub pending_configuration: Option<PreviousConfiguration>,
    pub previous: Option<PreviousConfiguration>,
    pub repair_directory: Option<String>,
    /// A local repair must not erase the source used to check publisher updates.
    pub publisher_source: Option<PlatformPackageSource>,
    pub removed: bool,
    pub updated_at: String,
}

pub struct ManagementStore {
    root: PathBuf,
    gate: Mutex<()>,
}
impl ManagementStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Mutex::new(()),
        }
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_identifier(id) {
            return Err("invalid_input: Invalid plugin installation".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    fn read_unlocked(&self, id: &str) -> Result<Option<ManagementRecord>, String> {
        let path = self.path(id)?;
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err("storage_failed: Cannot inspect plugin package review state".into())
            }
        };
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MANAGEMENT_BYTES {
            return Err(
                "unavailable: Unsafe or oversized plugin record retained without modification"
                    .into(),
            );
        }
        let bytes = fs::read(path)
            .map_err(|_| "storage_failed: Cannot read plugin package review state")?;
        if bytes.len() as u64 > MAX_MANAGEMENT_BYTES {
            return Err("unavailable: Oversized plugin review record retained".into());
        }
        let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            "unavailable: Corrupt plugin review record retained without modification"
        })?;
        if raw["schemaVersion"].as_u64() != Some(1) {
            return Err("incompatible: Plugin review record belongs to another app version; retained without modification".into());
        }
        let record: ManagementRecord = serde_json::from_value(raw).map_err(|_| {
            "unavailable: Unrecognized plugin review record retained without modification"
        })?;
        if record.installation_id != id {
            return Err("unavailable: Plugin review identity mismatch".into());
        }
        Ok(Some(record))
    }
    pub fn read_optional(&self, id: &str) -> Result<Option<ManagementRecord>, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin review lock failed")?;
        self.read_unlocked(id)
    }
    pub fn read(&self, id: &str) -> Result<ManagementRecord, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin review lock failed")?;
        self.read_unlocked(id)?
            .ok_or_else(|| "not_found: Plugin installation no longer exists".into())
    }
    pub fn update<T>(
        &self,
        id: &str,
        change: impl FnOnce(Option<ManagementRecord>) -> Result<(ManagementRecord, T), String>,
    ) -> Result<T, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin review lock failed")?;
        let (record, result) = change(self.read_unlocked(id)?)?;
        if record.schema_version != 1 || record.installation_id != id {
            return Err("invalid_input: Plugin review identity mismatch".into());
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| "storage_failed: Cannot encode plugin review state")?;
        if bytes.len() as u64 > MAX_MANAGEMENT_BYTES {
            return Err("storage_failed: Plugin review state exceeds its limit".into());
        }
        private_directory(&self.root)?;
        atomic_write(&self.path(id)?, &bytes)?;
        Ok(result)
    }
    pub fn list(&self) -> Vec<(String, Result<ManagementRecord, String>)> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return vec![];
        };
        let mut ids: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()?
                    .strip_suffix(".json")
                    .map(str::to_owned)
            })
            .collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let record = self.read(&id);
                (id, record)
            })
            .collect()
    }
}

pub fn installation_id(
    manifest: &PlatformManifest,
    source: &PlatformPackageSource,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    // Revision and local-fix paths do not change data ownership. A different
    // publisher/source with the same manifest id must get separate data.
    let identity = match source {
        PlatformPackageSource::Local { directory } => {
            let path = fs::canonicalize(directory)
                .map_err(|_| "invalid_source: Local plugin source is unavailable")?;
            serde_json::json!([manifest.id, "local", path])
        }
        PlatformPackageSource::Git { url, subdir, .. } => {
            serde_json::json!([manifest.id, "git", url, subdir])
        }
    };
    let bytes = serde_json::to_vec(&identity).map_err(|_| "Invalid plugin identity")?;
    Ok(format!("plugin-{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PlatformManifest {
        PlatformManifest::parse(br#"{"id":"example.test","name":"Test","version":"1.0.0","apiVersion":1,"entry":"plugin.mjs"}"#).unwrap()
    }
    fn record(id: &str) -> ManagementRecord {
        ManagementRecord {
            schema_version: 1,
            installation_id: id.into(),
            data_owner: Some("owner".into()),
            current: PackageReview {
                digest: "a".repeat(64),
                manifest: manifest(),
                source: PlatformPackageSource::Local {
                    directory: "/source".into(),
                },
                kind: None,
            },
            pending_update: None,
            pending_configuration: None,
            previous: None,
            repair_directory: None,
            publisher_source: None,
            removed: false,
            updated_at: "now".into(),
        }
    }
    #[test]
    fn future_or_corrupt_records_are_retained_and_do_not_hide_healthy_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let store = ManagementStore::new(dir.path().join("management"));
        store
            .update("healthy", |_| Ok((record("healthy"), ())))
            .unwrap();
        let future = br#"{"schemaVersion":99,"futureData":"keep"}"#;
        fs::write(store.path("future").unwrap(), future).unwrap();
        fs::write(store.path("broken").unwrap(), b"broken").unwrap();
        let mut unknown = serde_json::to_value(record("unknown")).unwrap();
        unknown["futureField"] = serde_json::json!({"retained":true});
        let unknown = serde_json::to_vec(&unknown).unwrap();
        fs::write(store.path("unknown").unwrap(), &unknown).unwrap();
        assert!(store
            .update("unknown", |_| Ok((record("unknown"), ())))
            .is_err());
        assert_eq!(fs::read(store.path("unknown").unwrap()).unwrap(), unknown);

        assert_eq!(
            store
                .list()
                .iter()
                .filter(|(_, value)| value.is_ok())
                .count(),
            1
        );
        assert!(store
            .update("future", |_| Ok((record("future"), ())))
            .is_err());
        assert_eq!(fs::read(store.path("future").unwrap()).unwrap(), future);
        assert_eq!(fs::read(store.path("broken").unwrap()).unwrap(), b"broken");
    }
    #[test]
    fn source_identity_separates_publishers_and_survives_revisions() {
        let source = |url: &str, commit: &str| PlatformPackageSource::Git {
            url: url.into(),
            git_ref: None,
            commit: commit.into(),
            subdir: None,
        };
        let id = installation_id(&manifest(), &source("https://one.example/plugin", "a")).unwrap();
        assert_eq!(
            id,
            installation_id(&manifest(), &source("https://one.example/plugin", "b")).unwrap()
        );
        assert_ne!(
            id,
            installation_id(&manifest(), &source("https://two.example/plugin", "a")).unwrap()
        );
    }
}
