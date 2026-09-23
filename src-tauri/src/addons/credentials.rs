//! Secrets are host-only. No value from this module is serialized into plugin IPC.
use super::{
    manager::{Installation, Manager, UiEvent},
    ErrorCode, ProtocolError, Result,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct Credentials {
    backend: Arc<dyn CredentialStore>,
    // Held only briefly and never across an await, so Settings can read the
    // state synchronously without touching the OS store or the secret.
    session: Arc<StdMutex<HashMap<(String, String), String>>>,
    serial: Arc<Mutex<()>>,
    configured: Arc<StdMutex<HashSet<(String, String)>>>,
}
/// What Settings can show about a declared credential, without its value.
/// A credential that was never configured leaves requests unauthenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialState {
    NotConfigured,
    Saved,
    SessionOnly,
    CleanupPending,
}
trait CredentialStore: Send + Sync {
    fn set(&self, service: &str, id: &str, value: &str) -> Result<()>;
    fn get(&self, service: &str, id: &str) -> Result<Option<String>>;
    fn delete(&self, service: &str, id: &str) -> Result<()>;
}
struct OsCredentialStore;
impl CredentialStore for OsCredentialStore {
    fn set(&self, service: &str, id: &str, value: &str) -> Result<()> {
        keyring::Entry::new(service, id)
            .and_then(|e| e.set_password(value))
            .map_err(|_| unavailable())
    }
    fn get(&self, service: &str, id: &str) -> Result<Option<String>> {
        match keyring::Entry::new(service, id).and_then(|e| e.get_password()) {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }
    fn delete(&self, service: &str, id: &str) -> Result<()> {
        match keyring::Entry::new(service, id).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}
impl Default for Credentials {
    fn default() -> Self {
        Self {
            backend: Arc::new(OsCredentialStore),
            session: Default::default(),
            serial: Default::default(),
            configured: Default::default(),
        }
    }
}
fn unavailable() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::CredentialRequired,
        "Credential store unavailable or locked; choose session-only storage explicitly",
    )
}
impl Credentials {
    /// An update cannot reuse a named bearer token at a different origin.
    /// Old origin keys remain indexed for rollback and complete uninstall cleanup.
    pub fn key(id: &str, origin: &str) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(id, origin)).unwrap())
        )
    }
    pub fn with_configured(entries: Vec<(String, String)>) -> Self {
        Self {
            configured: Arc::new(StdMutex::new(entries.into_iter().collect())),
            ..Self::default()
        }
    }
    fn stored(&self, installation: &str, id: &str) -> Option<CredentialState> {
        let key = (installation.to_owned(), id.to_owned());
        if self.session.lock().unwrap().contains_key(&key) {
            Some(CredentialState::SessionOnly)
        } else if self.configured.lock().unwrap().contains(&key) {
            Some(CredentialState::Saved)
        } else {
            None
        }
    }
    fn forget_session(&self, installation: &str, id: &str) {
        self.session
            .lock()
            .unwrap()
            .remove(&(installation.into(), id.into()));
    }
    pub async fn set(
        &self,
        installation: &str,
        id: &str,
        secret: String,
        session_only: bool,
    ) -> Result<()> {
        if secret.is_empty() || secret.len() > 8192 || secret.chars().any(char::is_control) {
            return Err(ProtocolError::invalid("Invalid bearer credential"));
        }
        let lock = self.serial.clone().lock_owned().await;
        let key = (installation.into(), id.into());
        if session_only {
            self.session.lock().unwrap().insert(key, secret);
            return Ok(());
        }
        let service = format!("{}.addons.{}", crate::APP_DIR_NAME, installation);
        let id = id.to_owned();
        let session = self.session.clone();
        let configured = self.configured.clone();
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            // The OS operation cannot be cancelled. Keep serialization and the
            // corresponding memory update inside this task even if IPC closes.
            let _lock = lock;
            backend.set(&service, &id, &secret)?;
            session.lock().unwrap().remove(&key);
            configured.lock().unwrap().insert(key);
            Ok(())
        })
        .await
        .map_err(|_| unavailable())?
    }
    pub async fn get(&self, installation: &str, id: &str) -> Result<Option<String>> {
        let lock = self.serial.clone().lock_owned().await;
        if let Some(value) = self
            .session
            .lock()
            .unwrap()
            .get(&(installation.into(), id.into()))
        {
            return Ok(Some(value.clone()));
        }
        // Never configured: the request stays unauthenticated. A saved value
        // that the store cannot read fails with CREDENTIAL_REQUIRED below.
        if !self
            .configured
            .lock()
            .unwrap()
            .contains(&(installation.into(), id.into()))
        {
            return Ok(None);
        }
        let service = format!("{}.addons.{}", crate::APP_DIR_NAME, installation);
        let id = id.to_owned();
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            backend.get(&service, &id)
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub(super) async fn clear_session(&self, installation: &str) {
        let _lock = self.serial.lock().await;
        self.session
            .lock()
            .unwrap()
            .retain(|(owner, _), _| owner != installation);
    }
    pub async fn delete(&self, installation: &str, id: &str) -> Result<()> {
        let lock = self.serial.clone().lock_owned().await;
        self.session
            .lock()
            .unwrap()
            .remove(&(installation.into(), id.into()));
        self.configured
            .lock()
            .unwrap()
            .remove(&(installation.into(), id.into()));
        let service = format!("{}.addons.{}", crate::APP_DIR_NAME, installation);
        let id = id.to_owned();
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            backend.delete(&service, &id)
        })
        .await
        .map_err(|_| unavailable())?
    }
    #[cfg(test)]
    pub(super) fn recorded(backend: Arc<RecordedStore>) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }
}
fn registry_unavailable(_: rusqlite::Error) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on registry is unavailable",
    )
}
fn declared_key(installation: &Installation, credential: &str) -> Result<String> {
    installation
        .manifest
        .credentials
        .iter()
        .find(|c| c.id == credential)
        .map(|c| Credentials::key(&c.id, &c.origin))
        .ok_or_else(|| ProtocolError::new(ErrorCode::PermissionDenied, "Credential was not declared"))
}
impl Manager {
    /// Per declared credential ID. Reads only host indexes, never a secret.
    pub fn credential_states(
        &self,
        installation: &Installation,
    ) -> Result<BTreeMap<String, CredentialState>> {
        let db = self.registry.lock().unwrap();
        let mut states = BTreeMap::new();
        for declaration in &installation.manifest.credentials {
            let key = Credentials::key(&declaration.id, &declaration.origin);
            let state = match self.credentials.stored(&installation.installation_id, &key) {
                Some(state) => state,
                None if db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM cleanup WHERE installation=?1 AND credential=?2)",
                        rusqlite::params![installation.installation_id, key],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(registry_unavailable)? =>
                {
                    CredentialState::CleanupPending
                }
                None => CredentialState::NotConfigured,
            };
            states.insert(declaration.id.clone(), state);
        }
        Ok(states)
    }
    /// The caller holds the installation's operation lock.
    pub async fn save_credential(
        &self,
        installation: &Installation,
        credential: &str,
        value: String,
        session_only: bool,
    ) -> Result<()> {
        let key = declared_key(installation, credential)?;
        // Persist the host-owned index before the non-cancellable OS write. If
        // this IPC task is dropped, uninstall/restart can still find the credential.
        if !session_only {
            self.record_credential(&installation.installation_id, &key)?;
        }
        self.credentials
            .set(&installation.installation_id, &key, value, session_only)
            .await?;
        if !session_only {
            // The new value replaced any earlier one awaiting removal. A kept
            // tombstone would delete the credential the user just saved.
            self.registry
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM cleanup WHERE installation=?1 AND credential=?2",
                    rusqlite::params![installation.installation_id, key],
                )
                .map_err(registry_unavailable)?;
        }
        let _ = self.events.send(UiEvent::Inventory);
        Ok(())
    }
    /// Removes a saved or session-only credential; later requests to its
    /// origin are unauthenticated. The caller holds the operation lock. A
    /// locked OS store keeps a retryable tombstone, as uninstall does.
    pub async fn clear_credential(
        &self,
        installation: &Installation,
        credential: &str,
    ) -> Result<Vec<String>> {
        let key = declared_key(installation, credential)?;
        let stored = {
            let mut db = self.registry.lock().unwrap();
            let tx = db.transaction().map_err(registry_unavailable)?;
            tx.execute("INSERT OR IGNORE INTO cleanup(installation,credential) SELECT installation,id FROM credential_entries WHERE installation=?1 AND id=?2",rusqlite::params![installation.installation_id,key]).map_err(registry_unavailable)?;
            tx.execute(
                "DELETE FROM credential_entries WHERE installation=?1 AND id=?2",
                rusqlite::params![installation.installation_id, key],
            )
            .map_err(registry_unavailable)?;
            let stored = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM cleanup WHERE installation=?1 AND credential=?2)",
                    rusqlite::params![installation.installation_id, key],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(registry_unavailable)?;
            tx.commit().map_err(registry_unavailable)?;
            stored
        };
        if !stored {
            // Session-only values never reached the OS credential store.
            self.credentials
                .forget_session(&installation.installation_id, &key);
        } else if self
            .credentials
            .delete(&installation.installation_id, &key)
            .await
            .is_ok()
        {
            self.registry
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM cleanup WHERE installation=?1 AND credential=?2",
                    rusqlite::params![installation.installation_id, key],
                )
                .map_err(registry_unavailable)?;
        }
        let _ = self.events.send(UiEvent::Inventory);
        self.cleanup_warnings()
    }
}
#[cfg(test)]
#[derive(Default)]
pub(super) struct RecordedStore {
    pub(super) locked: std::sync::atomic::AtomicBool,
    pub(super) values: std::sync::Mutex<HashMap<(String, String), String>>,
}
#[cfg(test)]
impl CredentialStore for RecordedStore {
    fn set(&self, service: &str, id: &str, value: &str) -> Result<()> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable());
        }
        self.values
            .lock()
            .unwrap()
            .insert((service.into(), id.into()), value.into());
        Ok(())
    }
    fn get(&self, service: &str, id: &str) -> Result<Option<String>> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable());
        }
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&(service.into(), id.into()))
            .cloned())
    }
    fn delete(&self, service: &str, id: &str) -> Result<()> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(unavailable());
        }
        self.values
            .lock()
            .unwrap()
            .remove(&(service.into(), id.into()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn locked_store_requires_explicit_session_fallback_and_removed_keys_stay_inaccessible() {
        let backend = Arc::new(RecordedStore::default());
        let key = Credentials::key("token", "https://api.example.com");
        let credentials = Credentials {
            backend: backend.clone(),
            ..Credentials::default()
        };
        credentials
            .set("installation", &key, "synthetic-persisted".into(), false)
            .await
            .unwrap();
        backend
            .locked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(credentials.get("installation", &key).await.is_err());
        assert!(credentials
            .set("installation", &key, "must-not-save".into(), false)
            .await
            .is_err());
        credentials
            .set("installation", &key, "explicit-session".into(), true)
            .await
            .unwrap();
        assert_eq!(
            credentials
                .get("installation", &key)
                .await
                .unwrap()
                .as_deref(),
            Some("explicit-session")
        );
        assert!(credentials.delete("installation", &key).await.is_err());
        assert_eq!(credentials.get("installation", &key).await.unwrap(), None);
        backend
            .locked
            .store(false, std::sync::atomic::Ordering::SeqCst);
        credentials.delete("installation", &key).await.unwrap();
        assert!(backend.values.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn unused_and_session_only_credentials_remove_without_an_os_service() {
        use super::super::{
            lifecycle::Reviews, manager::Manager, package::fixture_archive, Manifest,
        };
        for session_only in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
            let backend = Arc::new(RecordedStore::default());
            backend
                .locked
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Arc::get_mut(&mut manager).unwrap().credentials = Credentials {
                backend,
                ..Credentials::default()
            };
            let package = root.path().join("fixture.cmxaddon");
            std::fs::write(&package, fixture_archive()).unwrap();
            let reviews = Reviews::default();
            let review = reviews.prepare_local(&manager, &package).unwrap();
            let mut installed = reviews
                .accept(&manager, &review.token, false, false)
                .await
                .unwrap();
            // Declaring an optional credential does not mean it was ever saved.
            let declarations = Manifest::parse(
                include_bytes!("../../../examples/addons/issue-companion/manifest.json"),
                None,
            )
            .unwrap();
            installed.manifest.credentials = declarations.credentials;
            installed.manifest.http = declarations.http;
            manager.save(&installed).unwrap();
            let field = &installed.manifest.credentials[0];
            let key = Credentials::key(&field.id, &field.origin);
            if session_only {
                manager
                    .credentials
                    .set(
                        &installed.installation_id,
                        &key,
                        "synthetic-session".into(),
                        true,
                    )
                    .await
                    .unwrap();
            }
            assert!(manager
                .remove(&installed.manifest.id, false)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(
                manager
                    .credentials
                    .get(&installed.installation_id, &key)
                    .await
                    .unwrap(),
                None
            );
            assert!(manager.cleanup_warnings().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn settings_see_each_credential_state_and_clear_removes_saved_values() {
        use super::super::{lifecycle::Reviews, package::fixture_archive, Manifest};
        use std::sync::atomic::Ordering::SeqCst;
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        let backend = Arc::new(RecordedStore::default());
        Arc::get_mut(&mut manager).unwrap().credentials = Credentials::recorded(backend.clone());
        let package = root.path().join("fixture.cmxaddon");
        std::fs::write(&package, fixture_archive()).unwrap();
        let reviews = Reviews::default();
        let review = reviews.prepare_local(&manager, &package).unwrap();
        let mut installed = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let declarations = Manifest::parse(
            include_bytes!("../../../examples/addons/issue-companion/manifest.json"),
            None,
        )
        .unwrap();
        installed.manifest.credentials = declarations.credentials;
        installed.manifest.http = declarations.http;
        manager.save(&installed).unwrap();
        let field = installed.manifest.credentials[0].clone();
        let key = Credentials::key(&field.id, &field.origin);
        let state = |manager: &Manager| manager.credential_states(&installed).unwrap()[&field.id];
        assert_eq!(state(&manager), CredentialState::NotConfigured);
        // Session-only values never need the OS store, even to clear them.
        backend.locked.store(true, SeqCst);
        manager
            .save_credential(&installed, &field.id, "synthetic-session".into(), true)
            .await
            .unwrap();
        assert_eq!(state(&manager), CredentialState::SessionOnly);
        assert!(manager
            .clear_credential(&installed, &field.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(state(&manager), CredentialState::NotConfigured);
        backend.locked.store(false, SeqCst);
        manager
            .save_credential(&installed, &field.id, "synthetic-saved".into(), false)
            .await
            .unwrap();
        assert_eq!(state(&manager), CredentialState::Saved);
        // A locked store keeps a retryable tombstone; the value is unusable now.
        backend.locked.store(true, SeqCst);
        assert!(!manager
            .clear_credential(&installed, &field.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(state(&manager), CredentialState::CleanupPending);
        assert_eq!(
            manager
                .credentials
                .get(&installed.installation_id, &key)
                .await
                .unwrap(),
            None
        );
        // Saving again replaces the pending value; cleanup must not remove it.
        backend.locked.store(false, SeqCst);
        manager
            .save_credential(&installed, &field.id, "synthetic-replacement".into(), false)
            .await
            .unwrap();
        assert!(manager.retry_cleanup().await.unwrap().is_empty());
        assert_eq!(
            manager
                .credentials
                .get(&installed.installation_id, &key)
                .await
                .unwrap()
                .as_deref(),
            Some("synthetic-replacement")
        );
        drop(manager);
        let mut manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        let configured = manager.credentials.configured.clone();
        Arc::get_mut(&mut manager).unwrap().credentials = Credentials {
            backend: backend.clone(),
            configured,
            ..Credentials::default()
        };
        assert_eq!(state(&manager), CredentialState::Saved);
        assert!(manager
            .clear_credential(&installed, &field.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(state(&manager), CredentialState::NotConfigured);
        assert!(backend.values.lock().unwrap().is_empty());
        assert_eq!(
            manager
                .clear_credential(&installed, "undeclared")
                .await
                .unwrap_err()
                .data
                .code,
            ErrorCode::PermissionDenied
        );
    }
    #[tokio::test]
    async fn uninstall_retries_credentials_removed_from_later_manifests_even_when_data_is_kept() {
        use super::super::{lifecycle::Reviews, manager::Manager, package::fixture_archive};
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        let backend = Arc::new(RecordedStore::default());
        Arc::get_mut(&mut manager).unwrap().credentials = Credentials {
            backend: backend.clone(),
            ..Credentials::default()
        };
        let path = root.path().join("fixture.cmxaddon");
        std::fs::write(&path, fixture_archive()).unwrap();
        let reviews = Reviews::default();
        let review = reviews.prepare_local(&manager, &path).unwrap();
        let installed = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let retired = Credentials::key("retired-token", "https://old.example.com");
        manager
            .record_credential(&installed.installation_id, &retired)
            .unwrap();
        manager
            .credentials
            .set(
                &installed.installation_id,
                &retired,
                "synthetic-retired".into(),
                false,
            )
            .await
            .unwrap();
        backend
            .locked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!manager
            .remove(&installed.manifest.id, true)
            .await
            .unwrap()
            .is_empty());
        assert!(manager.list().unwrap().is_empty());
        assert_eq!(
            manager
                .credentials
                .get(&installed.installation_id, &retired)
                .await
                .unwrap(),
            None
        );
        drop(manager);
        let mut reopened = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        assert!(!reopened.cleanup_warnings().unwrap().is_empty());
        Arc::get_mut(&mut reopened).unwrap().credentials = Credentials {
            backend: backend.clone(),
            ..Credentials::default()
        };
        backend
            .locked
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(reopened.retry_cleanup().await.unwrap().is_empty());
        assert!(backend.values.lock().unwrap().is_empty());
    }
    #[tokio::test]
    #[ignore = "Requires the actual unlocked OS credential service; writes only a random test namespace"]
    async fn native_os_backend_roundtrip_and_deletion() {
        let installation = uuid::Uuid::new_v4().to_string();
        let key = Credentials::key("test-token", "https://fixture.example.com");
        let credentials = Credentials::default();
        credentials
            .set(
                &installation,
                &key,
                "synthetic-native-test-token".into(),
                false,
            )
            .await
            .unwrap();
        let read = credentials.get(&installation, &key).await;
        // Always attempt deletion before asserting the round-trip result.
        let deleted = credentials.delete(&installation, &key).await;
        assert_eq!(
            read.unwrap().as_deref(),
            Some("synthetic-native-test-token")
        );
        deleted.unwrap();
        let reopened = Credentials::with_configured(vec![(installation.clone(), key.clone())]);
        assert_eq!(reopened.get(&installation, &key).await.unwrap(), None);
    }
    #[tokio::test]
    async fn named_credentials_do_not_transfer_to_another_origin_or_installation() {
        let credentials = Credentials::default();
        let first = Credentials::key("token", "https://first.example");
        let second = Credentials::key("token", "https://second.example");
        credentials
            .set("installation", &first, "synthetic-token".into(), true)
            .await
            .unwrap();
        assert_eq!(
            credentials
                .get("installation", &first)
                .await
                .unwrap()
                .as_deref(),
            Some("synthetic-token")
        );
        assert_eq!(
            credentials.get("installation", &second).await.unwrap(),
            None
        );
        assert_eq!(credentials.get("replacement", &first).await.unwrap(), None);
    }
    #[tokio::test]
    async fn optional_unset_credentials_do_not_require_an_os_service() {
        let credentials = Credentials::default();
        assert_eq!(
            credentials
                .get("isolated-installation", "token")
                .await
                .unwrap(),
            None
        );
        credentials
            .set("first", "token", "synthetic-not-a-real-secret".into(), true)
            .await
            .unwrap();
        assert_eq!(
            credentials.get("first", "token").await.unwrap().as_deref(),
            Some("synthetic-not-a-real-secret")
        );
        assert_eq!(credentials.get("second", "token").await.unwrap(), None);
        assert_eq!(
            Credentials::default().get("first", "token").await.unwrap(),
            None
        );
    }
}
