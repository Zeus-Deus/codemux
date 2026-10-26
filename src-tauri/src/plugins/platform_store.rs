//! Plugin-only persistence. A damaged or future record is retained and refused in isolation.
use super::platform_manifest::{valid_identifier, PlatformManifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformGrants {
    pub capabilities: Vec<String>,
    pub project_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectReceipt {
    pub fingerprint: String,
    pub command_key: String,
    pub method: String,
    pub principal: String,
    /// Reserved before crossing into the core service. Replays reconcile the core command receipt.
    pub state: String,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub source_thread_id: Option<String>,
    pub project_id: String,
    #[serde(default)]
    pub core_tool: Option<String>,
    #[serde(default)]
    pub core_args: Option<Value>,
    /// Additional grants used to produce the effect must also hold on recovery.
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationRecord {
    pub schema_version: u32,
    pub installation_id: String,
    pub generation: String,
    pub manifest: PlatformManifest,
    pub digest: String,
    pub consented_digest: Option<String>,
    pub enabled: bool,
    pub unavailable_reason: Option<String>,
    pub grants: PlatformGrants,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, Vec<u8>>,
    #[serde(default)]
    pub storage: BTreeMap<String, Value>,
    #[serde(default)]
    pub receipts: BTreeMap<String, EffectReceipt>,
    #[serde(default)]
    pub event_cursor: i64,
    #[serde(default)]
    pub project_grant_starts: BTreeMap<String, i64>,
    #[serde(default)]
    pub acknowledged_deliveries: BTreeMap<String, i64>,
    #[serde(default)]
    pub event_backlog_choice_required: bool,
    #[serde(default)]
    pub delivery_error: Option<String>,
    #[serde(default)]
    pub event_effects: BTreeMap<String, Vec<String>>,
}

pub struct PlatformStore {
    root: PathBuf,
    // One app-owned store instance serializes read/modify/rename. Never held across an await.
    gate: Mutex<()>,
}

impl PlatformStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Mutex::new(()),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_identifier(id) {
            return Err("invalid_input: Invalid installation ID".into());
        }
        Ok(self.root.join("installations").join(format!("{id}.json")))
    }
    fn read_unlocked(&self, id: &str) -> Result<Option<InstallationRecord>, String> {
        let path = self.path(id)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("storage_failed: {e}")),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_RECORD_BYTES
        {
            return Err(
                "unavailable: Plugin record is unsafe or too large; retained without modification"
                    .into(),
            );
        }
        let bytes = fs::read(path).map_err(|e| format!("storage_failed: {e}"))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(
                "unavailable: Oversized plugin record retained without modification".into(),
            );
        }
        let raw: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "unavailable: Corrupt plugin record retained without modification")?;
        if raw["schemaVersion"].as_u64() != Some(1) {
            return Err("incompatible: Plugin record requires a different host; retained without modification".into());
        }
        let record: InstallationRecord = serde_json::from_value(raw).map_err(|_| {
            "unavailable: Plugin record cannot be decoded; retained without modification"
        })?;
        if record.installation_id != id {
            return Err("unavailable: Plugin record identity mismatch".into());
        }
        Ok(Some(record))
    }
    pub fn read(&self, id: &str) -> Result<InstallationRecord, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin store lock failed")?;
        self.read_unlocked(id)?
            .ok_or_else(|| "not_found: Plugin installation not found".into())
    }
    pub fn update<T>(
        &self,
        id: &str,
        change: impl FnOnce(&mut InstallationRecord) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin store lock failed")?;
        let mut record = self
            .read_unlocked(id)?
            .ok_or("not_found: Plugin installation not found")?;
        let result = change(&mut record)?;
        self.write_unlocked(&record)?;
        Ok(result)
    }
    pub fn upsert<T>(
        &self,
        id: &str,
        change: impl FnOnce(Option<InstallationRecord>) -> Result<(InstallationRecord, T), String>,
    ) -> Result<T, String> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "storage_failed: Plugin store lock failed")?;
        let (record, result) = change(self.read_unlocked(id)?)?;
        if record.installation_id != id {
            return Err("invalid_input: Installation identity mismatch".into());
        }
        self.write_unlocked(&record)?;
        Ok(result)
    }
    fn write_unlocked(&self, record: &InstallationRecord) -> Result<(), String> {
        let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("storage_failed: Plugin record limit exceeded".into());
        }
        let path = self.path(&record.installation_id)?;
        private_directory(&self.root)?;
        private_directory(path.parent().ok_or("Invalid plugin store path")?)?;
        atomic_write(&path, &bytes)
    }
    pub(crate) fn ids(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join("installations")) else {
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
        ids
    }
    /// Invalid installations are returned as diagnostics; healthy siblings remain usable.
    pub fn list(&self) -> Vec<(String, Result<InstallationRecord, String>)> {
        self.ids()
            .into_iter()
            .map(|id| {
                let record = self.read(&id);
                (id, record)
            })
            .collect()
    }
    pub fn data_directory(&self, id: &str) -> Result<PathBuf, String> {
        self.path(id)?;
        private_directory(&self.root)?;
        let base = self.root.join("data");
        private_directory(&base)?;
        let path = base.join(id);
        private_directory(&path)?;
        Ok(path)
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("storage_failed: Plugin directory must not be a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid store path")?)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| format!("storage_failed: {e}"))?;
    file.persist(path)
        .map_err(|e| format!("storage_failed: {e}"))?;
    #[cfg(unix)]
    fs::File::open(path.parent().ok_or("Invalid store path")?)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| format!("storage_failed: {e}"))?;
    Ok(())
}
pub(crate) fn digest(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            v => v.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(canonical(value).to_string().as_bytes())
    )
}
