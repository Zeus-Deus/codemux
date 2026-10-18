//! In-place provider switches (`commands::provider_switch`).
//!
//! A switch starts the new provider with no native history, so the chat so
//! far rides along on the first turn it receives. This table holds that
//! seed, one row per thread (a later switch replaces it). The row stays
//! after a turn carried it: `sent_nonce` names that turn, and while its
//! user message is in the transcript the provider has the context. Once it
//! isn't (the send failed, its queued turn was cancelled, or a revert rolled
//! it back) the next turn carries the seed again.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_chat_provider_switch_seeds (
            thread_id TEXT PRIMARY KEY,
            -- The provider the seed is for; a send to any other leaves it.
            provider TEXT NOT NULL,
            content TEXT NOT NULL,
            -- The newest transcript row when the switch happened.
            anchor_id INTEGER NOT NULL DEFAULT 0,
            -- Client nonce of the last turn that carried the seed.
            sent_nonce TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );",
    )
    .map_err(|e| format!("Failed to create provider switch table: {e}"))
}

/// A thread's switch seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSwitchSeed {
    pub content: String,
    pub anchor_id: i64,
    pub sent_nonce: Option<String>,
}

impl DatabaseStore {
    pub fn put_provider_switch_seed(
        &self,
        thread_id: &str,
        provider: &str,
        content: &str,
        anchor_id: i64,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_chat_provider_switch_seeds
                    (thread_id, provider, content, anchor_id, sent_nonce)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT(thread_id) DO UPDATE SET
                    provider = excluded.provider,
                    content = excluded.content,
                    anchor_id = excluded.anchor_id,
                    sent_nonce = NULL,
                    created_at = datetime('now')",
                params![thread_id, provider, content, anchor_id],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to save the provider switch context: {e}"))
    }

    /// The seed for `provider` on `thread_id`, if it is the thread's latest.
    pub fn provider_switch_seed(&self, thread_id: &str, provider: &str) -> Option<ProviderSwitchSeed> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT content, anchor_id, sent_nonce FROM agent_chat_provider_switch_seeds
                 WHERE thread_id = ?1 AND provider = ?2",
                params![thread_id, provider],
                |row| {
                    Ok(ProviderSwitchSeed {
                        content: row.get(0)?,
                        anchor_id: row.get(1)?,
                        sent_nonce: row.get(2)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Record the turn that carried the seed. Matches the content too, so a
    /// newer switch's seed is never the one marked.
    pub fn mark_provider_switch_seed_sent(
        &self,
        thread_id: &str,
        provider: &str,
        content: &str,
        nonce: &str,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE agent_chat_provider_switch_seeds SET sent_nonce = ?4
                 WHERE thread_id = ?1 AND provider = ?2 AND content = ?3",
                params![thread_id, provider, content, nonce],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to record the provider switch context: {e}"))
    }

    /// Whether the transcript after row `after_id` holds the user message
    /// sent with `nonce`.
    pub fn agent_chat_has_user_message_after(&self, thread_id: &str, nonce: &str, after_id: i64) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_chat_messages
                    WHERE thread_id = ?1 AND id > ?3 AND json_valid(payload)
                      AND json_extract(payload, '$.type') = 'user_message'
                      AND json_extract(payload, '$.client_nonce') = ?2)",
                params![thread_id, nonce, after_id],
                |row| row.get(0),
            )
            .unwrap_or(false)
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
    fn a_seed_waits_for_its_provider_and_remembers_the_turn_that_carried_it() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "so far", 4).unwrap();
        assert_eq!(db.provider_switch_seed("t", "claude"), None);
        let seed = db.provider_switch_seed("t", "codex").unwrap();
        assert_eq!((seed.content.as_str(), seed.anchor_id, seed.sent_nonce), ("so far", 4, None));
        db.mark_provider_switch_seed_sent("t", "codex", "stale", "n0").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex").unwrap().sent_nonce, None);
        db.mark_provider_switch_seed_sent("t", "codex", "so far", "n1").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex").unwrap().sent_nonce.as_deref(), Some("n1"));
    }

    #[test]
    fn a_later_switch_replaces_the_seed() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "first", 1).unwrap();
        db.mark_provider_switch_seed_sent("t", "codex", "first", "n1").unwrap();
        db.put_provider_switch_seed("t", "opencode", "second", 9).unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
        let seed = db.provider_switch_seed("t", "opencode").unwrap();
        assert_eq!((seed.anchor_id, seed.sent_nonce), (9, None));
    }

    #[test]
    fn finds_the_carrying_user_message_only_after_the_anchor() {
        let db = store();
        let row = |nonce: &str| {
            db.append_agent_chat_message(
                "t",
                &serde_json::json!({"type": "user_message", "text": "go", "client_nonce": nonce}).to_string(),
            )
            .unwrap()
            .unwrap()
        };
        let before = row("n1");
        assert!(!db.agent_chat_has_user_message_after("t", "n1", before));
        assert!(db.agent_chat_has_user_message_after("t", "n1", before - 1));
        row("n2");
        assert!(db.agent_chat_has_user_message_after("t", "n2", before));
        assert!(!db.agent_chat_has_user_message_after("t", "n3", 0));
    }

    #[test]
    fn deleting_the_chat_drops_its_seed() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "so far", 0).unwrap();
        db.delete_agent_chat_session("t").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
    }
}
