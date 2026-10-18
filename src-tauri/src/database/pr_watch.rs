//! Persistence for chats that watch a pull request (`crate::pr_watch`).
//!
//! One watch per chat thread. `pr_watch_seen` remembers every check run,
//! comment, review and conflict the chat was already told about, so a
//! restart never wakes it twice for the same event. Deliberately no foreign
//! keys, like the delegated-task journal: the sweep drops a watch whose
//! session row is gone.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pr_watches (
            thread_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            pr_number INTEGER NOT NULL,
            pr_url TEXT,
            status TEXT NOT NULL,
            generation INTEGER NOT NULL,
            max_attempts INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            comment_streak INTEGER NOT NULL DEFAULT 0,
            head_sha TEXT,
            baselined INTEGER NOT NULL DEFAULT 0,
            failure_pending_since_ms INTEGER,
            last_checked_at_ms INTEGER,
            last_wake_at_ms INTEGER,
            last_wake_summary TEXT,
            last_error TEXT,
            stop_reason TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS pr_watch_seen (
            thread_id TEXT NOT NULL,
            event_key TEXT NOT NULL,
            seen_at_ms INTEGER NOT NULL,
            PRIMARY KEY (thread_id, event_key)
        );",
    )
    .map_err(|e| format!("Failed to create PR watch tables: {e}"))
}

/// `active` sweeps; `exhausted` and `ended` are kept so the UI can say why
/// watching stopped, until the user re-arms or unwatches.
pub const PR_WATCH_ACTIVE: &str = "active";
pub const PR_WATCH_EXHAUSTED: &str = "exhausted";
pub const PR_WATCH_ENDED: &str = "ended";

/// One chat's watch on one pull request. Serialized as the status the
/// frontend reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrWatchRecord {
    pub thread_id: String,
    pub workspace_id: String,
    pub pr_number: u32,
    pub pr_url: Option<String>,
    pub status: String,
    /// Fresh random token on every (re-)arm. A sweep only writes back, and
    /// only wakes, when the generation it read is still current, so an
    /// unwatch or re-arm racing a sweep always wins.
    #[serde(skip)]
    pub generation: i64,
    /// Automatic fix attempts (failing checks or a conflict) allowed before
    /// watching stops.
    pub max_attempts: u32,
    /// Fix attempts spent since checks were last all green.
    pub attempts: u32,
    /// Comment-only wakes since the head last moved.
    pub comment_streak: u32,
    pub head_sha: Option<String>,
    /// False until the first sweep recorded the conversation that predates
    /// the watch, which never wakes the chat.
    #[serde(skip)]
    pub baselined: bool,
    #[serde(skip)]
    pub failure_pending_since_ms: Option<i64>,
    pub last_checked_at_ms: Option<i64>,
    pub last_wake_at_ms: Option<i64>,
    pub last_wake_summary: Option<String>,
    pub last_error: Option<String>,
    pub stop_reason: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

const SELECT_COLUMNS: &str = "thread_id, workspace_id, pr_number, pr_url, status, generation,
    max_attempts, attempts, comment_streak, head_sha, baselined, failure_pending_since_ms,
    last_checked_at_ms, last_wake_at_ms, last_wake_summary, last_error, stop_reason,
    created_at_ms, updated_at_ms";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrWatchRecord> {
    Ok(PrWatchRecord {
        thread_id: row.get(0)?,
        workspace_id: row.get(1)?,
        pr_number: row.get(2)?,
        pr_url: row.get(3)?,
        status: row.get(4)?,
        generation: row.get(5)?,
        max_attempts: row.get(6)?,
        attempts: row.get(7)?,
        comment_streak: row.get(8)?,
        head_sha: row.get(9)?,
        baselined: row.get::<_, i64>(10)? != 0,
        failure_pending_since_ms: row.get(11)?,
        last_checked_at_ms: row.get(12)?,
        last_wake_at_ms: row.get(13)?,
        last_wake_summary: row.get(14)?,
        last_error: row.get(15)?,
        stop_reason: row.get(16)?,
        created_at_ms: row.get(17)?,
        updated_at_ms: row.get(18)?,
    })
}

fn pr_watch(conn: &Connection, thread_id: &str) -> Result<Option<PrWatchRecord>, String> {
    conn.query_row(
        &format!("SELECT {SELECT_COLUMNS} FROM pr_watches WHERE thread_id = ?1"),
        params![thread_id],
        row_to_record,
    )
    .optional()
    .map_err(|e| format!("Failed to read PR watch: {e}"))
}

impl DatabaseStore {
    /// Start watching, or re-arm an existing watch: counters, errors and the
    /// stop reason reset. Events already told about stay seen unless the
    /// chat now watches a different pull request.
    pub fn arm_pr_watch(
        &self,
        thread_id: &str,
        workspace_id: &str,
        pr_number: u32,
        pr_url: Option<&str>,
        max_attempts: u32,
        now_ms: i64,
    ) -> Result<PrWatchRecord, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to open PR watch transaction: {e}"))?;
        let existing = pr_watch(&tx, thread_id)?;
        let same_pr = existing
            .as_ref()
            .is_some_and(|w| w.pr_number == pr_number && w.pr_url.as_deref() == pr_url);
        if !same_pr {
            tx.execute("DELETE FROM pr_watch_seen WHERE thread_id = ?1", params![thread_id])
                .map_err(|e| format!("Failed to reset PR watch history: {e}"))?;
        }
        // Random, not a counter: unwatch deletes the row, and a counter
        // restarting at 1 would let a sweep that read the old watch pass
        // the generation check against the new one.
        let generation = loop {
            let token = uuid::Uuid::new_v4().as_u64_pair().0 as i64;
            if existing.as_ref().map_or(true, |w| w.generation != token) {
                break token;
            }
        };
        let created_at_ms = existing.as_ref().map_or(now_ms, |w| w.created_at_ms);
        // A different pull request starts from nothing; the same one keeps
        // its baseline and head so re-arming does not replay history.
        let (head_sha, baselined) = match existing.as_ref().filter(|_| same_pr) {
            Some(w) => (w.head_sha.clone(), w.baselined),
            None => (None, false),
        };
        tx.execute(
            "INSERT OR REPLACE INTO pr_watches
                 (thread_id, workspace_id, pr_number, pr_url, status, generation, max_attempts,
                  attempts, comment_streak, head_sha, baselined, failure_pending_since_ms,
                  last_checked_at_ms, last_wake_at_ms, last_wake_summary, last_error, stop_reason,
                  created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8, ?9, NULL, NULL, NULL, NULL, NULL, NULL, ?10, ?11)",
            params![
                thread_id,
                workspace_id,
                pr_number,
                pr_url,
                PR_WATCH_ACTIVE,
                generation,
                max_attempts,
                head_sha,
                baselined as i64,
                created_at_ms,
                now_ms
            ],
        )
        .map_err(|e| format!("Failed to save PR watch: {e}"))?;
        let record = pr_watch(&tx, thread_id)?.ok_or("PR watch vanished while arming")?;
        tx.commit()
            .map_err(|e| format!("Failed to commit PR watch: {e}"))?;
        Ok(record)
    }

    pub fn get_pr_watch(&self, thread_id: &str) -> Result<Option<PrWatchRecord>, String> {
        pr_watch(&self.conn.lock().unwrap(), thread_id)
    }

    pub fn active_pr_watches(&self) -> Result<Vec<PrWatchRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM pr_watches WHERE status = ?1 ORDER BY created_at_ms, thread_id"
            ))
            .map_err(|e| format!("Failed to list PR watches: {e}"))?;
        let rows = stmt
            .query_map(params![PR_WATCH_ACTIVE], row_to_record)
            .map_err(|e| format!("Failed to list PR watches: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read PR watch: {e}"))
    }

    /// Remove a watch and its history. Returns whether one existed.
    pub fn delete_pr_watch(&self, thread_id: &str) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to open PR watch transaction: {e}"))?;
        tx.execute("DELETE FROM pr_watch_seen WHERE thread_id = ?1", params![thread_id])
            .map_err(|e| format!("Failed to clear PR watch history: {e}"))?;
        let removed = tx
            .execute("DELETE FROM pr_watches WHERE thread_id = ?1", params![thread_id])
            .map_err(|e| format!("Failed to delete PR watch: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit PR watch removal: {e}"))?;
        Ok(removed > 0)
    }

    pub fn pr_watch_seen_keys(&self, thread_id: &str) -> Result<HashSet<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT event_key FROM pr_watch_seen WHERE thread_id = ?1")
            .map_err(|e| format!("Failed to read PR watch history: {e}"))?;
        let rows = stmt
            .query_map(params![thread_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to read PR watch history: {e}"))?;
        rows.collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("Failed to read PR watch history: {e}"))
    }

    /// Write a sweep's outcome and mark `seen` events told, atomically, but
    /// only if `record.generation` is still current. `false` means the watch
    /// was removed or re-armed meanwhile and nothing was written — the
    /// caller must then not wake the chat either.
    pub fn save_pr_watch_progress(
        &self,
        record: &PrWatchRecord,
        seen: &[String],
        now_ms: i64,
    ) -> Result<bool, String> {
        self.write_pr_watch(record, seen, &[], now_ms)
    }

    /// Like [`Self::save_pr_watch_progress`], but forgets `untold` events
    /// instead: a wake that never reached the chat hands them back.
    pub fn save_pr_watch_progress_unmarking(
        &self,
        record: &PrWatchRecord,
        untold: &[String],
        now_ms: i64,
    ) -> Result<bool, String> {
        self.write_pr_watch(record, &[], untold, now_ms)
    }

    fn write_pr_watch(
        &self,
        record: &PrWatchRecord,
        seen: &[String],
        untold: &[String],
        now_ms: i64,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to open PR watch transaction: {e}"))?;
        let updated = tx
            .execute(
                "UPDATE pr_watches SET
                     pr_url = ?3, status = ?4, attempts = ?5, comment_streak = ?6, head_sha = ?7,
                     baselined = ?8, failure_pending_since_ms = ?9, last_checked_at_ms = ?10,
                     last_wake_at_ms = ?11, last_wake_summary = ?12, last_error = ?13,
                     stop_reason = ?14, updated_at_ms = ?15
                 WHERE thread_id = ?1 AND generation = ?2",
                params![
                    record.thread_id,
                    record.generation,
                    record.pr_url,
                    record.status,
                    record.attempts,
                    record.comment_streak,
                    record.head_sha,
                    record.baselined as i64,
                    record.failure_pending_since_ms,
                    record.last_checked_at_ms,
                    record.last_wake_at_ms,
                    record.last_wake_summary,
                    record.last_error,
                    record.stop_reason,
                    now_ms
                ],
            )
            .map_err(|e| format!("Failed to save PR watch progress: {e}"))?;
        if updated == 0 {
            return Ok(false);
        }
        for key in seen {
            tx.execute(
                "INSERT OR IGNORE INTO pr_watch_seen (thread_id, event_key, seen_at_ms)
                 VALUES (?1, ?2, ?3)",
                params![record.thread_id, key, now_ms],
            )
            .map_err(|e| format!("Failed to record PR watch event: {e}"))?;
        }
        for key in untold {
            tx.execute(
                "DELETE FROM pr_watch_seen WHERE thread_id = ?1 AND event_key = ?2",
                params![record.thread_id, key],
            )
            .map_err(|e| format!("Failed to hand back PR watch event: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit PR watch progress: {e}"))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_watch_arm_rearm_and_switch_pr() {
        let db = DatabaseStore::new_in_memory();
        let first = db.arm_pr_watch("t1", "ws", 7, Some("u"), 3, 10).unwrap();
        assert_eq!(first.status, PR_WATCH_ACTIVE);

        let mut progress = first.clone();
        progress.attempts = 2;
        progress.status = PR_WATCH_EXHAUSTED.into();
        progress.baselined = true;
        progress.head_sha = Some("abc".into());
        assert!(db
            .save_pr_watch_progress(&progress, &["comment:1".into()], 20)
            .unwrap());

        // Re-arming the same PR resets counters but keeps history.
        let rearmed = db.arm_pr_watch("t1", "ws", 7, Some("u"), 5, 30).unwrap();
        assert_ne!(rearmed.generation, first.generation);
        assert_eq!(rearmed.attempts, 0);
        assert_eq!(rearmed.max_attempts, 5);
        assert_eq!(rearmed.status, PR_WATCH_ACTIVE);
        assert!(rearmed.baselined);
        assert_eq!(rearmed.created_at_ms, 10);
        assert!(db.pr_watch_seen_keys("t1").unwrap().contains("comment:1"));
        assert_eq!(db.active_pr_watches().unwrap().len(), 1);

        // A different PR starts over.
        let switched = db.arm_pr_watch("t1", "ws", 8, None, 3, 40).unwrap();
        assert!(!switched.baselined);
        assert_eq!(switched.head_sha, None);
        assert!(db.pr_watch_seen_keys("t1").unwrap().is_empty());
    }

    #[test]
    fn pr_watch_stale_generation_writes_nothing() {
        let db = DatabaseStore::new_in_memory();
        let stale = db.arm_pr_watch("t1", "ws", 7, None, 3, 10).unwrap();
        db.arm_pr_watch("t1", "ws", 7, None, 3, 20).unwrap();
        assert!(!db
            .save_pr_watch_progress(&stale, &["check:x".into()], 30)
            .unwrap());
        assert!(db.pr_watch_seen_keys("t1").unwrap().is_empty());

        // A wake that never reached the chat hands its events back.
        let current = db.get_pr_watch("t1").unwrap().unwrap();
        assert!(db
            .save_pr_watch_progress(&current, &["check:a".into(), "comment:b".into()], 31)
            .unwrap());
        assert!(db
            .save_pr_watch_progress_unmarking(&current, &["check:a".into()], 32)
            .unwrap());
        assert_eq!(
            db.pr_watch_seen_keys("t1").unwrap(),
            HashSet::from(["comment:b".to_string()])
        );

        assert!(db.delete_pr_watch("t1").unwrap());
        assert!(!db.delete_pr_watch("t1").unwrap());
        assert!(db.get_pr_watch("t1").unwrap().is_none());
        assert!(!db.save_pr_watch_progress(&stale, &[], 40).unwrap());

        // Stop, then start again: a sweep that read the first watch still
        // cannot write to the replacement.
        let replacement = db.arm_pr_watch("t1", "ws", 8, None, 3, 50).unwrap();
        assert_ne!(replacement.generation, stale.generation);
        assert!(!db.save_pr_watch_progress(&stale, &["check:x".into()], 60).unwrap());
        assert_eq!(db.get_pr_watch("t1").unwrap().unwrap().pr_number, 8);
    }
}
