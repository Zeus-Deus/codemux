//! Secrets are host-only. No value from this module is serialized into plugin IPC.
use super::{ErrorCode, ProtocolError, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct Credentials {
    backend: Arc<dyn CredentialStore>,
    session: Arc<Mutex<HashMap<(String, String), String>>>,
    serial: Arc<Mutex<()>>,
    configured: Arc<Mutex<HashSet<(String, String)>>>,
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
            configured: Arc::new(Mutex::new(entries.into_iter().collect())),
            ..Self::default()
        }
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
            self.session.lock().await.insert(key, secret);
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
            session.blocking_lock().remove(&key);
            configured.blocking_lock().insert(key);
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
            .await
            .get(&(installation.into(), id.into()))
        {
            return Ok(Some(value.clone()));
        }
        if !self
            .configured
            .lock()
            .await
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
            .await
            .retain(|(owner, _), _| owner != installation);
    }
    pub async fn delete(&self, installation: &str, id: &str) -> Result<()> {
        let lock = self.serial.clone().lock_owned().await;
        self.session
            .lock()
            .await
            .remove(&(installation.into(), id.into()));
        self.configured
            .lock()
            .await
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct RecordedStore {
        locked: std::sync::atomic::AtomicBool,
        values: std::sync::Mutex<HashMap<(String, String), String>>,
    }
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
