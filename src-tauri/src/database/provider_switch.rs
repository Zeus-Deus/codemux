//! In-place provider switches (`commands::provider_switch`).
//!
//! A switch starts the new provider with no native history, so the chat so
//! far rides along on the first turn it receives. This table holds that
//! one-shot seed: written once the new provider has started, read by the
//! next send to that provider, and deleted once the send is accepted. One
//! row per thread: a second switch before anything was sent replaces it.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_chat_provider_switch_seeds (
            thread_id TEXT PRIMARY KEY,
            -- The provider the seed is for; a send to any other leaves it.
            provider TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );",
    )
    .map_err(|e| format!("Failed to create provider switch table: {e}"))
}

impl DatabaseStore {
    pub fn put_provider_switch_seed(
        &self,
        thread_id: &str,
        provider: &str,
        content: &str,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_chat_provider_switch_seeds (thread_id, provider, content)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(thread_id) DO UPDATE SET
                    provider = excluded.provider,
                    content = excluded.content,
                    created_at = datetime('now')",
                params![thread_id, provider, content],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to save the provider switch context: {e}"))
    }

    /// The seed waiting for `provider`'s first turn on `thread_id`.
    pub fn provider_switch_seed(&self, thread_id: &str, provider: &str) -> Option<String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT content FROM agent_chat_provider_switch_seeds
                 WHERE thread_id = ?1 AND provider = ?2",
                params![thread_id, provider],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Drop the seed once a turn carrying it was accepted. Matches the
    /// content too, so a newer switch's seed is never the one removed.
    pub fn clear_provider_switch_seed(
        &self,
        thread_id: &str,
        provider: &str,
        content: &str,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM agent_chat_provider_switch_seeds
                 WHERE thread_id = ?1 AND provider = ?2 AND content = ?3",
                params![thread_id, provider, content],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to clear the provider switch context: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "ws", Some("/repo"), "claude").unwrap();
        db
    }

    #[test]
    fn a_seed_waits_for_its_provider_and_clears_once() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "so far").unwrap();
        assert_eq!(db.provider_switch_seed("t", "claude"), None);
        assert_eq!(db.provider_switch_seed("t", "codex").as_deref(), Some("so far"));
        db.clear_provider_switch_seed("t", "codex", "so far").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
    }

    #[test]
    fn a_later_switch_replaces_the_seed_and_survives_an_older_clear() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "first").unwrap();
        db.put_provider_switch_seed("t", "codex", "second").unwrap();
        db.clear_provider_switch_seed("t", "codex", "first").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex").as_deref(), Some("second"));
        db.put_provider_switch_seed("t", "opencode", "third").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
    }

    #[test]
    fn deleting_the_chat_drops_its_seed() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "so far").unwrap();
        db.delete_agent_chat_session("t").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
    }
}
