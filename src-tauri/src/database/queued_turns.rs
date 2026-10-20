//! The follow-up queue, saved so it survives an app restart.
//!
//! Provider queues live in memory. This table mirrors them from the event
//! stream (see `commands::queued_turns`): a row appears when a turn is
//! queued and goes when it dispatches or is cancelled, with edits and moves
//! applied in between. Each row records the `epoch` (one id per app run)
//! that queued it. A row from an earlier run is *saved*: no provider holds
//! it any more, and it waits for the user to send, edit, move or remove
//! it. Nothing sends a saved row on its own.
//!
//! A row carries what a send needs to replay it through the normal send
//! path: the user's `text`, the framed `prompt` the composer sent (before
//! skills are rendered), the selected skills, per-turn overrides, the
//! already-finalized image files and the client nonce. The nonce is the
//! turn's identity across a resend: a saved turn sent again keeps it.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_chat_queued_turns (
            thread_id TEXT NOT NULL
                REFERENCES agent_chat_sessions(thread_id) ON DELETE CASCADE,
            queued_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            text TEXT NOT NULL,
            prompt TEXT,
            skill_ids TEXT NOT NULL DEFAULT '[]',
            skill_text TEXT,
            include_plugins INTEGER NOT NULL DEFAULT 1,
            images TEXT NOT NULL DEFAULT '[]',
            client_nonce TEXT,
            model_override TEXT,
            effort_override TEXT,
            permission_mode_override TEXT,
            epoch TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (thread_id, queued_id)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_chat_queued_turns_order
            ON agent_chat_queued_turns(thread_id, position);
        "#,
    )
    .map_err(|e| format!("Failed to create queued turn schema: {e}"))
}

/// Hand a merged-away thread's saved queue to the thread its transcript
/// moved to, before the cascade would drop it. `filter` selects the source
/// rows and must bind the survivor's thread id as `?2`, as in
/// [`super::turn_diffs::move_to_thread`]. Images stay where they are until
/// the caller moves them (`commands::queued_turns::adopt_moved_images`).
pub(super) fn move_to_thread(
    conn: &Connection,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<(), String> {
    conn.execute(
        &format!("UPDATE OR IGNORE agent_chat_queued_turns SET thread_id = ?2 WHERE {filter}"),
        params,
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to migrate queued turns: {e}"))
}

/// An image attached to a queued turn, already in the thread's image dir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedTurnImage {
    pub path: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedTurnRecord {
    pub provenance: crate::agent_provider::message::MessageProvenance,
    pub thread_id: String,
    pub queued_id: String,
    pub position: i64,
    /// What the user typed and sees.
    pub text: String,
    /// The framed prompt the composer sent, before skills are rendered;
    /// `None` means `text`.
    pub prompt: Option<String>,
    pub skill_ids: Vec<String>,
    pub skill_text: Option<String>,
    pub include_plugins: bool,
    pub images: Vec<QueuedTurnImage>,
    pub client_nonce: Option<String>,
    pub model_override: Option<String>,
    pub effort_override: Option<String>,
    pub permission_mode_override: Option<String>,
    pub epoch: String,
}

const COLUMNS: &str = "thread_id, queued_id, position, text, prompt, images, client_nonce,
     model_override, effort_override, permission_mode_override, epoch, skill_ids, skill_text,
     include_plugins, provenance";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueuedTurnRecord> {
    let images: String = row.get(5)?;
    Ok(QueuedTurnRecord {
        provenance: serde_json::from_str(&row.get::<_,String>(14)?).unwrap_or_default(),
        thread_id: row.get(0)?,
        queued_id: row.get(1)?,
        position: row.get(2)?,
        text: row.get(3)?,
        prompt: row.get(4)?,
        images: serde_json::from_str(&images).unwrap_or_default(),
        client_nonce: row.get(6)?,
        model_override: row.get(7)?,
        effort_override: row.get(8)?,
        permission_mode_override: row.get(9)?,
        epoch: row.get(10)?,
        skill_ids: serde_json::from_str(&row.get::<_, String>(11)?).unwrap_or_default(),
        skill_text: row.get(12)?,
        include_plugins: row.get::<_, i64>(13)? != 0,
    })
}

fn insert(conn: &Connection, row: &QueuedTurnRecord) -> Result<bool, String> {
    let images = serde_json::to_string(&row.images).map_err(|e| e.to_string())?;
    let skill_ids = serde_json::to_string(&row.skill_ids).map_err(|e| e.to_string())?;
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO agent_chat_queued_turns ({COLUMNS})
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
             WHERE EXISTS (SELECT 1 FROM agent_chat_sessions WHERE thread_id = ?1)"
        ),
        params![
            row.thread_id,
            row.queued_id,
            row.position,
            row.text,
            row.prompt,
            images,
            row.client_nonce,
            row.model_override,
            row.effort_override,
            row.permission_mode_override,
            row.epoch,
            skill_ids,
            row.skill_text,
            row.include_plugins,
            serde_json::to_string(&row.provenance).map_err(|e|e.to_string())?,
        ],
    )
    .map(|changed| changed > 0)
    .map_err(|e| format!("Failed to save queued turn: {e}"))
}

impl DatabaseStore {
    /// Save a newly queued turn behind the thread's queue; `position` is
    /// ignored. A row with the same nonce is the same message queued again
    /// (a saved turn resent behind a running one), so it is replaced. A
    /// thread without a session row has nothing to resume, so it saves
    /// nothing. Returns whether a row was written.
    pub fn insert_queued_turn(&self, row: &QueuedTurnRecord) -> Result<bool, String> {
        self.replace_queued_turn(None, row)
    }

    /// [`insert_queued_turn`](Self::insert_queued_turn) that also removes
    /// `replaces`, in one transaction, so the message is always saved under
    /// one id or the other. When `row` has already been dispatched or
    /// cancelled (its disposition is journaled before the queue mirror runs),
    /// it is not saved: the later disposition wins over this settlement.
    pub fn replace_queued_turn(
        &self,
        replaces: Option<&str>,
        row: &QueuedTurnRecord,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start queued turn save: {e}"))?;
        if replaces.is_some() {
            let settled = tx
                .query_row(
                    "SELECT 1 FROM agent_chat_messages
                     WHERE thread_id = ?1
                       AND json_extract(payload, '$.type') = 'native_queue_control'
                       AND json_extract(payload, '$.queued_id') = ?2
                       AND json_extract(payload, '$.disposition') IN ('dispatched', 'cancelled')
                     LIMIT 1",
                    params![row.thread_id, row.queued_id],
                    |_| Ok(()),
                )
                .is_ok();
            if settled {
                if let Some(old) = replaces {
                    tx.execute(
                        "DELETE FROM agent_chat_queued_turns WHERE thread_id = ?1 AND queued_id = ?2",
                        params![row.thread_id, old],
                    )
                    .map_err(|e| format!("Failed to replace queued turn: {e}"))?;
                }
                tx.commit()
                    .map_err(|e| format!("Failed to commit queued turn save: {e}"))?;
                return Ok(false);
            }
        }
        if let Some(old) = replaces.filter(|old| *old != row.queued_id) {
            tx.execute(
                "DELETE FROM agent_chat_queued_turns WHERE thread_id = ?1 AND queued_id = ?2",
                params![row.thread_id, old],
            )
            .map_err(|e| format!("Failed to replace queued turn: {e}"))?;
        }
        if let Some(nonce) = &row.client_nonce {
            tx.execute(
                "DELETE FROM agent_chat_queued_turns
                 WHERE thread_id = ?1 AND client_nonce = ?2 AND queued_id != ?3",
                params![row.thread_id, nonce, row.queued_id],
            )
            .map_err(|e| format!("Failed to replace queued turn: {e}"))?;
        }
        let position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM agent_chat_queued_turns
                 WHERE thread_id = ?1",
                params![row.thread_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("Failed to read queued turn order: {e}"))?;
        let written = insert(&tx, &QueuedTurnRecord { position, ..row.clone() })?;
        tx.commit()
            .map_err(|e| format!("Failed to commit queued turn save: {e}"))?;
        Ok(written)
    }

    /// Put back a row taken by [`take_queued_turn`](Self::take_queued_turn),
    /// at its old position.
    pub fn restore_queued_turn(&self, row: &QueuedTurnRecord) -> Result<bool, String> {
        insert(&self.conn.lock().unwrap(), row)
    }

    /// A thread's saved queue, in dispatch order.
    pub fn list_queued_turns(&self, thread_id: &str) -> Vec<QueuedTurnRecord> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut statement) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_chat_queued_turns
             WHERE thread_id = ?1 ORDER BY position, created_at"
        )) else {
            return Vec::new();
        };
        statement
            .query_map(params![thread_id], row_to_record)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn get_queued_turn(&self, thread_id: &str, queued_id: &str) -> Option<QueuedTurnRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {COLUMNS} FROM agent_chat_queued_turns
                 WHERE thread_id = ?1 AND queued_id = ?2"
            ),
            params![thread_id, queued_id],
            row_to_record,
        )
        .ok()
    }

    /// Remove and return a row, so only one caller can send it.
    pub fn take_queued_turn(
        &self,
        thread_id: &str,
        queued_id: &str,
    ) -> Result<Option<QueuedTurnRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!(
                    "DELETE FROM agent_chat_queued_turns
                     WHERE thread_id = ?1 AND queued_id = ?2 RETURNING {COLUMNS}"
                ),
                params![thread_id, queued_id],
                row_to_record,
            )
            .optional()
            .map_err(|e| format!("Failed to take queued turn: {e}"))?;
        Ok(row)
    }

    /// Whether acceptance evidence confirms this logical message was sent.
    /// Legacy envelopes predate the ledger and retain their historical meaning.
    pub fn queued_turn_was_sent(&self, thread_id: &str, client_nonce: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM agent_chat_messages m
             WHERE thread_id = ?1 AND json_extract(m.payload, '$.type') = 'user_message'
               AND json_extract(m.payload, '$.client_nonce') = ?2
               AND (json_extract(m.payload,'$.delivery_id') IS NULL OR EXISTS (SELECT 1 FROM agent_message_deliveries d WHERE d.id=json_extract(m.payload,'$.delivery_id') AND d.state='accepted'))
             LIMIT 1",
            params![thread_id, client_nonce],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn delete_queued_turn(&self, thread_id: &str, queued_id: &str) -> Result<bool, String> {
        self.take_queued_turn(thread_id, queued_id).map(|row| row.is_some())
    }

    pub fn update_queued_turn_text(
        &self,
        thread_id: &str,
        queued_id: &str,
        text: &str,
        prompt: Option<&str>,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx=conn.transaction().map_err(|e|e.to_string())?;
        let changed=tx.execute(
            "UPDATE agent_chat_queued_turns SET text = ?3, prompt = ?4
             WHERE thread_id = ?1 AND queued_id = ?2",
            params![thread_id, queued_id, text, prompt],
        ).map_err(|e|format!("Failed to edit queued turn: {e}"))?;
        if changed>0 {
            // Keep immutable admission evidence and the current editable queue
            // input separately. A definite-no-send retry must use the edit.
            tx.execute("UPDATE agent_message_deliveries SET dispatch_payload=json_set(COALESCE(dispatch_payload,payload),'$.input.text',COALESCE(?4,?3),'$.input.display_text',?3)
                WHERE id=(SELECT json_extract(provenance,'$.delivery_id') FROM agent_chat_queued_turns WHERE thread_id=?1 AND queued_id=?2) AND attempted=0 AND state IN ('pending','preparing','queued')",
                params![thread_id,queued_id,text,prompt]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e|e.to_string())?;
        Ok(changed>0)
    }

    /// Point a row's images at their new files.
    pub fn set_queued_turn_images(
        &self,
        thread_id: &str,
        queued_id: &str,
        images: &[QueuedTurnImage],
    ) -> Result<(), String> {
        let images = serde_json::to_string(images).map_err(|e| e.to_string())?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_queued_turns SET images = ?3 WHERE thread_id = ?1 AND queued_id = ?2",
            params![thread_id, queued_id, images],
        )
        .map(drop)
        .map_err(|e| format!("Failed to move queued turn images: {e}"))
    }

    /// Every saved queue row, for repairs that span threads.
    pub fn all_queued_turns(&self) -> Vec<QueuedTurnRecord> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut statement) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_chat_queued_turns ORDER BY thread_id, position"
        )) else {
            return Vec::new();
        };
        statement
            .query_map([], row_to_record)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Put `queued_ids` in this order. They swap the positions they already
    /// hold, so rows not listed keep theirs.
    pub fn reorder_queued_turns(&self, thread_id: &str, queued_ids: &[String]) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start queue reorder: {e}"))?;
        let mut slots = Vec::new();
        let mut present = Vec::new();
        for id in queued_ids {
            let position: Option<i64> = tx
                .query_row(
                    "SELECT position FROM agent_chat_queued_turns
                     WHERE thread_id = ?1 AND queued_id = ?2",
                    params![thread_id, id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("Failed to read queued turn order: {e}"))?;
            if let Some(position) = position {
                slots.push(position);
                present.push(id);
            }
        }
        slots.sort_unstable();
        for (id, position) in present.into_iter().zip(slots) {
            tx.execute(
                "UPDATE agent_chat_queued_turns SET position = ?3
                 WHERE thread_id = ?1 AND queued_id = ?2",
                params![thread_id, id, position],
            )
            .map_err(|e| format!("Failed to reorder queued turns: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit queue reorder: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(thread: &str, id: &str, epoch: &str) -> QueuedTurnRecord {
        QueuedTurnRecord {
            provenance: crate::agent_provider::message::MessageProvenance {
                message_id:Some(format!("message-{id}")),created_by:crate::agent_provider::message::CreatedBy::Agent,
                creation_source:crate::agent_provider::message::CreationSource::Provider,
                sender_thread_id:Some("sender".into()),sender_title:Some("Review agent".into()),
                input_intent:crate::agent_provider::message::InputIntent::QueuedTurn,..Default::default()
            },
            thread_id: thread.into(),
            queued_id: id.into(),
            position: 0,
            text: format!("text {id}"),
            prompt: Some(format!("<mode/>\n\ntext {id}")),
            images: vec![QueuedTurnImage {
                path: "/tmp/a.png".into(),
                media_type: "image/png".into(),
            }],
            client_nonce: Some(format!("nonce-{id}")),
            model_override: None,
            effort_override: Some("high".into()),
            permission_mode_override: None,
            epoch: epoch.into(),
            skill_ids: vec!["skill-a".into()],
            skill_text: None,
            include_plugins: false,
        }
    }

    fn ids(db: &DatabaseStore, thread: &str) -> Vec<String> {
        db.list_queued_turns(thread).into_iter().map(|r| r.queued_id).collect()
    }

    #[test]
    fn orchestration_message_provenance_survives_queue_edit_and_saved_replay() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "w", None, "claude").unwrap();
        for id in ["a", "b", "c"] {
            assert!(db.insert_queued_turn(&row("t", id, "e1")).unwrap());
        }
        // Re-inserting an id is a no-op; an unknown thread saves nothing.
        assert!(!db.insert_queued_turn(&row("t", "a", "e1")).unwrap());
        assert!(!db.insert_queued_turn(&row("missing", "z", "e1")).unwrap());
        assert_eq!(ids(&db, "t"), ["a", "b", "c"]);
        let saved = db.get_queued_turn("t", "b").unwrap();
        assert_eq!(saved.images.len(), 1);
        assert_eq!(saved.effort_override.as_deref(), Some("high"));
        assert_eq!((saved.skill_ids.as_slice(), saved.include_plugins), (&["skill-a".to_string()][..], false));

        db.reorder_queued_turns("t", &["c".into(), "gone".into(), "a".into()]).unwrap();
        assert_eq!(ids(&db, "t"), ["c", "b", "a"]);

        assert!(db.update_queued_turn_text("t", "b", "new", Some("<mode/>\n\nnew")).unwrap());
        assert_eq!(db.get_queued_turn("t", "b").unwrap().text, "new");
        assert_eq!(db.get_queued_turn("t", "b").unwrap().provenance,row("t","b","e1").provenance);

        let taken = db.take_queued_turn("t", "b").unwrap().unwrap();
        assert_eq!(db.take_queued_turn("t", "b").unwrap(), None);
        assert_eq!(ids(&db, "t"), ["c", "a"]);
        db.restore_queued_turn(&taken).unwrap();
        assert_eq!(ids(&db, "t"), ["c", "b", "a"]);

        assert!(db.delete_queued_turn("t", "c").unwrap());
        assert!(!db.delete_queued_turn("t", "c").unwrap());
        assert_eq!(ids(&db, "t"), ["b", "a"]);

        // The same message queued again under a new id replaces its row.
        let resent = QueuedTurnRecord { queued_id: "b2".into(), ..row("t", "b", "e2") };
        assert!(db.insert_queued_turn(&resent).unwrap());
        assert_eq!(ids(&db, "t"), ["a", "b2"]);
        assert_eq!(db.get_queued_turn("t","b2").unwrap().provenance,row("t","b","e1").provenance);
        // A resend replaces the saved row it came from, whatever its nonce.
        let moved = QueuedTurnRecord { queued_id: "a2".into(), client_nonce: None, ..row("t", "a", "e2") };
        assert!(db.replace_queued_turn(Some("a"), &moved).unwrap());
        assert_eq!(ids(&db, "t"), ["b2", "a2"]);
        // The bridge mirroring it afterwards changes nothing.
        assert!(!db.insert_queued_turn(&moved).unwrap());
        assert_eq!(ids(&db, "t"), ["b2", "a2"]);
    }

    #[test]
    fn a_settled_resend_is_not_saved_again() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "w", None, "claude").unwrap();
        db.insert_queued_turn(&row("t", "saved", "e1")).unwrap();
        // The resend's queued copy was cancelled before its send returned.
        let cancelled = crate::agent_provider::ProviderRuntimeEvent::QueuedTurnCancelled {
            thread_id: crate::agent_provider::ThreadId("t".into()),
            queued_id: "live".into(),
        };
        db.control_record_queue_event(&cancelled).unwrap();
        let live = QueuedTurnRecord { queued_id: "live".into(), ..row("t", "saved", "e2") };
        assert!(!db.replace_queued_turn(Some("saved"), &live).unwrap());
        assert!(ids(&db, "t").is_empty());
    }

    #[test]
    fn merging_sessions_keeps_the_saved_queue() {
        let db = init_test_database();
        db.upsert_agent_chat_session("old", "w", None, "claude").unwrap();
        db.upsert_agent_chat_session("new", "w", None, "claude").unwrap();
        db.set_agent_chat_sdk_session_id("old", "sdk-1").unwrap();
        db.set_agent_chat_sdk_session_id("new", "sdk-1").unwrap();
        db.insert_queued_turn(&row("old", "q", "e1")).unwrap();
        db.collapse_duplicate_agent_chat_sessions("sdk-1").unwrap();
        let survivor = if db.get_agent_chat_session("new").is_some() { "new" } else { "old" };
        assert_eq!(ids(&db, survivor), ["q"]);
    }

    #[test]
    fn a_nonce_in_the_transcript_means_the_turn_was_sent() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "w", None, "claude").unwrap();
        assert!(!db.queued_turn_was_sent("t", "n1"));
        db.append_agent_chat_message("t", &serde_json::json!({"type":"user_message","text":"hi","client_nonce":"n1"}).to_string()).unwrap();
        assert!(db.queued_turn_was_sent("t", "n1"));
        assert!(!db.queued_turn_was_sent("t", "n2"));
    }
}
