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

/// How a seed names the chat whose full history it points at.
pub(crate) fn history_ref(thread_id: &str) -> String {
    format!("conversation_id `{thread_id}`")
}

/// `seed` with its history pointer moved from `from` to `to`. Only the
/// header Codemux wrote is touched, never the conversation in the body.
pub(crate) fn retarget_seed(seed: &str, from: &str, to: &str) -> String {
    let (header, body) = seed.split_once("\n\n").unwrap_or((seed, ""));
    let header = header.replacen(&history_ref(from), &history_ref(to), 1);
    if body.is_empty() {
        header
    } else {
        format!("{header}\n\n{body}")
    }
}

/// A history reopen merges `merged` into `survivor`
/// (`merge_agent_chat_session_rows`). Unless the survivor has a seed of its
/// own, it takes the newest of theirs as it stood, delivery state included,
/// with its history pointer moved to the survivor: the merged ids are gone.
pub(super) fn move_to_thread(conn: &Connection, merged: &[String], survivor: &str) -> Result<(), String> {
    let read_error = |e: rusqlite::Error| format!("Failed to read the provider switch context: {e}");
    let own: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_chat_provider_switch_seeds WHERE thread_id = ?1)",
            params![survivor],
            |row| row.get(0),
        )
        .map_err(read_error)?;
    if own {
        return Ok(());
    }
    let mut newest: Option<(&String, String, String, i64, Option<String>, String)> = None;
    for thread in merged {
        let seed = conn
            .query_row(
                "SELECT provider, content, anchor_id, sent_nonce, created_at
                 FROM agent_chat_provider_switch_seeds WHERE thread_id = ?1",
                params![thread],
                |row| Ok((thread, row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(read_error)?;
        if let Some(seed) = seed.filter(|seed| newest.as_ref().map_or(true, |n| seed.5 > n.5)) {
            newest = Some(seed);
        }
    }
    let Some((from, provider, content, anchor_id, sent_nonce, created_at)) = newest else {
        return Ok(());
    };
    conn.execute(
        "INSERT INTO agent_chat_provider_switch_seeds
            (thread_id, provider, content, anchor_id, sent_nonce, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![survivor, provider, retarget_seed(&content, from, survivor), anchor_id, sent_nonce, created_at],
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to keep the provider switch context: {e}"))
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

    /// Whether the turn sent with `nonce` reached the provider: its user
    /// message is in the transcript after row `after_id`, and a completed
    /// item follows it. A turn stopped or rejected before the provider took
    /// it leaves no reply behind.
    pub fn agent_chat_turn_answered_after(&self, thread_id: &str, nonce: &str, after_id: i64) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_chat_messages u
                    WHERE u.thread_id = ?1 AND u.id > ?3 AND json_valid(u.payload)
                      AND json_extract(u.payload, '$.type') = 'user_message'
                      AND json_extract(u.payload, '$.client_nonce') = ?2
                      AND EXISTS(SELECT 1 FROM agent_chat_messages r
                          WHERE r.thread_id = u.thread_id AND r.id > u.id AND json_valid(r.payload)
                            AND json_extract(r.payload, '$.type') = 'item_completed'))",
                params![thread_id, nonce, after_id],
                |row| row.get(0),
            )
            .unwrap_or(false)
    }

    /// A thread's seed and the provider it is for.
    pub fn provider_switch_seed_of(&self, thread_id: &str) -> Option<(String, ProviderSwitchSeed)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT provider, content, anchor_id, sent_nonce FROM agent_chat_provider_switch_seeds
                 WHERE thread_id = ?1",
                params![thread_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        ProviderSwitchSeed {
                            content: row.get(1)?,
                            anchor_id: row.get(2)?,
                            sent_nonce: row.get(3)?,
                        },
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn a_turn_counts_as_answered_once_a_reply_follows_it_after_the_anchor() {
        let db = store();
        let append = |thread: &str, payload: serde_json::Value| {
            db.append_agent_chat_message(thread, &payload.to_string()).unwrap().unwrap()
        };
        let user = |nonce: &str| json!({"type": "user_message", "text": "go", "client_nonce": nonce});
        let reply = json!({"type": "item_completed"});
        let first = append("t", user("n1"));
        assert!(!db.agent_chat_turn_answered_after("t", "n1", 0), "no reply yet");
        append("t", reply.clone());
        assert!(db.agent_chat_turn_answered_after("t", "n1", 0));
        assert!(!db.agent_chat_turn_answered_after("t", "n1", first), "before the anchor");
        assert!(!db.agent_chat_turn_answered_after("t", "n2", 0));
    }

    #[test]
    fn reads_a_seed_with_its_provider() {
        let db = store();
        assert_eq!(db.provider_switch_seed_of("t"), None);
        db.put_provider_switch_seed("t", "codex", "SEED", 3).unwrap();
        let (provider, seed) = db.provider_switch_seed_of("t").unwrap();
        assert_eq!((provider.as_str(), seed.content.as_str(), seed.anchor_id), ("codex", "SEED", 3));
    }

    /// `old` and its history reopen `new`, sharing one native session.
    fn reopened(db: &DatabaseStore) {
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('old', 'sdk-1', 'ws', 'codex', '2025-01-01', '2025-01-01'),
                        ('new', 'sdk-1', 'ws', 'codex', '2025-03-01', '2025-03-01')",
            )
            .unwrap();
    }

    #[test]
    fn reopening_from_history_keeps_the_seed_and_where_it_stands() {
        let db = DatabaseStore::new_in_memory();
        reopened(&db);
        let seed = "<codemux-provider-handoff>\nSee conversation_id `old`.\n</codemux-provider-handoff>\n\nuser: conversation_id `old`";
        db.put_provider_switch_seed("old", "codex", seed, 7).unwrap();
        db.mark_provider_switch_seed_sent("old", "codex", seed, "n1").unwrap();
        db.collapse_duplicate_agent_chat_sessions("sdk-1").unwrap();
        let moved = db.provider_switch_seed("new", "codex").unwrap();
        assert_eq!(
            moved.content,
            "<codemux-provider-handoff>\nSee conversation_id `new`.\n</codemux-provider-handoff>\n\nuser: conversation_id `old`",
            "only the header's history pointer follows the chat"
        );
        assert_eq!((moved.anchor_id, moved.sent_nonce.as_deref()), (7, Some("n1")));
    }

    #[test]
    fn reopening_keeps_a_seed_the_survivor_already_has() {
        let db = DatabaseStore::new_in_memory();
        reopened(&db);
        db.put_provider_switch_seed("old", "codex", "older", 1).unwrap();
        db.put_provider_switch_seed("new", "claude", "newer", 2).unwrap();
        db.collapse_duplicate_agent_chat_sessions("sdk-1").unwrap();
        let (provider, seed) = db.provider_switch_seed_of("new").unwrap();
        assert_eq!((provider.as_str(), seed.content.as_str()), ("claude", "newer"));
    }

    #[test]
    fn deleting_the_chat_drops_its_seed() {
        let db = store();
        db.put_provider_switch_seed("t", "codex", "so far", 0).unwrap();
        db.delete_agent_chat_session("t").unwrap();
        assert_eq!(db.provider_switch_seed("t", "codex"), None);
    }
}
