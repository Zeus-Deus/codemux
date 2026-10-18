//! Webhook triggers for automations, plus the encrypted secret store their
//! signature checks read from.
//!
//! Schema changes here are additive and idempotent so they run on every
//! startup without a `SCHEMA_VERSION` bump: each `ALTER TABLE` tolerates
//! "duplicate column name", and every `CREATE` is `IF NOT EXISTS`.
//!
//! A webhook automation is device-local. The account API has no trigger
//! field, so `automations_sync` never pushes one, and it always runs on this
//! machine (`host_id IS NULL`), the device that receives the request.
use super::*;
use rand::RngCore;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    for stmt in [
        // 'schedule' | 'webhook'. Fixed at creation (see `AutomationTrigger`).
        "ALTER TABLE automations ADD COLUMN trigger_type TEXT NOT NULL DEFAULT 'schedule'",
        // Public half of the webhook URL; looked up on every request.
        "ALTER TABLE automations ADD COLUMN webhook_id TEXT",
        // Secret half of the webhook URL. Shown to the user so they can copy
        // the URL, so it is stored as-is; rotating it revokes the old URL.
        "ALTER TABLE automations ADD COLUMN webhook_token TEXT",
        // JSON `WebhookSignature`; NULL means no signature check.
        "ALTER TABLE automations ADD COLUMN webhook_signature TEXT",
        // `secrets.id` of the signing secret, set whenever a signature is.
        "ALTER TABLE automations ADD COLUMN webhook_secret_ref TEXT",
        "ALTER TABLE automation_runs ADD COLUMN trigger_type TEXT NOT NULL DEFAULT 'schedule'",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_automations_webhook_id \
             ON automations(webhook_id) WHERE webhook_id IS NOT NULL",
        // Values are sealed by `crate::secrets` (AES-256-GCM under a random
        // key kept outside the database) and never returned to the UI.
        "CREATE TABLE IF NOT EXISTS secrets (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            ciphertext BLOB NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    ] {
        if let Err(e) = conn.execute(stmt, []) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(format!("Schema migration failed ({stmt}): {msg}"));
            }
        }
    }
    Ok(())
}

/// What starts an automation's runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutomationTrigger {
    /// The recurrence in `schedule`, evaluated by `scheduler::tick`.
    #[default]
    Schedule,
    /// An HTTP request to the automation's webhook URL.
    Webhook,
}

impl AutomationTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Webhook => "webhook",
        }
    }

    /// Unknown values read back as `Schedule`, the column default.
    pub fn from_db(value: &str) -> Self {
        match value {
            "webhook" => Self::Webhook,
            _ => Self::Schedule,
        }
    }
}

/// How the signature digest is written in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureEncoding {
    Hex,
    Base64,
}

/// An HMAC-SHA256 check over the raw request body. GitHub sends
/// `x-hub-signature-256: sha256=<hex>`. The secret is held separately
/// (`webhook_secret_ref`) and is never part of this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookSignature {
    /// Lowercase request header carrying the signature.
    pub header: String,
    pub encoding: SignatureEncoding,
    /// Text before the digest, such as `sha256=`. Empty for none.
    #[serde(default)]
    pub prefix: String,
}

/// A stored secret, without its value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMeta {
    pub id: String,
    pub label: String,
    pub created_at: String,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh random id for a stored secret.
pub fn new_secret_id() -> String {
    format!("secret_{}", random_hex(16))
}

/// A fresh `(webhook_id, webhook_token)` pair for a new or rotated URL.
pub(super) fn new_webhook_credentials() -> (String, String) {
    (uuid::Uuid::new_v4().simple().to_string(), random_hex(32))
}

pub(super) fn signature_from_db(raw: Option<String>) -> Option<WebhookSignature> {
    raw.and_then(|json| serde_json::from_str(&json).ok())
}

impl DatabaseStore {
    /// The live webhook automation that owns `webhook_id`, if any.
    pub fn find_webhook_automation(&self, webhook_id: &str) -> Option<AutomationRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {AUTOMATION_COLUMNS} FROM automations
                 WHERE webhook_id = ?1 AND trigger_type = 'webhook'
                   AND user_id = 'local' AND deleted_at IS NULL"
            ),
            params![webhook_id],
            row_to_automation,
        )
        .optional()
        .unwrap_or(None)
    }

    /// Issue a new URL token for a webhook automation. The old URL stops
    /// working immediately.
    pub fn rotate_automation_webhook_token(&self, id: i64) -> Result<AutomationRecord, String> {
        let (_, token) = new_webhook_credentials();
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE automations SET webhook_token = ?1, updated_at = datetime('now')
                 WHERE id = ?2 AND trigger_type = 'webhook'
                   AND user_id = 'local' AND deleted_at IS NULL",
                params![token, id],
            )
            .map_err(|e| format!("Failed to rotate webhook token: {e}"))?;
        if affected == 0 {
            return Err(format!("No webhook automation with id {id}"));
        }
        conn.query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            params![id],
            row_to_automation,
        )
        .map_err(|e| format!("Failed to re-read automation: {e}"))
    }

    /// Set or clear a webhook automation's signature check. A signature
    /// always comes with the `secrets.id` of its signing secret, which must
    /// exist.
    pub fn set_automation_webhook_signature(
        &self,
        id: i64,
        signature: Option<(&WebhookSignature, &str)>,
    ) -> Result<AutomationRecord, String> {
        let conn = self.conn.lock().unwrap();
        let (json, secret_ref) = match signature {
            Some((signature, secret_ref)) => {
                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM secrets WHERE id = ?1)",
                        params![secret_ref],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("Failed to look up secret: {e}"))?;
                if !exists {
                    return Err("That secret no longer exists. Enter it again.".into());
                }
                let json = serde_json::to_string(signature)
                    .map_err(|e| format!("Failed to encode signature: {e}"))?;
                (Some(json), Some(secret_ref))
            }
            None => (None, None),
        };
        let affected = conn
            .execute(
                "UPDATE automations
                 SET webhook_signature = ?1, webhook_secret_ref = ?2,
                     updated_at = datetime('now')
                 WHERE id = ?3 AND trigger_type = 'webhook'
                   AND user_id = 'local' AND deleted_at IS NULL",
                params![json, secret_ref, id],
            )
            .map_err(|e| format!("Failed to set webhook signature: {e}"))?;
        if affected == 0 {
            return Err(format!("No webhook automation with id {id}"));
        }
        conn.query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            params![id],
            row_to_automation,
        )
        .map_err(|e| format!("Failed to re-read automation: {e}"))
    }

    /// Record a webhook-triggered run. The overlap check and the insert
    /// share one lock, so two concurrent deliveries cannot both start a
    /// run: the second is recorded `skipped_busy`. Returns `Ok(None)` when
    /// a run already exists for `scheduled_for`.
    pub fn record_webhook_run(
        &self,
        automation_id: i64,
        scheduled_for: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let busy: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM automation_runs
                 WHERE automation_id = ?1 AND status IN ('scheduled', 'running'))",
                params![automation_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check active runs: {e}"))?;
        let status = if busy { "skipped_busy" } else { "scheduled" };
        let affected = conn
            .execute(
                "INSERT INTO automation_runs
                    (automation_id, status, scheduled_for, trigger_type)
                 VALUES (?1, ?2, ?3, 'webhook')
                 ON CONFLICT(automation_id, scheduled_for) DO NOTHING",
                params![automation_id, status, scheduled_for],
            )
            .map_err(|e| format!("Failed to record webhook run: {e}"))?;
        if affected == 0 {
            return Ok(None);
        }
        let id = conn.last_insert_rowid();
        conn.query_row(
            &format!("SELECT {AUTOMATION_RUN_COLUMNS} FROM automation_runs WHERE id = ?1"),
            params![id],
            row_to_automation_run,
        )
        .map(Some)
        .map_err(|e| format!("Failed to re-read webhook run: {e}"))
    }

    // ── Secrets ──

    /// Store an already-encrypted secret under `id` (see `new_secret_id`).
    pub fn insert_secret(
        &self,
        id: &str,
        label: &str,
        ciphertext: &[u8],
    ) -> Result<SecretMeta, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO secrets (id, label, ciphertext) VALUES (?1, ?2, ?3)",
            params![id, label, ciphertext],
        )
        .map_err(|e| format!("Failed to store secret: {e}"))?;
        conn.query_row(
            "SELECT id, label, created_at FROM secrets WHERE id = ?1",
            params![id],
            row_to_secret_meta,
        )
        .map_err(|e| format!("Failed to re-read secret: {e}"))
    }

    /// Every stored secret, newest first, without values.
    pub fn list_secrets(&self) -> Vec<SecretMeta> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn
            .prepare("SELECT id, label, created_at FROM secrets ORDER BY created_at DESC, id")
        {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_secrets prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_secret_meta) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_secrets query failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn load_secret_ciphertext(&self, id: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT ciphertext FROM secrets WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .unwrap_or(None)
    }

    /// Delete a secret. Refused while a live automation still checks
    /// signatures with it, so a delivery never loses its secret silently.
    pub fn delete_secret(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let in_use: Option<String> = conn
            .query_row(
                "SELECT name FROM automations
                 WHERE webhook_secret_ref = ?1 AND deleted_at IS NULL LIMIT 1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to check secret use: {e}"))?;
        if let Some(name) = in_use {
            return Err(format!(
                "The automation \"{name}\" uses this secret. Remove its signature check first."
            ));
        }
        let affected = conn
            .execute("DELETE FROM secrets WHERE id = ?1", params![id])
            .map_err(|e| format!("Failed to delete secret: {e}"))?;
        if affected == 0 {
            return Err("No such secret".into());
        }
        Ok(())
    }
}

fn row_to_secret_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<SecretMeta> {
    Ok(SecretMeta {
        id: row.get(0)?,
        label: row.get(1)?,
        created_at: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent_and_backfills_existing_rows() {
        let db = init_test_database();
        let existing = db
            .insert_automation(&AutomationInput {
                name: "Daily".into(),
                prompt: "p".into(),
                agent: "claude".into(),
                schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".into(),
                timezone: "UTC".into(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
                trigger_type: None,
            })
            .unwrap();
        // A second startup runs the same statements against the upgraded
        // schema; every one must be a no-op.
        create_schema(&db.conn.lock().unwrap()).unwrap();
        let reread = db.get_automation(existing.id).unwrap();
        assert_eq!(reread.trigger_type, AutomationTrigger::Schedule);
        assert!(reread.webhook_id.is_none());
    }

    #[test]
    fn an_undecodable_stored_signature_fails_closed() {
        use crate::automations::webhook::{accept_delivery, DeliveryRejection, WebhookRequest};
        let db = init_test_database();
        let a = db
            .insert_automation(&AutomationInput {
                name: "Hook".into(),
                prompt: "p".into(),
                agent: "claude".into(),
                schedule: String::new(),
                timezone: "UTC".into(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
                trigger_type: Some(AutomationTrigger::Webhook),
            })
            .unwrap();
        let secret = crate::secrets::store(&db, "GitHub", "s").unwrap();
        let signature = WebhookSignature {
            header: "x-hub-signature-256".into(),
            encoding: SignatureEncoding::Hex,
            prefix: "sha256=".into(),
        };
        db.set_automation_webhook_signature(a.id, Some((&signature, &secret.id)))
            .unwrap();
        // Damage, or an encoding from a newer build this one does not know.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE automations SET webhook_signature = '{\"encoding\":\"sha512\"}' WHERE id = ?1",
                params![a.id],
            )
            .unwrap();
        let req = WebhookRequest::new("POST", "/", "", [], Vec::new());
        assert_eq!(
            accept_delivery(
                &db,
                a.webhook_id.as_deref().unwrap(),
                a.webhook_token.as_deref().unwrap(),
                &req,
                chrono::Utc::now(),
            )
            .unwrap_err(),
            DeliveryRejection::SignatureUnavailable
        );
    }

    #[test]
    fn webhook_token_is_never_serialized_from_a_record() {
        let db = init_test_database();
        let record = db
            .insert_automation(&AutomationInput {
                name: "Hook".into(),
                prompt: "p".into(),
                agent: "claude".into(),
                schedule: String::new(),
                timezone: "UTC".into(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
                trigger_type: Some(AutomationTrigger::Webhook),
            })
            .unwrap();
        let token = record.webhook_token.clone().unwrap();
        assert!(!serde_json::to_string(&record).unwrap().contains(&token));
    }
}
