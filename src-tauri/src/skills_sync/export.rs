// Skills sync — local export / import (Step 10 — Stage 4).
//
// The catastrophic-loss mitigation for E2E sync: if the user forgets
// their sync password, the server-stored ciphertext is unrecoverable
// (by design — the server has no key material). Without a backup,
// resetting their password destroys every synced skill.
//
// This module provides the safety net: pull every encrypted skill,
// decrypt it with the in-memory key, and write the plaintext to a
// JSON file the user picks via the OS file dialog. The reset flow
// in `commands/skills_sync.rs` enforces this as a step before
// triggering the email-based password reset.
//
// Format choice: plaintext JSON, not encrypted. The backup is on
// the user's own disk, encrypted "at rest" only by whatever
// disk-level protection their OS provides. Encrypting the export
// would have required asking for an export password (terrible
// UX) or deriving one from the same forgotten password (defeats
// the purpose). Vexis's `commands/dictionary.rs::export_dictionary`
// follows the same plaintext-JSON pattern.
//
// Import path: re-encrypt each skill with the CURRENT in-memory
// encryption_key (the new password's key, after reset) and push
// via `crate::skills_sync::api_client::create_skill`. This is
// effectively "replay your synced skills under the new key." We
// don't try to restore the original `remote_id`s — the server
// assigns fresh ones on POST and the local mapping table catches
// up on the next sync cycle.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::encryption::{decrypt, encrypt, EncryptedData, EncryptionManager};
use crate::skills_sync::api_client::{self, SkillUpload};

/// Bumped when the on-disk shape changes incompatibly. Readers
/// below the current value reject the file with a clear error.
pub const EXPORT_FORMAT_VERSION: u32 = 1;

/// Product tag — used so a Vexis export can't accidentally be
/// imported into Codemux (or vice versa) and silently produce
/// gibberish.
pub const EXPORT_PRODUCT: &str = "codemux";

/// Top-level shape on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsExportFile {
    pub version: u32,
    /// ISO-8601 UTC. The exporter's wall clock; informational
    /// only (no security boundary depends on it).
    pub exported_at: String,
    /// The email of the account that produced this export. The
    /// importer warns when it doesn't match the current user;
    /// does not block the import.
    pub user_email: String,
    /// Always `EXPORT_PRODUCT` for Codemux. Reserved for future
    /// shared-protocol products.
    pub product: String,
    pub skill_count: usize,
    pub skills: Vec<ExportedSkill>,
}

/// One skill, decrypted. `origin_path` is informational (the
/// exporter's last-known local path) — the importer always pushes
/// to fresh remote ids, and the next pull lands the skills at
/// `~/.codemux/skills/<name>/SKILL.md` regardless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSkill {
    pub name: String,
    pub content: String,
    pub provider: String,
    pub scope: String,
    #[serde(default)]
    pub origin_path: Option<String>,
    pub updated_at: String,
}

/// Returned by `export_all_synced_skills`. Logged + surfaced in
/// the UI toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub path: PathBuf,
    pub skill_count: usize,
    pub bytes_written: u64,
    /// Skills that came back from the server but failed to
    /// decrypt — typically zero, but a stale local key against
    /// freshly-set server data could produce non-zero.
    pub failed_count: usize,
}

/// Returned by `import_exported_skills`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub queued_count: usize,
    pub failed_count: usize,
    /// The current user's email differs from the export's
    /// `user_email`. Soft warning — the import still proceeds.
    pub mismatched_email: bool,
}

/// Default filename used when the OS save-dialog opens. Includes
/// the YYYY-MM-DD so users with multiple exports can tell them
/// apart at a glance.
pub fn recommended_export_filename() -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");
    format!("codemux-skills-export-{today}.json")
}

/// Pull every encrypted skill, decrypt, and atomic-write a JSON
/// file. Caller passes the bearer token + the in-memory key
/// holder; we don't reach into Tauri state from here so the
/// module stays unit-testable.
pub async fn export_all_synced_skills(
    token: &str,
    encryption: &EncryptionManager,
    output_path: &Path,
    user_email: &str,
) -> Result<ExportSummary, String> {
    let remote = api_client::list_skills(token).await?;
    let mut skills = Vec::with_capacity(remote.len());
    let mut failed_count = 0usize;

    for wire in remote {
        match decrypt_wire(&wire, encryption) {
            Ok(skill) => skills.push(skill),
            Err(err) => {
                eprintln!(
                    "[skills_sync/export] failed to decrypt remote_id={}: {err}",
                    wire.remote_id
                );
                failed_count += 1;
            }
        }
    }

    let exported_at = chrono::Utc::now().to_rfc3339();
    let envelope = SkillsExportFile {
        version: EXPORT_FORMAT_VERSION,
        exported_at,
        user_email: user_email.to_string(),
        product: EXPORT_PRODUCT.to_string(),
        skill_count: skills.len(),
        skills,
    };

    let bytes = atomic_write_json(output_path, &envelope)?;
    Ok(ExportSummary {
        path: output_path.to_path_buf(),
        skill_count: envelope.skill_count,
        bytes_written: bytes,
        failed_count,
    })
}

/// Read a previously-exported file, validate its envelope, and
/// re-push every skill to the server using the CURRENT
/// encryption key. Returns a summary the UI surfaces as a toast.
pub async fn import_exported_skills(
    token: &str,
    encryption: &EncryptionManager,
    file_path: &Path,
    current_user_email: &str,
) -> Result<ImportSummary, String> {
    let bytes = fs::read(file_path)
        .map_err(|e| format!("read export file: {e}"))?;
    let envelope: SkillsExportFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("parse export file: {e}"))?;

    if envelope.version != EXPORT_FORMAT_VERSION {
        return Err(format!(
            "Export file version {} not supported (expected {EXPORT_FORMAT_VERSION})",
            envelope.version
        ));
    }
    if envelope.product != EXPORT_PRODUCT {
        return Err(format!(
            "Export file is from a different product ({}) — not importable into Codemux",
            envelope.product
        ));
    }

    let mismatched_email = !user_emails_match(&envelope.user_email, current_user_email);
    let mut queued_count = 0usize;
    let mut failed_count = 0usize;

    for skill in envelope.skills {
        match push_one_skill(token, encryption, &skill).await {
            Ok(()) => queued_count += 1,
            Err(err) => {
                eprintln!(
                    "[skills_sync/import] push '{}' failed: {err}",
                    skill.name
                );
                failed_count += 1;
            }
        }
    }

    Ok(ImportSummary {
        queued_count,
        failed_count,
        mismatched_email,
    })
}

// ────────────────────────────────────────────────────────────────
// Helpers.

fn decrypt_wire(
    wire: &api_client::SkillWire,
    encryption: &EncryptionManager,
) -> Result<ExportedSkill, String> {
    encryption
        .with_key(|key| -> Result<ExportedSkill, String> {
            let name_bytes = decrypt(
                &EncryptedData {
                    ciphertext: base64_decode(&wire.encrypted_name)?,
                    nonce: base64_decode(&wire.nonce_name)?,
                },
                key,
            )?;
            let content_bytes = decrypt(
                &EncryptedData {
                    ciphertext: base64_decode(&wire.encrypted_content)?,
                    nonce: base64_decode(&wire.nonce_content)?,
                },
                key,
            )?;
            Ok(ExportedSkill {
                name: String::from_utf8(name_bytes)
                    .map_err(|e| format!("name not utf-8: {e}"))?,
                content: String::from_utf8(content_bytes)
                    .map_err(|e| format!("content not utf-8: {e}"))?,
                provider: wire.provider.clone(),
                scope: wire.scope.clone(),
                origin_path: None,
                updated_at: wire.updated_at.clone(),
            })
        })
        .ok_or_else(|| "sync key not loaded".to_string())?
}

async fn push_one_skill(
    token: &str,
    encryption: &EncryptionManager,
    skill: &ExportedSkill,
) -> Result<(), String> {
    let upload = encryption
        .with_key(|key| -> Result<SkillUpload, String> {
            let name = encrypt(skill.name.as_bytes(), key)?;
            let content = encrypt(skill.content.as_bytes(), key)?;
            Ok(SkillUpload {
                encrypted_name: base64_encode(&name.ciphertext),
                nonce_name: base64_encode(&name.nonce),
                encrypted_content: base64_encode(&content.ciphertext),
                nonce_content: base64_encode(&content.nonce),
                provider: skill.provider.clone(),
                scope: skill.scope.clone(),
            })
        })
        .ok_or_else(|| "sync key not loaded".to_string())??;
    api_client::create_skill(token, &upload).await?;
    Ok(())
}

/// Email comparison for the "this backup is from a different
/// account" check. Matches the same normalization used for the
/// `codemux-api-*` derivation salt so a backup from
/// `Alice@Example.COM` and a current session as
/// `alice@example.com` count as the same account.
fn user_emails_match(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create export dir: {e}"))?;
        }
    }
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("serialize export: {e}"))?;
    let bytes = json.len() as u64;

    // Tmp+rename so a crash mid-write can't corrupt an existing
    // backup file the user previously saved at the same path.
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &json).map_err(|e| format!("write tmp export: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename export: {e}"))?;
    Ok(bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64 decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn known_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(7);
        }
        k
    }

    fn manager_with_known_key() -> EncryptionManager {
        let m = EncryptionManager::default();
        m.set_key(known_key()).unwrap();
        m
    }

    fn sample_export_file(skills: Vec<ExportedSkill>, email: &str) -> SkillsExportFile {
        SkillsExportFile {
            version: EXPORT_FORMAT_VERSION,
            exported_at: chrono::Utc::now().to_rfc3339(),
            user_email: email.to_string(),
            product: EXPORT_PRODUCT.to_string(),
            skill_count: skills.len(),
            skills,
        }
    }

    fn skill_row(name: &str, content: &str) -> ExportedSkill {
        ExportedSkill {
            name: name.into(),
            content: content.into(),
            provider: "codemux".into(),
            scope: "user".into(),
            origin_path: None,
            updated_at: "2026-04-29T20:00:00Z".into(),
        }
    }

    // ── Filename helper ────────────────────────────────────────

    #[test]
    fn recommended_filename_includes_today() {
        let name = recommended_export_filename();
        assert!(name.starts_with("codemux-skills-export-"));
        assert!(name.ends_with(".json"));
        // YYYY-MM-DD between prefix and suffix.
        let date = name
            .trim_start_matches("codemux-skills-export-")
            .trim_end_matches(".json");
        assert_eq!(date.len(), 10, "expected YYYY-MM-DD, got '{date}'");
    }

    // ── atomic_write_json ──────────────────────────────────────

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("nested/sub/export.json");
        let envelope = sample_export_file(
            vec![skill_row("a", "alpha")],
            "user@example.com",
        );
        let bytes = atomic_write_json(&dest, &envelope).unwrap();
        assert!(dest.exists());
        assert!(bytes > 0);
        assert!(!dest.with_extension("tmp").exists());
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("export.json");

        let v1 = sample_export_file(
            vec![skill_row("a", "first")],
            "user@example.com",
        );
        atomic_write_json(&p, &v1).unwrap();
        let v2 = sample_export_file(
            vec![skill_row("a", "second")],
            "user@example.com",
        );
        atomic_write_json(&p, &v2).unwrap();

        let recovered: SkillsExportFile =
            serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        assert_eq!(recovered.skills[0].content, "second");
    }

    // ── decrypt_wire (export decryption) ───────────────────────

    fn fake_wire(name: &str, content: &str, remote_id: &str, key: &[u8; 32]) -> api_client::SkillWire {
        let n = encrypt(name.as_bytes(), key).unwrap();
        let c = encrypt(content.as_bytes(), key).unwrap();
        api_client::SkillWire {
            remote_id: remote_id.into(),
            encrypted_name: base64_encode(&n.ciphertext),
            nonce_name: base64_encode(&n.nonce),
            encrypted_content: base64_encode(&c.ciphertext),
            nonce_content: base64_encode(&c.nonce),
            provider: "codemux".into(),
            scope: "user".into(),
            updated_at: "2026-04-29T20:00:00Z".into(),
        }
    }

    #[test]
    fn decrypt_wire_recovers_plaintext_with_correct_key() {
        let key = known_key();
        let mgr = manager_with_known_key();
        let wire = fake_wire("demo", "# Demo\nbody", "1", &key);
        let skill = decrypt_wire(&wire, &mgr).unwrap();
        assert_eq!(skill.name, "demo");
        assert_eq!(skill.content, "# Demo\nbody");
        assert_eq!(skill.provider, "codemux");
        assert_eq!(skill.scope, "user");
    }

    #[test]
    fn decrypt_wire_fails_when_no_key_loaded() {
        let key = known_key();
        let wire = fake_wire("demo", "content", "1", &key);
        let mgr_no_key = EncryptionManager::default();
        let res = decrypt_wire(&wire, &mgr_no_key);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("sync key not loaded"));
    }

    #[test]
    fn decrypt_wire_fails_with_wrong_key() {
        let mut other_key = known_key();
        other_key[0] ^= 0xff;
        let wire = fake_wire("demo", "content", "1", &other_key);
        let mgr = manager_with_known_key();
        assert!(decrypt_wire(&wire, &mgr).is_err());
    }

    // ── Export envelope structure ──────────────────────────────

    #[test]
    fn export_file_serializes_with_expected_top_level_fields() {
        let envelope = sample_export_file(
            vec![skill_row("foo", "bar")],
            "user@example.com",
        );
        let json = serde_json::to_value(&envelope).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("version").unwrap().as_u64().unwrap(), 1);
        assert_eq!(obj.get("product").unwrap().as_str().unwrap(), "codemux");
        assert_eq!(obj.get("user_email").unwrap().as_str().unwrap(), "user@example.com");
        assert_eq!(obj.get("skill_count").unwrap().as_u64().unwrap(), 1);
        assert!(obj.contains_key("exported_at"));
        assert!(obj.contains_key("skills"));
    }

    // ── Import envelope validation ─────────────────────────────

    #[tokio::test]
    async fn import_rejects_unknown_version() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("bad.json");
        let envelope = serde_json::json!({
            "version": 999,
            "exported_at": "2026-04-29T00:00:00Z",
            "user_email": "user@example.com",
            "product": EXPORT_PRODUCT,
            "skill_count": 0,
            "skills": [],
        });
        fs::write(&p, envelope.to_string()).unwrap();

        let mgr = manager_with_known_key();
        let res = import_exported_skills(
            "ignored-token",
            &mgr,
            &p,
            "user@example.com",
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("version"));
    }

    #[tokio::test]
    async fn import_rejects_wrong_product() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("vexis.json");
        let envelope = serde_json::json!({
            "version": EXPORT_FORMAT_VERSION,
            "exported_at": "2026-04-29T00:00:00Z",
            "user_email": "user@example.com",
            "product": "vexis",
            "skill_count": 0,
            "skills": [],
        });
        fs::write(&p, envelope.to_string()).unwrap();

        let mgr = manager_with_known_key();
        let res = import_exported_skills(
            "ignored-token",
            &mgr,
            &p,
            "user@example.com",
        )
        .await;
        assert!(res.is_err());
        let msg = res.unwrap_err();
        assert!(msg.contains("different product") || msg.contains("vexis"));
    }

    #[tokio::test]
    async fn import_rejects_malformed_json() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("garbage.json");
        fs::write(&p, b"not valid json {").unwrap();

        let mgr = manager_with_known_key();
        let res = import_exported_skills(
            "ignored-token",
            &mgr,
            &p,
            "user@example.com",
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("parse"));
    }

    #[tokio::test]
    async fn import_rejects_missing_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("does-not-exist.json");

        let mgr = manager_with_known_key();
        let res = import_exported_skills(
            "ignored-token",
            &mgr,
            &p,
            "user@example.com",
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("read"));
    }

    // ── Email-mismatch handling ────────────────────────────────

    #[test]
    fn user_emails_match_is_case_and_whitespace_insensitive() {
        assert!(user_emails_match("user@example.com", "user@example.com"));
        assert!(user_emails_match("USER@example.com", "user@example.com"));
        assert!(user_emails_match("  user@example.com ", "user@example.com"));
        assert!(!user_emails_match("user@example.com", "other@example.com"));
    }

    // Note: a true "email mismatch warns but proceeds" test
    // requires a mock HTTP server because import calls
    // `create_skill`. The server-side push behavior is already
    // exercised by the engine's integration with the real API in
    // Stage 3's smoke. Here we lock the email-comparison
    // semantics so the warning fires for the right inputs.

    #[test]
    fn export_summary_serializes_camelcase() {
        let summary = ExportSummary {
            path: PathBuf::from("/tmp/x.json"),
            skill_count: 3,
            bytes_written: 512,
            failed_count: 0,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert!(json.get("skillCount").is_some());
        assert!(json.get("bytesWritten").is_some());
        assert!(json.get("failedCount").is_some());
    }

    #[test]
    fn import_summary_serializes_camelcase() {
        let summary = ImportSummary {
            queued_count: 5,
            failed_count: 1,
            mismatched_email: true,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert!(json.get("queuedCount").is_some());
        assert!(json.get("failedCount").is_some());
        assert!(json.get("mismatchedEmail").is_some());
    }

    // ── Roundtrip through the file format ──────────────────────

    #[test]
    fn write_and_read_roundtrip_preserves_skills() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("roundtrip.json");

        let original = sample_export_file(
            vec![
                skill_row("a", "alpha body"),
                skill_row("b", "beta body"),
                skill_row("c", "gamma body with newlines\nand things"),
            ],
            "user@example.com",
        );
        atomic_write_json(&p, &original).unwrap();

        let raw = fs::read(&p).unwrap();
        let recovered: SkillsExportFile = serde_json::from_slice(&raw).unwrap();
        assert_eq!(recovered.version, EXPORT_FORMAT_VERSION);
        assert_eq!(recovered.product, EXPORT_PRODUCT);
        assert_eq!(recovered.user_email, "user@example.com");
        assert_eq!(recovered.skill_count, 3);
        assert_eq!(recovered.skills.len(), 3);
        assert_eq!(recovered.skills[2].content, "gamma body with newlines\nand things");
    }

    #[test]
    fn empty_export_is_valid_json_not_error() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("empty.json");
        let envelope = sample_export_file(vec![], "user@example.com");
        atomic_write_json(&p, &envelope).unwrap();

        let recovered: SkillsExportFile =
            serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        assert_eq!(recovered.skill_count, 0);
        assert!(recovered.skills.is_empty());
    }
}
