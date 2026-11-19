//! The layout journal is shared by all platforms. Unix commits it in the
//! same transaction as terminal daemon ownership.

use super::DatabaseStore;
use crate::state::{AppStateSnapshot, ArchivedWorkspaceSnapshot, LayoutCloseIntent, PendingArchive};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub(super) fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS pending_layout_closes (
            scope TEXT NOT NULL CHECK (scope IN ('pane', 'session', 'tab', 'workspace', 'archive')),
            workspace_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            payload TEXT CHECK ((scope = 'archive') = (payload IS NOT NULL)),
            PRIMARY KEY (scope, workspace_id, target_id)
        );
        CREATE TABLE IF NOT EXISTS retained_archive_chats (
            workspace_id TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            cwd TEXT NOT NULL,
            archive_id TEXT NOT NULL,
            archived_workspace_id TEXT NOT NULL,
            PRIMARY KEY(workspace_id, thread_id)
        ) WITHOUT ROWID;")
}

impl PendingArchive {
    fn from_payload(payload: &str, workspace_id: &str, archive_id: &str) -> Result<Self, String> {
        let archive: Self = serde_json::from_str(payload).map_err(|e| e.to_string())?;
        if archive.entry.workspace_id != workspace_id || archive.entry.archive_id != archive_id {
            return Err("Archive close payload has a different identity".into());
        }
        Ok(archive)
    }

    fn capture_history(&mut self, tx: &Transaction<'_>, legacy_replacement: Option<&str>) -> Result<(), String> {
        // Legacy intents moved rows eagerly without recording thread IDs. A
        // retained native binding can prove a moved thread's original owner.
        let mut stmt = tx.prepare(
            "SELECT s.thread_id FROM agent_chat_sessions s
             WHERE s.provider = 'hermes' AND s.cwd = ?2 AND
                (s.workspace_id = ?1 OR (s.workspace_id = ?3 AND EXISTS (
                    SELECT 1 FROM hermes_bindings b WHERE b.thread_id = s.thread_id
                    AND b.workspace_id = ?1 AND b.cwd = ?2)))
             ORDER BY s.thread_id",
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![self.entry.workspace_id, self.history_cwd(), legacy_replacement], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        self.hermes_thread_ids = Some(rows.collect::<rusqlite::Result<_>>().map_err(|e| e.to_string())?);
        Ok(())
    }

    fn history_cwd(&self) -> &str {
        self.entry.worktree_path.as_deref().unwrap_or(&self.entry.cwd)
    }

    fn settle_history(&mut self, tx: &Transaction<'_>, replacement_present: bool) -> Result<(), String> {
        let Some(replacement) = self.restored_workspace_id.clone() else { return Ok(()) };
        if replacement == self.entry.workspace_id { return Ok(()) }
        if self.hermes_thread_ids.is_none() {
            self.capture_history(tx, Some(&replacement))?;
            let payload = serde_json::to_string(self).map_err(|e| e.to_string())?;
            tx.execute("UPDATE pending_layout_closes SET payload=?3
                WHERE scope='archive' AND workspace_id=?1 AND target_id=?2",
                params![self.entry.workspace_id, self.entry.archive_id, payload]).map_err(|e| e.to_string())?;
        }
        let archived = self.entry.workspace_id.as_str();
        let (from, to) = if replacement_present {
            (archived, replacement.as_str())
        } else {
            (replacement.as_str(), archived)
        };
        let mut update = tx.prepare_cached(
            "UPDATE agent_chat_sessions SET workspace_id=?2
             WHERE workspace_id=?1 AND cwd=?3 AND provider='hermes' AND thread_id=?4",
        ).map_err(|e| e.to_string())?;
        let retain_transfer = crate::state::retained_layouts().protects(&replacement);
        for thread in self.hermes_thread_ids.iter().flatten() {
            if retain_transfer {
                // The close intent can be retired or superseded independently
                // of a saved layout that still names this exact replacement.
                tx.execute(
                    "INSERT OR REPLACE INTO retained_archive_chats
                     (workspace_id, thread_id, cwd, archive_id, archived_workspace_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![replacement, thread, self.history_cwd(), self.entry.archive_id, archived],
                ).map_err(|e| e.to_string())?;
            }
            update.execute(params![from, to, self.history_cwd(), thread]).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl LayoutCloseIntent {
    fn row_key(&self) -> (&str, &str, &str) {
        match self {
            Self::Pane(id) => ("pane", "", id),
            Self::Session {
                workspace_id,
                session_id,
            } => ("session", workspace_id, session_id),
            Self::Tab {
                workspace_id,
                tab_id,
            } => ("tab", workspace_id, tab_id),
            Self::Workspace(id) => ("workspace", id, ""),
            // Older workspace closes have an empty target. A consumed archive
            // closes the same workspace and also removes this exact entry.
            Self::ConsumedArchive { workspace_id, archive_id } => ("workspace", workspace_id, archive_id),
            Self::Archive(archive) => ("archive", &archive.entry.workspace_id, &archive.entry.archive_id),
        }
    }
}

pub(super) fn write_intent(
    tx: &Transaction<'_>,
    intent: &LayoutCloseIntent,
    payload: Option<&str>,
) -> Result<(), String> {
    let (scope, intent_workspace_id, target_id) = intent.row_key();
    tx.execute(
        "INSERT INTO pending_layout_closes (scope, workspace_id, target_id, payload)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![scope, intent_workspace_id, target_id, payload],
    )
    .map_err(|error| error.to_string())?;
    if let Some(id) = intent.closed_workspace_id() {
        // Once the user closes or rearchives the replacement, its absence
        // from a later snapshot must not bring the original archive back.
        let mut select = tx.prepare("SELECT workspace_id, target_id, payload FROM pending_layout_closes
            WHERE scope='archive' AND json_extract(payload, '$.restored_workspace_id') = ?1")
            .map_err(|e| e.to_string())?;
        let parents = select.query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        }).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
        drop(select);
        for (workspace, archive, payload) in parents {
            PendingArchive::from_payload(&payload, &workspace, &archive)?.settle_history(tx, true)?;
        }
        tx.execute(
            "UPDATE OR REPLACE pending_layout_closes SET scope = 'workspace', payload = NULL
             WHERE scope = 'archive' AND json_extract(payload, '$.restored_workspace_id') = ?1",
            [id],
        ).map_err(|error| error.to_string())?;
    }
    Ok(())
}

impl DatabaseStore {
    /// Follow a selected replacement even if the repaired primary was used
    /// (and the archive unarchived there again) between these two boots.
    pub(crate) fn settle_startup_chat_history(
        &self,
        intents: &[LayoutCloseIntent],
        snapshot: &AppStateSnapshot,
    ) -> Result<(), String> {
        let retained = crate::state::retained_layouts();
        let recovering = crate::state::layout_recovery_pending();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let transfers = {
            let mut select = tx.prepare(
                "SELECT r.workspace_id, r.thread_id, r.archive_id, r.archived_workspace_id, s.workspace_id
                 FROM retained_archive_chats r LEFT JOIN agent_chat_sessions s
                 ON s.thread_id=r.thread_id AND s.cwd=r.cwd AND s.provider='hermes'",
            ).map_err(|e| e.to_string())?;
            let rows = select.query_map([], |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?,
            ))).map_err(|e| e.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
        };
        let live: std::collections::HashSet<_> = snapshot.workspaces.iter().map(|ws| ws.workspace_id.0.as_str()).collect();
        for (workspace, thread, archive_id, archived_workspace, current_owner) in transfers {
            if current_owner.as_ref().is_some_and(|owner| !live.contains(owner.as_str())) {
                let restored_owner = if live.contains(workspace.as_str()) {
                    Some(&workspace)
                } else if snapshot.archived_workspaces.iter().any(|entry| entry.archive_id == archive_id) {
                    Some(&archived_workspace)
                } else {
                    None
                };
                if let Some(owner) = restored_owner {
                    tx.execute("UPDATE agent_chat_sessions SET workspace_id=?1 WHERE thread_id=?2 AND workspace_id<>?1",
                        params![owner, thread]).map_err(|e| e.to_string())?;
                }
            }
            if !recovering && !retained.protects(&workspace) {
                tx.execute("DELETE FROM retained_archive_chats WHERE workspace_id=?1 AND thread_id=?2",
                    params![workspace, thread]).map_err(|e| e.to_string())?;
            }
        }
        // A newer, still-pending unarchive takes precedence.
        Self::settle_closes(&tx, intents, snapshot, false)?;
        tx.commit().map_err(|e| e.to_string())
    }

    #[cfg(any(windows, test))]
    pub(crate) fn record_recovery_layout_close(&self, intent: &LayoutCloseIntent) -> Result<(), String> {
        let check_pending_parent = if crate::state::layout_recovery_pending()
            || crate::state::retained_layouts().references_close(intent)
        {
            None
        } else {
            let Some(workspace) = intent.closed_workspace_id() else { return Ok(()) };
            Some(workspace)
        };
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if let Some(workspace) = check_pending_parent {
            // Startup can have selected a durable primary while the
            // conditional unarchive is still waiting for acknowledgement.
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_layout_closes WHERE scope='archive'
                 AND json_extract(payload, '$.restored_workspace_id')=?1)",
                [workspace], |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if !pending { return Ok(()); }
        }
        let payload = match intent {
            LayoutCloseIntent::Archive(entry) => Some(serde_json::to_string(entry).map_err(|e| e.to_string())?),
            _ => None,
        };
        write_intent(&tx, intent, payload.as_deref())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub(crate) fn pending_layout_closes(&self) -> Result<Vec<LayoutCloseIntent>, String> {
        Self::read_layout_closes(&self.conn.lock().unwrap())
    }

    fn read_layout_closes(conn: &Connection) -> Result<Vec<LayoutCloseIntent>, String> {
        let mut select = conn
            .prepare(
                "SELECT scope, workspace_id, target_id, payload FROM pending_layout_closes ORDER BY rowid",
            )
            .map_err(|error| error.to_string())?;
        let rows = select
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (scope, workspace_id, target_id, payload) =
                row.map_err(|error| error.to_string())?;
            match scope.as_str() {
                "pane" => Ok(LayoutCloseIntent::Pane(target_id)),
                "session" => Ok(LayoutCloseIntent::Session {
                    workspace_id,
                    session_id: target_id,
                }),
                "tab" => Ok(LayoutCloseIntent::Tab {
                    workspace_id,
                    tab_id: target_id,
                }),
                "workspace" if target_id.is_empty() => Ok(LayoutCloseIntent::Workspace(workspace_id)),
                "workspace" => Ok(LayoutCloseIntent::ConsumedArchive { workspace_id, archive_id: target_id }),
                "archive" => {
                    let archive = PendingArchive::from_payload(
                        payload.as_deref().ok_or("Archive close payload is missing")?,
                        &workspace_id, &target_id,
                    )?;
                    Ok(LayoutCloseIntent::Archive(Box::new(archive)))
                }
                _ => Err(format!("Unknown layout close scope {scope}")),
            }
        })
        .collect()
    }

    /// Settle history and, once the final primary is durable, acknowledge the
    /// same intent atomically. A queued save may hold an older archive payload.
    pub(crate) fn settle_layout_closes(
        &self,
        intents: &[LayoutCloseIntent],
        snapshot: &AppStateSnapshot,
        retire: bool,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        Self::settle_closes(&tx, intents, snapshot, retire)?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn settle_closes(
        tx: &Transaction<'_>,
        intents: &[LayoutCloseIntent],
        snapshot: &AppStateSnapshot,
        retire: bool,
    ) -> Result<(), String> {
        let retained = crate::state::retained_layouts();
        #[cfg(unix)]
        let alias_owners = if retire && !retained.is_empty()
            && intents.iter().any(|intent| matches!(intent, LayoutCloseIntent::Session { .. }))
        {
            retained.terminal_owners(&Self::read_terminal_replacements(tx)?)
        } else {
            Default::default()
        };
        for intent in intents {
            if let LayoutCloseIntent::Archive(expected) = intent {
                let Some(mut current) = Self::read_pending_archive(tx, &expected.entry)? else { continue };
                if current.restored_workspace_id != expected.restored_workspace_id || current.entry != expected.entry {
                    continue;
                }
                current.settle_history(tx, current.replacement_in(snapshot))?;
            }
            #[cfg(unix)]
            if let LayoutCloseIntent::Session { workspace_id, session_id } = intent {
                if alias_owners.contains(&(workspace_id.clone(), session_id.clone())) {
                    continue;
                }
            }
            if retire && !retained.references_close(intent) {
                tx.execute(
                    "DELETE FROM pending_layout_closes WHERE scope = ?1 AND workspace_id = ?2 AND target_id = ?3",
                    intent.row_key(),
                ).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// A deletion is unconditional. An unarchive consumes the entry only in
    /// layouts that contain the exact workspace that replaced it.
    pub(crate) fn consume_recovery_archive(
        &self,
        entry: &ArchivedWorkspaceSnapshot,
        restored_workspace_id: Option<&str>,
    ) -> Result<Option<PendingArchive>, String> {
        if !crate::state::layout_recovery_pending() && !crate::state::retained_layouts().references_archive(entry) {
            return Ok(None);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        // Older retained archives displaced by this entry stay consumed.
        for intent in Self::read_layout_closes(&tx)? {
            if let LayoutCloseIntent::Archive(prior) = intent {
                if entry.supersedes_or_matches(&prior.entry)
                    && (restored_workspace_id.is_none() || prior.entry.archive_id != entry.archive_id)
                {
                    tx.execute(
                        "UPDATE OR REPLACE pending_layout_closes SET scope = 'workspace', payload = NULL
                         WHERE scope = 'archive' AND workspace_id = ?1 AND target_id = ?2",
                        params![prior.entry.workspace_id, prior.entry.archive_id],
                    ).map_err(|error| error.to_string())?;
                }
            }
        }
        let recorded = if let Some(id) = restored_workspace_id {
            let previous = Self::read_pending_archive(&tx, entry)?
                .filter(|prior| prior.restored_workspace_id.as_deref() == Some(id));
            let legacy = previous.is_some();
            let mut archive = previous.unwrap_or_else(|| PendingArchive {
                entry: entry.clone(), restored_workspace_id: Some(id.to_owned()), hermes_thread_ids: None,
            });
            if archive.hermes_thread_ids.is_none() {
                archive.capture_history(&tx, legacy.then_some(id))?;
            }
            let payload = serde_json::to_string(&archive).map_err(|error| error.to_string())?;
            // An explicit delete wins over a concurrent restore or a retry.
            let changed = tx.execute(
                "INSERT INTO pending_layout_closes (scope, workspace_id, target_id, payload)
                 SELECT 'archive', ?1, ?2, ?3 WHERE NOT EXISTS (
                    SELECT 1 FROM pending_layout_closes WHERE scope = 'workspace' AND workspace_id = ?1 AND target_id = ?2
                 ) ON CONFLICT(scope, workspace_id, target_id) DO UPDATE SET payload = excluded.payload",
                params![entry.workspace_id, entry.archive_id, payload],
            ).map_err(|error| error.to_string())?;
            (changed != 0).then_some(archive)
        } else {
            // The archive may have come from a layout with no journal row.
            tx.execute(
                "INSERT INTO pending_layout_closes (scope, workspace_id, target_id)
                 VALUES ('workspace', ?1, ?2) ON CONFLICT DO NOTHING",
                params![entry.workspace_id, entry.archive_id],
            ).map_err(|error| error.to_string())?;
            None
        };
        tx.commit().map_err(|error| error.to_string())?;
        Ok(recorded)
    }

    pub(crate) fn pending_archive(&self, entry: &ArchivedWorkspaceSnapshot) -> Result<Option<PendingArchive>, String> {
        Self::read_pending_archive(&self.conn.lock().unwrap(), entry)
    }

    fn read_pending_archive(conn: &Connection, entry: &ArchivedWorkspaceSnapshot) -> Result<Option<PendingArchive>, String> {
        let payload = conn.query_row(
            "SELECT payload FROM pending_layout_closes WHERE scope = 'archive' AND workspace_id = ?1 AND target_id = ?2",
            params![entry.workspace_id, entry.archive_id], |row| row.get::<_, String>(0),
        ).optional().map_err(|error| error.to_string())?;
        payload.map(|p| PendingArchive::from_payload(&p, &entry.workspace_id, &entry.archive_id)).transpose()
    }
}
