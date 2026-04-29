// Client-side end-to-end encryption for synced skills (Step 10).
// Mirrors Vexis's `src-tauri/src/encryption/mod.rs` byte-for-byte at
// the protocol level — same primitives, same nonce length, same
// auth-tag placement — so a blob encrypted by one product can be
// decrypted by the other given the same `(password, email)` pair
// going through the shared `codemux-api-*` derivation.
//
// Primitives:
//   - AEAD:   XChaCha20-Poly1305 (24-byte random nonce, 16-byte tag)
//   - Key:    32-byte raw key (output of HKDF-Expand with the
//             `codemux-api-encryption-key-v1` info label, see
//             `crate::auth::derivation::derive_login_credentials`)
//
// The server stores `{ciphertext, nonce}` pairs as opaque base64
// TEXT (matching the `voice_*` wire format). It never holds the key,
// never decrypts, and has no way to read what's inside a skill.
//
// Stage 2 will wire this into the `/api/skills` HTTP client and the
// settings UI; Stage 1 ships it as a self-contained module covered
// by unit tests + the smoke-test CLI in `tools/`.

use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Length of the XChaCha20-Poly1305 nonce in bytes. The 192-bit
/// nonce makes random-nonce collisions astronomically unlikely
/// (unlike ChaCha20's 96-bit nonce, where collision risk matters
/// after 2^32 messages with the same key).
pub const NONCE_LEN: usize = 24;

/// Length of the symmetric key used by both Argon2id (output) and
/// XChaCha20-Poly1305 (key).
pub const KEY_LEN: usize = 32;

/// An encrypted blob. The nonce is stored separately from the
/// ciphertext so callers can round-trip through JSON (base64 the
/// bytes) or SQLite (two BLOB columns). The Poly1305 auth tag is
/// part of `ciphertext`.
///
/// Serializes to camelCase JSON because this type crosses the Tauri
/// IPC boundary and the HTTP boundary to `/api/skills`. The wire
/// shape matches Vexis's `EncryptedData` byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedData {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// Encrypt `plaintext` with XChaCha20-Poly1305 using a fresh random
/// 24-byte nonce. Returns both the ciphertext (with auth tag
/// appended) and the nonce — callers must store or transmit both to
/// decrypt later. Errors are surfaced as `String` to match the
/// pattern used in `crate::auth::derivation`.
pub fn encrypt(plaintext: &[u8], key: &[u8; KEY_LEN]) -> Result<EncryptedData, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("encrypt: {e}"))?;
    Ok(EncryptedData {
        ciphertext: ct,
        nonce: nonce_bytes.to_vec(),
    })
}

/// Decrypt an `EncryptedData` blob. Returns the plaintext on
/// success, or an error if the auth tag fails to verify (wrong key,
/// tampered ciphertext, tampered nonce, wrong nonce length, …).
///
/// XChaCha20-Poly1305 is an AEAD, so a successful decrypt is proof
/// that the ciphertext was produced by someone who knew the key
/// AND has not been modified since.
pub fn decrypt(data: &EncryptedData, key: &[u8; KEY_LEN]) -> Result<Vec<u8>, String> {
    if data.nonce.len() != NONCE_LEN {
        return Err(format!(
            "nonce must be {} bytes, got {}",
            NONCE_LEN,
            data.nonce.len()
        ));
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(&data.nonce);
    cipher
        .decrypt(nonce, data.ciphertext.as_ref())
        .map_err(|e| format!("decrypt: {e}"))
}

// ────────────────────────────────────────────────────────────────
// EncryptionManager — in-memory key holder (Stage 2)
// ────────────────────────────────────────────────────────────────
//
// Holds the user's `encryption_key` in process memory after it has
// been loaded from `~/.local/share/codemux/sync-key.enc` (machine-
// bound) or freshly derived from `(password, email)` on a setup or
// repair flow. Owned by Tauri's managed-state system so any Tauri
// command can read it via `State<'_, EncryptionManager>`.
//
// Mirrors Vexis's `src-tauri/src/encryption/manager.rs::EncryptionManager`
// at the API level. Intentionally simpler — no key derivation
// methods on the manager itself; derivation lives in
// `crate::auth::derive_login_credentials`.
//
// **Threat model:** the bytes never cross the Tauri IPC boundary.
// JS callers see a `sync_available: bool`; the raw key never leaves
// the Rust process. The renderer, React DevTools, and any
// JavaScript supply-chain attack are out of scope for the key.

use std::sync::Mutex;

/// Holds the 32-byte symmetric encryption key in process memory.
/// `None` means sync is locked / not yet set up.
#[derive(Default)]
pub struct EncryptionManager {
    key: Mutex<Option<[u8; KEY_LEN]>>,
}

impl EncryptionManager {
    /// Set the key. Replaces any previous value, zeroing the old
    /// one in-place via `write_volatile` before drop.
    pub fn set_key(&self, key: [u8; KEY_LEN]) -> Result<(), String> {
        let mut guard = self
            .key
            .lock()
            .map_err(|e| format!("encryption-manager poisoned: {e}"))?;
        if let Some(old) = guard.as_mut() {
            for byte in old.iter_mut() {
                unsafe { core::ptr::write_volatile(byte, 0) };
            }
        }
        *guard = Some(key);
        Ok(())
    }

    /// Returns true if a key is currently loaded.
    pub fn is_available(&self) -> bool {
        self.key
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Clear the in-memory key, zeroing the bytes first. Called on
    /// logout or when reset_sync wipes server-side state.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.key.lock() {
            if let Some(old) = guard.as_mut() {
                for byte in old.iter_mut() {
                    unsafe { core::ptr::write_volatile(byte, 0) };
                }
            }
            *guard = None;
        }
    }

    /// Run the closure with an immutable reference to the loaded key
    /// bytes. Used by Stage 3's sync layer to encrypt/decrypt
    /// without exposing the bytes to callers above the AEAD layer.
    /// Returns `None` if no key is loaded.
    #[allow(dead_code)] // Stage 3 sync layer
    pub fn with_key<R>(&self, f: impl FnOnce(&[u8; KEY_LEN]) -> R) -> Option<R> {
        let guard = self.key.lock().ok()?;
        guard.as_ref().map(f)
    }
}

impl std::fmt::Debug for EncryptionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = self.is_available();
        f.debug_struct("EncryptionManager")
            .field("key_loaded", &loaded)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn test_key() -> [u8; KEY_LEN] {
        let mut k = [0u8; KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(7);
        }
        k
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let pt = b"hello codemux";
        let blob = encrypt(pt, &key).unwrap();
        let out = decrypt(&blob, &key).unwrap();
        assert_eq!(out, pt);
    }

    #[test]
    fn encrypt_empty_plaintext_roundtrip() {
        let key = test_key();
        let blob = encrypt(b"", &key).unwrap();
        assert_eq!(decrypt(&blob, &key).unwrap(), b"");
    }

    #[test]
    fn encrypt_unicode_roundtrip() {
        let key = test_key();
        let pt = "héllo 世界 🎉 codemux".as_bytes();
        let blob = encrypt(pt, &key).unwrap();
        assert_eq!(decrypt(&blob, &key).unwrap(), pt);
    }

    #[test]
    fn encrypt_64kb_roundtrip() {
        // Realistic upper bound for a single skill body.
        let key = test_key();
        let pt: Vec<u8> = (0..65_536).map(|i| (i % 256) as u8).collect();
        let blob = encrypt(&pt, &key).unwrap();
        assert_eq!(decrypt(&blob, &key).unwrap(), pt);
    }

    #[test]
    fn encrypt_generates_24_byte_nonce() {
        let blob = encrypt(b"hello", &test_key()).unwrap();
        assert_eq!(blob.nonce.len(), NONCE_LEN);
    }

    #[test]
    fn encrypt_same_plaintext_twice_different_ciphertexts() {
        // Random nonce per call — same plaintext must produce
        // different ciphertext + nonce, otherwise the AEAD's
        // semantic security is broken.
        let key = test_key();
        let a = encrypt(b"hello", &key).unwrap();
        let b = encrypt(b"hello", &key).unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn encrypt_1000_unique_nonces() {
        let key = test_key();
        let mut seen = HashSet::with_capacity(1000);
        for _ in 0..1000 {
            let blob = encrypt(b"hello", &key).unwrap();
            assert!(
                seen.insert(blob.nonce.clone()),
                "nonce collision in 1000 encryptions — OsRng broken?"
            );
        }
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key_a = test_key();
        let mut key_b = test_key();
        key_b[0] ^= 1;
        let blob = encrypt(b"hello", &key_a).unwrap();
        assert!(decrypt(&blob, &key_b).is_err());
    }

    #[test]
    fn decrypt_with_flipped_ciphertext_bit_fails() {
        let key = test_key();
        let mut blob = encrypt(b"hello world", &key).unwrap();
        blob.ciphertext[0] ^= 0x01;
        assert!(decrypt(&blob, &key).is_err());
    }

    #[test]
    fn decrypt_with_flipped_nonce_bit_fails() {
        let key = test_key();
        let mut blob = encrypt(b"hello world", &key).unwrap();
        blob.nonce[0] ^= 0x01;
        assert!(decrypt(&blob, &key).is_err());
    }

    #[test]
    fn decrypt_with_truncated_ciphertext_fails() {
        let key = test_key();
        let mut blob = encrypt(b"hello world", &key).unwrap();
        blob.ciphertext.pop();
        assert!(decrypt(&blob, &key).is_err());
    }

    #[test]
    fn decrypt_with_wrong_nonce_length_fails() {
        let key = test_key();
        let blob = encrypt(b"hello", &key).unwrap();
        let bad_short = EncryptedData {
            ciphertext: blob.ciphertext.clone(),
            nonce: blob.nonce[..12].to_vec(),
        };
        assert!(decrypt(&bad_short, &key).is_err());
    }

    #[test]
    fn ciphertext_includes_16_byte_auth_tag() {
        // XChaCha20-Poly1305 appends a 16-byte tag to plaintext.
        let blob = encrypt(b"hello", &test_key()).unwrap();
        assert_eq!(blob.ciphertext.len(), 5 + 16);
    }

    #[test]
    fn encrypted_data_serializes_camel_case_json() {
        let blob = EncryptedData {
            ciphertext: vec![1, 2, 3],
            nonce: vec![4, 5, 6],
        };
        let json = serde_json::to_string(&blob).unwrap();
        assert!(json.contains("\"ciphertext\""));
        assert!(json.contains("\"nonce\""));
        assert!(!json.contains("cipher_text"));
    }

    // ── EncryptionManager ───────────────────────────────────────

    #[test]
    fn encryption_manager_starts_unloaded() {
        let m = EncryptionManager::default();
        assert!(!m.is_available());
    }

    #[test]
    fn encryption_manager_set_key_makes_it_available() {
        let m = EncryptionManager::default();
        m.set_key(test_key()).unwrap();
        assert!(m.is_available());
    }

    #[test]
    fn encryption_manager_clear_makes_it_unavailable() {
        let m = EncryptionManager::default();
        m.set_key(test_key()).unwrap();
        m.clear();
        assert!(!m.is_available());
    }

    #[test]
    fn encryption_manager_with_key_runs_closure() {
        let m = EncryptionManager::default();
        m.set_key(test_key()).unwrap();
        let recovered = m.with_key(|k| *k);
        assert_eq!(recovered, Some(test_key()));
    }

    #[test]
    fn encryption_manager_with_key_returns_none_when_unloaded() {
        let m = EncryptionManager::default();
        let result: Option<u8> = m.with_key(|_| 0u8);
        assert!(result.is_none());
    }

    #[test]
    fn encryption_manager_set_key_replaces_previous() {
        let m = EncryptionManager::default();
        m.set_key(test_key()).unwrap();
        let mut other = test_key();
        other[0] ^= 0xff;
        m.set_key(other).unwrap();
        let recovered = m.with_key(|k| *k);
        assert_eq!(recovered, Some(other));
    }

    #[test]
    fn encryption_manager_debug_does_not_print_key_bytes() {
        let m = EncryptionManager::default();
        m.set_key(test_key()).unwrap();
        let rendered = format!("{:?}", m);
        // The Debug impl prints "key_loaded: true|false", never the bytes.
        assert!(rendered.contains("key_loaded"));
        for byte in test_key() {
            // Don't assert the specific hex pair (random keys could
            // contain it coincidentally), but assert the slice
            // literal isn't there.
            assert!(!rendered.contains(&format!("0x{:02x}", byte)));
        }
    }

    #[test]
    fn encryption_manager_can_drive_aead_via_with_key() {
        // End-to-end: a plaintext encrypted via with_key + the same
        // key set in another manager decrypts cleanly. Proves the
        // closure captures the bytes correctly without copying them
        // to a wider scope.
        let pt = b"end-to-end through the manager";
        let m_a = EncryptionManager::default();
        m_a.set_key(test_key()).unwrap();
        let blob = m_a.with_key(|k| encrypt(pt, k).unwrap()).unwrap();

        let m_b = EncryptionManager::default();
        m_b.set_key(test_key()).unwrap();
        let recovered = m_b.with_key(|k| decrypt(&blob, k).unwrap()).unwrap();
        assert_eq!(recovered, pt);
    }
}
