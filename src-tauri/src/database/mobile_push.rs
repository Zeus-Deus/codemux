//! Phone-app push registrations (`crate::web_remote::native_push`).
//!
//! One row per paired remote session that runs the native Codemux app. The
//! row holds the relay handle the phone obtained for itself (`device_id` and
//! the `send_key` capability), never the APNs/FCM token: only the relay knows
//! that. Revoking the session ends delivery; the sender also drops rows whose
//! session is gone.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mobile_push_devices (
            session_id TEXT PRIMARY KEY,
            relay_url TEXT NOT NULL,
            device_id TEXT NOT NULL,
            send_key TEXT NOT NULL,
            platform TEXT NOT NULL,
            label TEXT,
            categories TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("Failed to create mobile push table: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobilePushDevice {
    pub session_id: String,
    pub relay_url: String,
    pub device_id: String,
    #[serde(skip_serializing)]
    pub send_key: String,
    pub platform: String,
    pub label: Option<String>,
    /// JSON object of category → enabled, owned by `native_push`.
    pub categories: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn row_to_device(row: &rusqlite::Row<'_>) -> rusqlite::Result<MobilePushDevice> {
    Ok(MobilePushDevice {
        session_id: row.get(0)?,
        relay_url: row.get(1)?,
        device_id: row.get(2)?,
        send_key: row.get(3)?,
        platform: row.get(4)?,
        label: row.get(5)?,
        categories: row.get(6)?,
        created_at_ms: row.get(7)?,
        updated_at_ms: row.get(8)?,
    })
}

const COLUMNS: &str =
    "session_id, relay_url, device_id, send_key, platform, label, categories, created_at_ms, updated_at_ms";

impl DatabaseStore {
    /// Insert or replace this session's registration. A relay handle belongs
    /// to one phone, so another session holding the same handle (the phone
    /// re-paired) is dropped rather than notified twice.
    pub fn upsert_mobile_push_device(&self, device: &MobilePushDevice) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to open mobile push transaction: {e}"))?;
        tx.execute(
            "DELETE FROM mobile_push_devices WHERE relay_url = ?1 AND device_id = ?2 AND session_id != ?3",
            params![device.relay_url, device.device_id, device.session_id],
        )
        .map_err(|e| format!("Failed to replace mobile push device: {e}"))?;
        let created_at_ms: i64 = tx
            .query_row(
                "SELECT created_at_ms FROM mobile_push_devices WHERE session_id = ?1",
                params![device.session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read mobile push device: {e}"))?
            .unwrap_or(device.created_at_ms);
        tx.execute(
            &format!("INSERT OR REPLACE INTO mobile_push_devices ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
            params![
                device.session_id,
                device.relay_url,
                device.device_id,
                device.send_key,
                device.platform,
                device.label,
                device.categories,
                created_at_ms,
                device.updated_at_ms
            ],
        )
        .map_err(|e| format!("Failed to save mobile push device: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit mobile push device: {e}"))
    }

    pub fn mobile_push_device(&self, session_id: &str) -> Result<Option<MobilePushDevice>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM mobile_push_devices WHERE session_id = ?1"),
            params![session_id],
            row_to_device,
        )
        .optional()
        .map_err(|e| format!("Failed to read mobile push device: {e}"))
    }

    pub fn mobile_push_devices(&self) -> Result<Vec<MobilePushDevice>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM mobile_push_devices ORDER BY created_at_ms"))
            .map_err(|e| format!("Failed to list mobile push devices: {e}"))?;
        let rows = stmt
            .query_map([], row_to_device)
            .map_err(|e| format!("Failed to list mobile push devices: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to list mobile push devices: {e}"))
    }

    pub fn set_mobile_push_categories(
        &self,
        session_id: &str,
        categories: &str,
        now_ms: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE mobile_push_devices SET categories = ?2, updated_at_ms = ?3 WHERE session_id = ?1",
            params![session_id, categories, now_ms],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to update mobile push preferences: {e}"))
    }

    pub fn delete_mobile_push_device(&self, session_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM mobile_push_devices WHERE session_id = ?1",
            params![session_id],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to delete mobile push device: {e}"))
    }

    /// Forget a handle the relay reported gone, whichever session holds it.
    /// Matching the key too means a phone that re-registered in the
    /// meantime keeps its fresh row.
    pub fn delete_mobile_push_handle(
        &self,
        relay_url: &str,
        device_id: &str,
        send_key: &str,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM mobile_push_devices WHERE relay_url = ?1 AND device_id = ?2 AND send_key = ?3",
            params![relay_url, device_id, send_key],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to delete mobile push device: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(session: &str, handle: &str, key: &str, at: i64) -> MobilePushDevice {
        MobilePushDevice {
            session_id: session.into(),
            relay_url: "https://relay.test/api/push".into(),
            device_id: handle.into(),
            send_key: key.into(),
            platform: "ios".into(),
            label: Some("iPhone".into()),
            categories: "{}".into(),
            created_at_ms: at,
            updated_at_ms: at,
        }
    }

    #[test]
    fn mobile_push_upsert_keeps_creation_time_and_one_session_per_handle() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_mobile_push_device(&device("s1", "d1", "k1", 10)).unwrap();
        db.upsert_mobile_push_device(&device("s1", "d1", "k2", 20)).unwrap();
        let row = db.mobile_push_device("s1").unwrap().unwrap();
        assert_eq!((row.created_at_ms, row.updated_at_ms, row.send_key.as_str()), (10, 20, "k2"));

        // The phone re-paired: its handle moves to the new session.
        db.upsert_mobile_push_device(&device("s2", "d1", "k2", 30)).unwrap();
        let all = db.mobile_push_devices().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id, "s2");
    }

    #[test]
    fn mobile_push_gone_handle_only_deletes_the_matching_key() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_mobile_push_device(&device("s1", "d1", "fresh", 10)).unwrap();
        assert!(!db.delete_mobile_push_handle("https://relay.test/api/push", "d1", "stale").unwrap());
        assert!(db.mobile_push_device("s1").unwrap().is_some());
        assert!(db.delete_mobile_push_handle("https://relay.test/api/push", "d1", "fresh").unwrap());
        assert!(db.mobile_push_device("s1").unwrap().is_none());
    }

    #[test]
    fn mobile_push_preferences_and_unregister() {
        let db = DatabaseStore::new_in_memory();
        assert!(!db.set_mobile_push_categories("s1", "{\"pr\":false}", 5).unwrap());
        db.upsert_mobile_push_device(&device("s1", "d1", "k", 1)).unwrap();
        assert!(db.set_mobile_push_categories("s1", "{\"pr\":false}", 5).unwrap());
        assert_eq!(db.mobile_push_device("s1").unwrap().unwrap().categories, "{\"pr\":false}");
        assert!(db.delete_mobile_push_device("s1").unwrap());
        assert!(!db.delete_mobile_push_device("s1").unwrap());
    }
}
