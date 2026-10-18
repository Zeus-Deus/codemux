//! Private secrets the user enters once, such as a webhook signing secret.
//!
//! Values are sealed with the same machine-guarded AES-256-GCM as the auth
//! token (`auth::encrypt_data`) and stored in the `secrets` table. Nothing
//! returns a value to the UI, an agent, or a log: callers get a
//! [`SecretMeta`] and refer to the secret by its id. Only backend code that
//! needs the value, such as webhook signature checks, calls [`reveal`].

use crate::database::automation_triggers::SecretMeta;
use crate::database::DatabaseStore;

const MAX_LABEL_LEN: usize = 200;
const MAX_VALUE_LEN: usize = 16 * 1024;

/// Encrypt and store a secret. Surrounding whitespace is dropped, as a
/// pasted token often carries a trailing newline.
pub fn store(db: &DatabaseStore, label: &str, value: &str) -> Result<SecretMeta, String> {
    let label = label.trim();
    let value = value.trim();
    if label.is_empty() {
        return Err("A secret needs a name".into());
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(format!("Secret name is too long (max {MAX_LABEL_LEN} chars)"));
    }
    if value.is_empty() {
        return Err("The secret is empty".into());
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(format!("The secret is too long (max {MAX_VALUE_LEN} bytes)"));
    }
    let sealed = crate::auth::encrypt_data(value.as_bytes())?;
    db.insert_secret(label, &sealed)
}

/// Decrypt a stored secret for backend use. The error never contains the
/// value.
pub(crate) fn reveal(db: &DatabaseStore, id: &str) -> Result<String, String> {
    let sealed = db
        .load_secret_ciphertext(id)
        .ok_or_else(|| "The secret no longer exists".to_string())?;
    let plain = crate::auth::decrypt_data(&sealed)
        .map_err(|_| "The secret could not be decrypted on this machine".to_string())?;
    String::from_utf8(plain).map_err(|_| "The stored secret is not valid text".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_encrypted_and_reveals_the_trimmed_value() {
        let db = crate::database::init_test_database();
        let meta = store(&db, " GitHub ", "  s3cret-value\n").unwrap();
        assert_eq!(meta.label, "GitHub");
        assert!(meta.id.starts_with("secret_"));

        let sealed = db.load_secret_ciphertext(&meta.id).unwrap();
        assert!(!sealed
            .windows(b"s3cret-value".len())
            .any(|window| window == b"s3cret-value"));
        assert_eq!(reveal(&db, &meta.id).unwrap(), "s3cret-value");
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
