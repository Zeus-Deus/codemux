//! Transactional provenance and crash-recovery journal for read-only imports.
use super::*;
use crate::local_session_import::{ImportResponse, ImportedSession, ParsedSession};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS agent_chat_local_imports (
 source_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL UNIQUE, provider TEXT NOT NULL,
 workspace_id TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL,
 pending_layout INTEGER NOT NULL DEFAULT 1
);";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppStateStore;
    fn fixture() -> ParsedSession {
        ParsedSession {
            source_id: "11111111-1111-4111-8111-111111111111".into(),
            provider: "claude".into(),
            cwd: "/synthetic/project".into(),
            last_active_at: "2026-09-29T10:00:00Z".into(),
            messages: vec![
                ("user".into(), "Synthetic prompt".into()),
                ("assistant".into(), "Synthetic answer".into()),
            ],
        }
    }
    #[test]
    fn local_import_sql_failure_rolls_back_workspace_and_all_rows() {
        let db = DatabaseStore::new_in_memory();
        let state = AppStateStore::default();
        state.clear_workspaces();
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_import BEFORE INSERT ON agent_chat_messages BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        let before = serde_json::to_value(state.snapshot()).unwrap();
        let a = fixture();
        assert!(state.import_local_sessions(&db, &[a.clone()]).is_err());
        assert_eq!(serde_json::to_value(state.snapshot()).unwrap(), before);
        assert!(db.get_agent_chat_session(&a.thread_id()).is_none());
        assert!(db.local_import_source_ids().unwrap().is_empty());
    }
    #[test]
    fn local_import_concurrent_import_commits_exactly_one_snapshot() {
        let db = std::sync::Arc::new(DatabaseStore::new_in_memory());
        let state = std::sync::Arc::new(AppStateStore::default());
        state.clear_workspaces();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers = (0..4)
            .map(|_| {
                let db = db.clone();
                let state = state.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    state.import_local_sessions(&db, &[fixture()]).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().map(|r| r.imported.len()).sum::<usize>(), 1);
        assert_eq!(results.iter().map(|r| r.skipped).sum::<usize>(), 3);
        assert_eq!(state.snapshot().workspaces.len(), 1);
        assert_eq!(state.snapshot().workspaces[0].surfaces.len(), 1);
        assert_eq!(db.list_agent_chat_messages(&fixture().thread_id()).len(), 3);
    }
    #[test]
    fn local_import_restart_preserves_dedupe_and_does_not_overwrite_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.db");
        let state = AppStateStore::default();
        state.clear_workspaces();
        let a = fixture();
        {
            let conn = open_connection(&path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };
            state.import_local_sessions(&db, &[a.clone()]).unwrap();
        }
        let conn = open_connection(&path).unwrap();
        create_schema(&conn).unwrap();
        let db = DatabaseStore {
            conn: Mutex::new(conn),
        };
        let before = db.list_agent_chat_messages(&a.thread_id());
        let mut edited = a.clone();
        edited.messages = vec![("user".into(), "Do not overwrite".into())];
        let result = state.import_local_sessions(&db, &[edited]).unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(db.list_agent_chat_messages(&a.thread_id()), before);
        let fts: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM agent_chat_search WHERE thread_id=?1",
                [a.thread_id()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 2);
    }
    #[tokio::test]
    async fn local_import_provenance_blocks_continuation_even_without_reserved_prefix() {
        use tauri::Manager;
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session(
            "synthetic-renamed-thread",
            "ws",
            Some("/synthetic/project"),
            "claude",
        )
        .unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO agent_chat_local_imports(source_id,thread_id,provider,workspace_id,cwd,title) VALUES('synthetic-source','synthetic-renamed-thread','claude','ws','/synthetic/project','Imported')",[]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(crate::commands::agent_chat::ProviderRegistry::new());
        let result = crate::commands::agent_chat::ensure_live_session(
            app.handle(),
            crate::agent_provider::ProviderKind::Claude,
            &crate::agent_provider::ThreadId("synthetic-renamed-thread".into()),
        )
        .await;
        assert!(result
            .unwrap_err()
            .starts_with("imported_snapshot_read_only"));
    }
    #[tokio::test]
    async fn local_import_malformed_pane_start_rejects_prefixless_provenance() {
        use tauri::Manager;
        for thread in ["prefixless-imported", "local-import-synthetic"] {
            let db = DatabaseStore::new_in_memory();
            db.conn.lock().unwrap().execute("INSERT INTO agent_chat_local_imports(source_id,thread_id,provider,workspace_id,cwd,title) VALUES('synthetic-source',?1,'claude','ws','/synthetic/project','Imported')", [thread]).unwrap();
            let state = AppStateStore::default();
            let workspace = state.snapshot().active_workspace_id;
            let pane = state
                .create_agent_chat_pane(&workspace.0, None, None, None, Some(thread.into()))
                .unwrap();
            let before = serde_json::to_value(state.snapshot()).unwrap();
            let app = tauri::test::mock_app();
            app.manage(db);
            app.manage(state);
            app.manage(crate::commands::agent_chat::ProviderRegistry::new());
            app.manage(crate::observability::ObservabilityStore::default());
            let input = serde_json::from_value(serde_json::json!({"thread_id":"fresh-thread","cwd":"/synthetic/project","additional_directories":[]})).unwrap();
            let error = crate::commands::agent_chat::agent_chat_start_session(
                app.handle().clone(),
                pane.0,
                crate::agent_provider::ProviderKind::Codex,
                input,
                Some(thread.into()),
            )
            .await
            .unwrap_err();
            assert!(
                error.starts_with("imported_snapshot_read_only"),
                "{thread}: {error}"
            );
            assert_eq!(
                serde_json::to_value(app.state::<AppStateStore>().snapshot()).unwrap(),
                before
            );
        }
    }
    #[test]
    fn local_import_atomic_claim_rechecks_provenance_after_preflight() {
        use crate::agent_provider::ProviderKind;
        use tauri::Manager;
        let db = DatabaseStore::new_in_memory();
        db.conn.lock().unwrap().execute("INSERT INTO agent_chat_local_imports(source_id,thread_id,provider,workspace_id,cwd,title) VALUES('synthetic-source','prefixless-imported','claude','ws','/synthetic/project','Imported')", []).unwrap();
        let state = AppStateStore::default();
        let workspace = state.snapshot().active_workspace_id;
        let pane = state
            .create_agent_chat_pane(
                &workspace.0,
                None,
                None,
                None,
                Some("ordinary-thread".into()),
            )
            .unwrap();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(state);
        let state = app.state::<AppStateStore>();
        crate::local_session_import::require_live_session(
            app.handle(),
            &state.agent_chat_thread_id(&pane.0).unwrap(),
        )
        .unwrap();
        // The binding changes AFTER preflight, precisely the TOCTOU window.
        state.set_agent_chat_thread_id(&pane.0, Some("prefixless-imported".into()));
        let before = serde_json::to_value(state.snapshot()).unwrap();
        let result = state.claim_agent_chat_pane_checked(
            &pane.0,
            ProviderKind::Codex,
            "fresh-thread",
            Some("prefixless-imported"),
            |thread| crate::local_session_import::require_live_session(app.handle(), thread),
        );
        assert!(
            matches!(result, Err(crate::state::PaneClaimConflict::ReadOnly(error)) if error.starts_with("imported_snapshot_read_only"))
        );
        assert_eq!(serde_json::to_value(state.snapshot()).unwrap(), before);
    }
    #[test]
    fn local_import_never_overwrites_existing_thread_collision() {
        let db = DatabaseStore::new_in_memory();
        let state = AppStateStore::default();
        state.clear_workspaces();
        let a = fixture();
        db.upsert_agent_chat_session(
            &a.thread_id(),
            "live-workspace",
            Some("/live/project"),
            "codex",
        )
        .unwrap();
        db.append_agent_chat_message(
            &a.thread_id(),
            "{\"type\":\"user_message\",\"text\":\"Live history\"}",
        )
        .unwrap();
        let result = state.import_local_sessions(&db, &[a.clone()]).unwrap();
        assert_eq!(result.skipped, 1);
        assert!(state.snapshot().workspaces.is_empty());
        assert_eq!(
            db.get_agent_chat_session(&a.thread_id())
                .unwrap()
                .workspace_id,
            "live-workspace"
        );
        assert!(db.list_agent_chat_messages(&a.thread_id())[0].contains("Live history"));
    }
}

impl DatabaseStore {
    pub(crate) fn write_local_imports(
        &self,
        sessions: &[ParsedSession],
        mut prepare: impl FnMut(&ParsedSession) -> Result<String, String>,
    ) -> Result<ImportResponse, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut response = ImportResponse::default();
        for session in sessions {
            let thread = session.thread_id();
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_chat_local_imports WHERE source_id=?1) OR EXISTS(SELECT 1 FROM agent_chat_sessions WHERE thread_id=?2)",params![session.source_id,thread],|r|r.get(0)).map_err(|e|e.to_string())?;
            if exists {
                response.skipped += 1;
                continue;
            }
            let workspace = prepare(session)?;
            tx.execute("INSERT INTO agent_chat_sessions(thread_id,workspace_id,cwd,provider,title,created_at,last_active_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",params![thread,workspace,session.cwd,session.provider,session.title(),session.last_active_at]).map_err(|e|e.to_string())?;
            let mut turn = String::new();
            for (index, (role, text)) in session.messages.iter().enumerate() {
                if role == "user" || turn.is_empty() {
                    turn = format!("{}-turn-{index}", thread);
                }
                let payload = if role == "user" {
                    serde_json::json!({"type":"user_message","thread_id":thread,"turn_id":turn,"text":text})
                } else {
                    serde_json::to_value(
                        crate::agent_provider::ProviderRuntimeEvent::ItemCompleted {
                            thread_id: crate::agent_provider::ThreadId(thread.clone()),
                            turn_id: crate::agent_provider::TurnId(turn.clone()),
                            item: crate::agent_provider::CompletedItem::AssistantText {
                                text: text.clone(),
                            },
                            subagent_id: None,
                        },
                    )
                    .map_err(|e| e.to_string())?
                };
                tx.execute("INSERT INTO agent_chat_messages(thread_id,payload,created_at) VALUES(?1,?2,?3)",params![thread,payload.to_string(),session.last_active_at]).map_err(|e|e.to_string())?;
                if role == "assistant" {
                    let done = serde_json::to_value(
                        crate::agent_provider::ProviderRuntimeEvent::TurnCompleted {
                            thread_id: crate::agent_provider::ThreadId(thread.clone()),
                            turn_id: crate::agent_provider::TurnId(turn.clone()),
                            status: crate::agent_provider::TurnStatus::Success,
                            usage: None,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    tx.execute("INSERT INTO agent_chat_messages(thread_id,payload,created_at) VALUES(?1,?2,?3)",params![thread,done.to_string(),session.last_active_at]).map_err(|e|e.to_string())?;
                }
            }
            tx.execute("INSERT INTO agent_chat_local_imports(source_id,thread_id,provider,workspace_id,cwd,title) VALUES(?1,?2,?3,?4,?5,?6)",params![session.source_id,thread,session.provider,workspace,session.cwd,session.title()]).map_err(|e|e.to_string())?;
            response.imported.push(ImportedSession {
                source_id: session.source_id.clone(),
                thread_id: thread,
                workspace_id: workspace,
            });
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(response)
    }
    pub(crate) fn local_import_provider(&self, thread: &str) -> Result<Option<String>, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT provider FROM agent_chat_local_imports WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }
    pub(crate) fn local_import_source_ids(&self) -> Result<HashSet<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT source_id FROM agent_chat_local_imports")
            .map_err(|e| e.to_string())?;
        let result = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<HashSet<String>>>()
            .map_err(|e| e.to_string());
        result
    }
    pub(crate) fn pending_local_imports(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt=conn.prepare("SELECT source_id,thread_id,provider,workspace_id,cwd FROM agent_chat_local_imports WHERE pending_layout=1 ORDER BY rowid").map_err(|e|e.to_string())?;
        let result = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string());
        result
    }
    pub(crate) fn complete_local_import_layout(&self, ids: &[String]) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for id in ids {
            tx.execute(
                "UPDATE agent_chat_local_imports SET pending_layout=0 WHERE source_id=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}
