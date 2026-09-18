//! Secrets are host-only. No value from this module is serialized into plugin IPC.
use super::{ErrorCode, ProtocolError, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;
#[derive(Default, Clone)]
pub struct Credentials {
    session: Arc<Mutex<HashMap<(String, String), String>>>,
    serial: Arc<Mutex<()>>,
    configured: Arc<Mutex<HashSet<(String, String)>>>,
}
fn unavailable() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::CredentialRequired,
        "Credential store unavailable or locked; choose session-only storage explicitly",
    )
}
impl Credentials {
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
        tokio::task::spawn_blocking(move || {
            // The OS operation cannot be cancelled. Keep serialization and the
            // corresponding memory update inside this task even if IPC closes.
            let _lock = lock;
            keyring::Entry::new(&service, &id)
                .and_then(|entry| entry.set_password(&secret))
                .map_err(|_| unavailable())?;
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
        match tokio::task::spawn_blocking(move || {
            let _lock = lock;
            keyring::Entry::new(&service, &id).and_then(|entry| entry.get_password())
        })
        .await
        .map_err(|_| unavailable())?
        {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(unavailable()),
        }
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
        match tokio::task::spawn_blocking(move || {
            let _lock = lock;
            keyring::Entry::new(&service, &id).and_then(|entry| entry.delete_credential())
        })
        .await
        .map_err(|_| unavailable())?
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
