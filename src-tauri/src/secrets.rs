//! Private secrets the user enters once, such as a webhook signing secret.
//!
//! Values are sealed with AES-256-GCM under a random 256-bit master key and
//! stored in the `secrets` table. The key lives in its own owner-only file
//! (`secrets.key` in the app data directory), apart from the database in
//! the config directory, so a copy of the database alone reveals nothing.
//! Losing the key makes stored secrets unreadable; signature checks then
//! fail closed until the user enters the secret again.
//!
//! Nothing returns a value to the UI, an agent, or a log: callers get a
//! [`SecretMeta`] and refer to the secret by its id. Only backend code that
//! needs the value, such as webhook signature checks, calls [`reveal`].

use std::path::Path;
#[cfg(not(test))]
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

use crate::database::automation_triggers::SecretMeta;
use crate::database::DatabaseStore;

const MAX_LABEL_LEN: usize = 200;
const MAX_VALUE_LEN: usize = 16 * 1024;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// First byte of every sealed value, so the format can change later.
const FORMAT_V1: u8 = 1;

/// Encrypt and store a secret. The value is kept byte for byte: a signing
/// secret with surrounding whitespace is still that secret.
pub fn store(db: &DatabaseStore, label: &str, value: &str) -> Result<SecretMeta, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("A secret needs a name".into());
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(format!("Secret name is too long (max {MAX_LABEL_LEN} chars)"));
    }
    if value.trim().is_empty() {
        return Err("The secret is empty".into());
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(format!("The secret is too long (max {MAX_VALUE_LEN} bytes)"));
    }
    let id = crate::database::automation_triggers::new_secret_id();
    let sealed = seal(&master_key()?, &id, value.as_bytes())?;
    db.insert_secret(&id, label, &sealed)
}

/// Decrypt a stored secret for backend use. The error never contains the
/// value.
pub(crate) fn reveal(db: &DatabaseStore, id: &str) -> Result<String, String> {
    let sealed = db
        .load_secret_ciphertext(id)
        .ok_or_else(|| "The secret no longer exists".to_string())?;
    let plain = open(&master_key()?, id, &sealed)?;
    String::from_utf8(plain).map_err(|_| "The stored secret is not valid text".to_string())
}

/// Seal `plain` under `key`. The secret's id is bound in as associated
/// data, so one row's ciphertext cannot be swapped into another.
fn seal(key: &[u8; KEY_LEN], id: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "Secret cipher unavailable")?;
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: id.as_bytes() })
        .map_err(|_| "Could not encrypt the secret")?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
    out.push(FORMAT_V1);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn open(key: &[u8; KEY_LEN], id: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
    const UNREADABLE: &str = "The secret could not be decrypted on this machine";
    if sealed.len() <= 1 + NONCE_LEN || sealed[0] != FORMAT_V1 {
        return Err(UNREADABLE.into());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| UNREADABLE)?;
    let (nonce, ciphertext) = sealed[1..].split_at(NONCE_LEN);
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: id.as_bytes() })
        .map_err(|_| UNREADABLE.into())
}

#[cfg(not(test))]
fn key_path() -> Result<PathBuf, String> {
    let dir = dirs::data_dir().ok_or("Could not determine the data directory")?;
    Ok(dir.join(crate::APP_DIR_NAME).join("secrets.key"))
}

#[cfg(not(test))]
fn master_key() -> Result<[u8; KEY_LEN], String> {
    load_or_create_key(&key_path()?)
}

/// Tests never touch the real key file; each test process gets its own key.
#[cfg(test)]
fn master_key() -> Result<[u8; KEY_LEN], String> {
    static KEY: std::sync::LazyLock<[u8; KEY_LEN]> = std::sync::LazyLock::new(|| {
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        key
    });
    Ok(*KEY)
}

/// Read the master key, creating it (owner read/write only) on first use.
///
/// A new key is written and synced to a temporary file, then published
/// with a no-overwrite link, so readers never see a partial key and a
/// concurrent first use settles on whichever key landed first.
fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], String> {
    use std::io::{ErrorKind, Write};

    if !path.exists() {
        let dir = path.parent().ok_or("The secrets key has no directory")?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Could not create the secrets directory: {e}"))?;
        // `tempfile` creates the file owner-only (0600 on Unix).
        let mut temp = tempfile::NamedTempFile::new_in(dir)
            .map_err(|e| format!("Could not create the secrets key: {e}"))?;
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        temp.write_all(&key)
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|e| format!("Could not write the secrets key: {e}"))?;
        match temp.persist_noclobber(path) {
            Ok(_) => return Ok(key),
            // Another caller published first; use its key.
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Could not save the secrets key: {}", error.error)),
        }
    }

    let bytes = std::fs::read(path).map_err(|e| format!("Could not read the secrets key: {e}"))?;
    #[cfg(unix)]
    {
        // Tighten a key file whose mode was widened since creation.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    bytes
        .try_into()
        .map_err(|_| "The secrets key file is damaged".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_encrypted_and_reveals_the_exact_value() {
        let db = crate::database::init_test_database();
        let meta = store(&db, " GitHub ", "  s3cret-value\n").unwrap();
        assert_eq!(meta.label, "GitHub");
        assert!(meta.id.starts_with("secret_"));

        let sealed = db.load_secret_ciphertext(&meta.id).unwrap();
        assert!(!sealed
            .windows(b"s3cret-value".len())
            .any(|window| window == b"s3cret-value"));
        // Whitespace is part of a signing secret; it survives.
        assert_eq!(reveal(&db, &meta.id).unwrap(), "  s3cret-value\n");
    }

    #[test]
    fn ciphertext_is_bound_to_its_key_and_id() {
        let key = [7u8; KEY_LEN];
        let sealed = seal(&key, "secret_a", b"value").unwrap();
        assert_eq!(open(&key, "secret_a", &sealed).unwrap(), b"value");
        assert!(open(&[8u8; KEY_LEN], "secret_a", &sealed).is_err());
        assert!(open(&key, "secret_b", &sealed).is_err());
        assert!(open(&key, "secret_a", &sealed[..5]).is_err());
    }

    #[test]
    fn key_file_is_created_once_owner_only_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("secrets.key");
        let first = load_or_create_key(&path).unwrap();
        assert_eq!(load_or_create_key(&path).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, b"short").unwrap();
        assert!(load_or_create_key(&path).is_err());
    }

    #[test]
    fn concurrent_first_use_agrees_on_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.key");
        let keys: Vec<[u8; KEY_LEN]> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| load_or_create_key(&path).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(keys.iter().all(|key| *key == keys[0]));
        // No temporary files are left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn listing_never_includes_the_value() {
        let db = crate::database::init_test_database();
        store(&db, "Token", "very-private").unwrap();
        let listed = serde_json::to_string(&db.list_secrets()).unwrap();
        assert!(listed.contains("Token"));
        assert!(!listed.contains("very-private"));
    }

    #[test]
    fn rejects_empty_and_oversized_input() {
        let db = crate::database::init_test_database();
        assert!(store(&db, "", "x").is_err());
        assert!(store(&db, "name", "   ").is_err());
        assert!(store(&db, "name", &"x".repeat(MAX_VALUE_LEN + 1)).is_err());
        assert!(db.list_secrets().is_empty());
    }

    #[test]
    fn reveal_of_a_missing_secret_fails() {
        let db = crate::database::init_test_database();
        assert!(reveal(&db, "secret_missing").is_err());
    }

    #[test]
    fn delete_removes_an_unused_secret() {
        let db = crate::database::init_test_database();
        let meta = store(&db, "Old", "value").unwrap();
        db.delete_secret(&meta.id).unwrap();
        assert!(db.list_secrets().is_empty());
        assert!(db.delete_secret(&meta.id).is_err());
    }
}
