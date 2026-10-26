//! Transport-independent isolated plugin services. Construction does no I/O and cannot gate startup.
use super::{
    platform_authority::{self, PlatformCaller},
    platform_events::PlatformEvents,
    platform_manifest::PlatformManifest,
    platform_store::{InstallationRecord, PlatformGrants, PlatformStore, MAX_JSON_BYTES},
};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Runtime};
use tokio::sync::watch;

pub struct PlatformService {
    pub store: Arc<PlatformStore>,
    pub events: PlatformEvents,
    generations: Mutex<BTreeMap<String, watch::Sender<String>>>,
    active_effects: Arc<Mutex<BTreeSet<String>>>,
}
pub(crate) struct EffectLease {
    active: Arc<Mutex<BTreeSet<String>>>,
    key: String,
}
impl Drop for EffectLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.key);
        }
    }
}

impl PlatformService {
    pub fn open(root: PathBuf) -> Self {
        Self {
            store: Arc::new(PlatformStore::new(root.clone())),
            events: PlatformEvents::new(root.join("events.sqlite")),
            generations: Mutex::new(BTreeMap::new()),
            active_effects: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }
    pub(crate) fn begin_effect(&self, key: &str) -> Result<EffectLease, String> {
        let mut active = self
            .active_effects
            .lock()
            .map_err(|_| "unavailable: Plugin effect registry failed")?;
        if !active.insert(key.into()) {
            return Err("busy: This logical plugin effect is already being admitted".into());
        }
        Ok(EffectLease {
            active: self.active_effects.clone(),
            key: key.into(),
        })
    }
    /// A package promotion is not consent. Existing data, intent, grants and receipts survive.
    pub fn register(
        &self,
        installation_id: &str,
        generation: &str,
        manifest: PlatformManifest,
        digest: &str,
    ) -> Result<InstallationRecord, String> {
        super::platform_manifest::valid_identifier(generation)
            .then_some(())
            .ok_or("invalid_input: Invalid generation")?;
        let bytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        let manifest = PlatformManifest::parse(&bytes)?;
        if digest.is_empty() || digest.len() > 256 {
            return Err("invalid_input: Invalid package digest".into());
        }
        let latest = self.events.latest()?;
        let record = self.store.upsert(installation_id, |previous| {
            let mut record = previous.unwrap_or_else(|| InstallationRecord {
                schema_version: 1,
                installation_id: installation_id.into(),
                generation: generation.into(),
                manifest: manifest.clone(),
                digest: digest.into(),
                consented_digest: None,
                enabled: true,
                unavailable_reason: None,
                grants: PlatformGrants::default(),
                settings: BTreeMap::new(),
                secrets: BTreeMap::new(),
                storage: BTreeMap::new(),
                receipts: BTreeMap::new(),
                event_cursor: latest,
                project_grant_starts: BTreeMap::new(),
                acknowledged_deliveries: BTreeMap::new(),
                event_backlog_choice_required: false,
                delivery_error: None,
                event_effects: BTreeMap::new(),
            });
            if record.digest != digest && record.event_cursor < latest {
                record.event_backlog_choice_required = true;
            }
            record.generation = generation.into();
            record.manifest = manifest;
            record.digest = digest.into();
            // Never silently grant new capabilities during a package update.
            let requested = record.manifest.requested_capabilities();
            record
                .grants
                .capabilities
                .retain(|name| requested.contains(name));
            Ok((record.clone(), record))
        })?;
        self.notify_generation(installation_id, generation)?;
        Ok(record)
    }
    pub fn consent(
        &self,
        id: &str,
        digest: &str,
        grants: PlatformGrants,
    ) -> Result<InstallationRecord, String> {
        self.consent_inner(id, digest, grants, true)
    }
    pub(crate) fn consent_pending(
        &self,
        id: &str,
        digest: &str,
        grants: PlatformGrants,
    ) -> Result<InstallationRecord, String> {
        self.consent_inner(id, digest, grants, false)
    }
    fn consent_inner(
        &self,
        id: &str,
        digest: &str,
        grants: PlatformGrants,
        clear_unavailable: bool,
    ) -> Result<InstallationRecord, String> {
        let latest = self.events.latest()?;
        let result = self.store.update(id, |record| {
            if record.digest != digest {
                return Err("stale_digest: Consent must name the exact installed snapshot".into());
            }
            platform_authority::validate_grants(&record.manifest, &grants)?;
            if !record.manifest.compatibility().available {
                return Err(
                    "incompatible: This host cannot execute the plugin requirements".into(),
                );
            }
            for project in &grants.project_ids {
                if !record.grants.project_ids.contains(project) {
                    record.project_grant_starts.insert(project.clone(), latest);
                }
            }
            record
                .project_grant_starts
                .retain(|id, _| grants.project_ids.contains(id));
            if !record
                .grants
                .capabilities
                .iter()
                .any(|v| v == "events.run-finalized")
                && grants
                    .capabilities
                    .iter()
                    .any(|v| v == "events.run-finalized")
            {
                record.event_cursor = latest;
            }
            record.grants = grants;
            record.consented_digest = Some(digest.into());
            record.generation = uuid::Uuid::new_v4().to_string();
            if clear_unavailable {
                record.unavailable_reason = None;
            }
            Ok(record.clone())
        })?;
        self.notify_generation(id, &result.generation)?;
        Ok(result)
    }
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<InstallationRecord, String> {
        let latest = self.events.latest()?;
        let record = self.store.update(id, |record| {
            if !enabled || (!record.enabled && record.event_cursor < latest) {
                record.event_backlog_choice_required = true;
            }
            record.enabled = enabled;
            record.generation = uuid::Uuid::new_v4().to_string();
            Ok(record.clone())
        })?;
        self.notify_generation(id, &record.generation)?;
        if enabled {
            self.events.clear_empty_gap(&self.store, id)
        } else {
            Ok(record)
        }
    }
    /// Failure retains the user's enabled intent. Repair/resume is explicit.
    pub fn mark_unavailable(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<InstallationRecord, String> {
        let resuming = reason.is_none();
        let record = self.store.update(id, |record| {
            record.unavailable_reason = reason;
            record.event_backlog_choice_required = true;
            record.generation = uuid::Uuid::new_v4().to_string();
            Ok(record.clone())
        })?;
        self.notify_generation(id, &record.generation)?;
        if resuming {
            self.events.clear_empty_gap(&self.store, id)
        } else {
            Ok(record)
        }
    }
    /// Publish authority and its generation together, only after the entire change validates.
    pub(crate) fn update_authority(
        &self,
        id: &str,
        change: impl FnOnce(&mut InstallationRecord) -> Result<(), String>,
    ) -> Result<InstallationRecord, String> {
        let record = self.store.update(id, |record| {
            change(record)?;
            record.generation = uuid::Uuid::new_v4().to_string();
            Ok(record.clone())
        })?;
        self.notify_generation(id, &record.generation)?;
        Ok(record)
    }
    fn notify_generation(&self, id: &str, generation: &str) -> Result<(), String> {
        let mut generations = self
            .generations
            .lock()
            .map_err(|_| "unavailable: Generation registry lock failed")?;
        generations
            .entry(id.into())
            .and_modify(|sender| {
                sender.send_replace(generation.into());
            })
            .or_insert_with(|| watch::channel(generation.into()).0);
        Ok(())
    }
    fn subscribe_generation(
        &self,
        caller: &PlatformCaller,
    ) -> Result<watch::Receiver<String>, String> {
        let mut generations = self
            .generations
            .lock()
            .map_err(|_| "unavailable: Generation registry lock failed")?;
        Ok(generations
            .entry(caller.installation_id.clone())
            .or_insert_with(|| watch::channel(caller.generation.clone()).0)
            .subscribe())
    }
    pub(crate) fn authorize<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        name: &str,
        project: Option<&str>,
    ) -> Result<InstallationRecord, String> {
        crate::agent_control::service::authorize(app, &caller.control)
            .map_err(|e| e.to_string())?;
        let record = self.store.read(&caller.installation_id)?;
        platform_authority::capability(&record, caller, name, project)?;
        Ok(record)
    }
    pub async fn dispatch<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if super::disabled_by_env() {
            return Err("disabled: Isolated plugins are disabled by the environment".into());
        }
        if !params.is_object() || params.to_string().len() > MAX_JSON_BYTES {
            return Err("invalid_input: Host arguments must be a bounded JSON object".into());
        }
        platform_authority::assert_active(&self.store.read(&caller.installation_id)?, caller)?;
        // A stale first caller after restart must not seed the watch registry
        // with revoked authority and prevent the current generation from running.
        let mut generation = self.subscribe_generation(caller)?;
        if *generation.borrow() != caller.generation {
            return Err("stale_generation: Plugin authority was revoked".into());
        }
        let work = async {
            match method {
                "settings.get" | "secrets.get" => {
                    self.setting_get(app, caller, &params, method == "secrets.get")
                }
                "storage.get" | "storage.set" | "storage.delete" => {
                    self.storage(app, caller, method, &params)
                }
                "events.next" | "events.backlog" | "events.ack" => {
                    self.events.dispatch(self, app, caller, method, params)
                }
                _ => super::platform_workflow::dispatch(self, app, caller, method, params).await,
            }
        };
        tokio::select! {
            biased;
            _ = generation.changed() => Err("stale_generation: Plugin authority was revoked during the host call".into()),
            result = work => result,
        }
    }
    fn storage<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        let record = self.authorize(app, caller, "storage", None)?;
        let key = string_field(params, "key", 128)?;
        if method == "storage.get" {
            return Ok(record.storage.get(key).cloned().unwrap_or(Value::Null));
        }
        self.store.update(&caller.installation_id, |record| {
            platform_authority::capability(record, caller, "storage", None)?;
            match method {
                "storage.get" => Ok(record.storage.get(key).cloned().unwrap_or(Value::Null)),
                "storage.delete" => {
                    record.storage.remove(key);
                    Ok(Value::Null)
                }
                _ => {
                    let value = params
                        .get("value")
                        .ok_or("invalid_input: Missing storage value")?;
                    if value.to_string().len() > MAX_JSON_BYTES {
                        return Err("invalid_input: Storage value exceeds 1 MiB".into());
                    }
                    record.storage.insert(key.into(), value.clone());
                    if serde_json::to_vec(&record.storage)
                        .map_err(|e| e.to_string())?
                        .len()
                        > 16 * MAX_JSON_BYTES
                    {
                        return Err("storage_failed: Plugin storage exceeds 16 MiB".into());
                    }
                    Ok(Value::Null)
                }
            }
        })
    }
    fn setting_get<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        params: &Value,
        secret_only: bool,
    ) -> Result<Value, String> {
        let record = self.authorize(app, caller, "settings", None)?;
        let key = string_field(params, "key", 128)?;
        let setting = record
            .manifest
            .settings
            .iter()
            .find(|s| s.key == key)
            .ok_or("invalid_input: Setting is not declared")?;
        if secret_only && setting.kind != "secret" {
            return Err("invalid_input: Secret access requires a declared secret setting".into());
        }
        if setting.kind == "secret" {
            let Some(sealed) = record.secrets.get(key) else {
                return Ok(Value::Null);
            };
            return Ok(Value::String(self.open_secret(
                &record.installation_id,
                key,
                sealed,
            )?));
        }
        Ok(record
            .settings
            .get(key)
            .cloned()
            .or(setting.default.clone())
            .unwrap_or(Value::Null))
    }
    /// A transport must establish settings-management authority before calling this admin method.
    pub fn update_settings(
        &self,
        id: &str,
        values: BTreeMap<String, Value>,
    ) -> Result<InstallationRecord, String> {
        self.store
            .update(id, |record| {
                for (key, value) in values {
                    let setting = record
                        .manifest
                        .settings
                        .iter()
                        .find(|s| s.key == key)
                        .ok_or("invalid_input: Setting is not declared")?;
                    if value.is_null() {
                        record.settings.remove(&key);
                        record.secrets.remove(&key);
                        continue;
                    }
                    match setting.kind.as_str() {
                        "secret" => {
                            let value =
                                value.as_str().ok_or("invalid_input: Secret must be text")?;
                            if value.is_empty() {
                                record.secrets.remove(&key);
                            } else {
                                record
                                    .secrets
                                    .insert(key.clone(), self.seal_secret(id, &key, value)?);
                            }
                        }
                        "string" if value.as_str().is_some_and(|v| v.len() <= 16384) => {
                            record.settings.insert(key, value);
                        }
                        "number" if value.is_number() => {
                            record.settings.insert(key, value);
                        }
                        "boolean" if value.is_boolean() => {
                            record.settings.insert(key, value);
                        }
                        _ => {
                            return Err(
                                "invalid_input: Setting does not match its declared type".into()
                            )
                        }
                    }
                }
                record.generation = uuid::Uuid::new_v4().to_string();
                Ok(record.clone())
            })
            .and_then(|record| {
                self.notify_generation(id, &record.generation)?;
                Ok(record)
            })
    }
    pub fn public_settings(record: &InstallationRecord) -> Value {
        Value::Object(
            record
                .manifest
                .settings
                .iter()
                .map(|setting| {
                    let value = if setting.kind == "secret" {
                        json!({"hasSecret":record.secrets.contains_key(&setting.key)})
                    } else {
                        record
                            .settings
                            .get(&setting.key)
                            .cloned()
                            .or(setting.default.clone())
                            .unwrap_or(Value::Null)
                    };
                    (setting.key.clone(), value)
                })
                .collect(),
        )
    }
    fn master_key(&self, create: bool) -> Result<[u8; 32], String> {
        use std::io::{Read, Write};
        super::platform_store::private_directory(self.store.root())?;
        let path = self.store.root().join("secrets.key");
        let read = || -> Result<[u8; 32], String> {
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|_| "storage_failed: Secret key is unavailable")?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != 32 {
                return Err("storage_failed: Invalid secret key retained".into());
            }
            let mut bytes = [0; 32];
            std::fs::File::open(&path)
                .and_then(|mut file| file.read_exact(&mut bytes))
                .map_err(|_| "storage_failed: Secret key is unreadable")?;
            Ok(bytes)
        };
        if path.exists() {
            return read();
        }
        if !create {
            return Err(
                "storage_failed: Secret key is missing; retained secrets cannot be read".into(),
            );
        }
        // Settings updates already hold this store's write gate. Atomic record
        // files allow a separate reader here without recursively locking it.
        if super::platform_store::PlatformStore::new(self.store.root().to_path_buf())
            .list()
            .into_iter()
            .any(|(_, record)| record.map_or(true, |record| !record.secrets.is_empty()))
        {
            return Err("storage_failed: Missing key for retained plugin secrets; restore the original key before writing secrets".into());
        }
        let mut key = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(&key)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| "storage_failed: Could not save secret key")?;
                Ok(key)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(),
            Err(_) => Err("storage_failed: Secret key unavailable".into()),
        }
    }
    fn seal_secret(&self, id: &str, key: &str, value: &str) -> Result<Vec<u8>, String> {
        if value.len() > 16384 {
            return Err("invalid_input: Secret exceeds 16 KiB".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.master_key(true)?)
            .map_err(|_| "Secret encryption unavailable")?;
        let mut nonce = [0; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let aad = format!("{id}\0{key}");
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: value.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "Could not seal plugin secret")?;
        let mut out = vec![1];
        out.extend(nonce);
        out.extend(encrypted);
        Ok(out)
    }
    fn open_secret(&self, id: &str, key: &str, value: &[u8]) -> Result<String, String> {
        if value.len() < 30 || value[0] != 1 {
            return Err("storage_failed: Secret format is unavailable".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.master_key(false)?)
            .map_err(|_| "Secret encryption unavailable")?;
        let aad = format!("{id}\0{key}");
        let plain = cipher
            .decrypt(
                Nonce::from_slice(&value[1..13]),
                Payload {
                    msg: &value[13..],
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "storage_failed: Secret cannot be decrypted")?;
        String::from_utf8(plain).map_err(|_| "storage_failed: Secret cannot be decoded".into())
    }
}
pub(crate) fn string_field<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
        .ok_or_else(|| format!("invalid_input: {key} must be nonempty text (max {max} bytes)"))
}

#[cfg(test)]
#[path = "platform_tests.rs"]
mod tests;
