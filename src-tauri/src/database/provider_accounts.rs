//! Extra provider accounts (`crate::provider_accounts`).
//!
//! The CLI's own sign-in is the implicit `default` account and has no row.
//! Each extra account owns a home directory the CLI is pointed at with
//! `CLAUDE_CONFIG_DIR` / `CODEX_HOME`. Defaults are scoped: the empty scope
//! is the app-wide default, any other scope is a workspace id. A binding
//! pins the account a chat thread runs on. No foreign keys: a binding must
//! survive its session row being rebuilt, and a removed account simply
//! stops resolving.
use super::*;

/// One extra account, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAccountRow {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub home: String,
    pub created_at_ms: i64,
}

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS provider_accounts (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            label TEXT NOT NULL,
            home TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS provider_account_defaults (
            scope TEXT NOT NULL,
            provider TEXT NOT NULL,
            account_id TEXT NOT NULL,
            PRIMARY KEY (scope, provider)
        );
        CREATE TABLE IF NOT EXISTS provider_account_bindings (
            thread_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            account_id TEXT NOT NULL,
            PRIMARY KEY (thread_id, provider)
        );",
    )
    .map_err(|e| format!("Failed to create provider account tables: {e}"))?;
    // Which account produced each usage row. NULL is the default account,
    // which is also what every row written before accounts existed means.
    if let Err(e) = conn.execute("ALTER TABLE agent_usage_ledger ADD COLUMN account_id TEXT", []) {
        if !e.to_string().contains("duplicate column name") {
            return Err(format!("Failed to add usage account column: {e}"));
        }
    }
    Ok(())
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderAccountRow> {
    Ok(ProviderAccountRow {
        id: row.get(0)?,
        provider: row.get(1)?,
        label: row.get(2)?,
        home: row.get(3)?,
        created_at_ms: row.get(4)?,
    })
}

impl DatabaseStore {
    /// Every extra account, oldest first.
    pub fn provider_accounts(&self) -> Result<Vec<ProviderAccountRow>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, provider, label, home, created_at_ms FROM provider_accounts
                 ORDER BY created_at_ms ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], account_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn provider_account(&self, id: &str) -> Result<Option<ProviderAccountRow>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, provider, label, home, created_at_ms FROM provider_accounts WHERE id = ?1",
            [id],
            account_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn insert_provider_account(&self, account: &ProviderAccountRow) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO provider_accounts (id, provider, label, home, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account.id,
                account.provider,
                account.label,
                account.home,
                account.created_at_ms
            ],
        )
        .map_err(|e| format!("Failed to save account: {e}"))?;
        Ok(())
    }

    pub fn rename_provider_account(&self, id: &str, label: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute("UPDATE provider_accounts SET label = ?2 WHERE id = ?1", params![id, label])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Remove an account with every default and binding naming it. Chats
    /// that ran on it fall back to whatever default applies next, starting
    /// fresh: their native id names a transcript that left with the account,
    /// and any older copy elsewhere predates what the chat shows.
    ///
    /// Returns the chats whose native id was cleared.
    pub fn delete_provider_account(&self, id: &str) -> Result<Vec<(String, String)>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let cleared: Vec<(String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT s.thread_id, s.sdk_session_id FROM agent_chat_sessions s
                     JOIN provider_account_bindings b
                       ON b.thread_id = s.thread_id AND b.provider = s.provider
                     WHERE b.account_id = ?1 AND s.sdk_session_id IS NOT NULL",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        tx.execute(
            "UPDATE agent_chat_sessions SET sdk_session_id = NULL
             WHERE EXISTS (SELECT 1 FROM provider_account_bindings b
                           WHERE b.thread_id = agent_chat_sessions.thread_id
                             AND b.provider = agent_chat_sessions.provider
                             AND b.account_id = ?1)",
            [id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM provider_accounts WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM provider_account_defaults WHERE account_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM provider_account_bindings WHERE account_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(cleared)
    }

    /// The default for `provider` in `scope` (`""` = app-wide).
    pub fn provider_account_default(&self, scope: &str, provider: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT account_id FROM provider_account_defaults WHERE scope = ?1 AND provider = ?2",
            params![scope, provider],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Every default in `scope`, keyed by provider.
    pub fn provider_account_defaults(&self, scope: &str) -> Result<HashMap<String, String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT provider, account_id FROM provider_account_defaults WHERE scope = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([scope], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Set the default, or clear it with `None` so the next scope applies.
    pub fn set_provider_account_default(
        &self,
        scope: &str,
        provider: &str,
        account_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match account_id {
            Some(id) => conn.execute(
                "INSERT INTO provider_account_defaults (scope, provider, account_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT(scope, provider) DO UPDATE SET account_id = excluded.account_id",
                params![scope, provider, id],
            ),
            None => conn.execute(
                "DELETE FROM provider_account_defaults WHERE scope = ?1 AND provider = ?2",
                params![scope, provider],
            ),
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The account `thread_id` is pinned to for `provider`. A chat that
    /// moved between providers keeps one binding per provider, so switching
    /// back resumes on the account that holds that provider's transcript.
    pub fn provider_account_binding(&self, thread_id: &str, provider: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT account_id FROM provider_account_bindings WHERE thread_id = ?1 AND provider = ?2",
            params![thread_id, provider],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn set_provider_account_binding(
        &self,
        thread_id: &str,
        provider: &str,
        account_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO provider_account_bindings (thread_id, provider, account_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id, provider) DO UPDATE SET account_id = excluded.account_id",
            params![thread_id, provider, account_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Give `to` every binding `from` has (a provider re-keyed the thread).
    pub fn copy_provider_account_bindings(&self, from: &str, to: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO provider_account_bindings (thread_id, provider, account_id)
             SELECT ?2, provider, account_id FROM provider_account_bindings WHERE thread_id = ?1
             ON CONFLICT(thread_id, provider) DO UPDATE SET account_id = excluded.account_id",
            params![from, to],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DatabaseStore {
        DatabaseStore::new_in_memory()
    }

    fn account(id: &str, provider: &str, at: i64) -> ProviderAccountRow {
        ProviderAccountRow {
            id: id.into(),
            provider: provider.into(),
            label: format!("{id} label"),
            home: format!("/homes/{id}"),
            created_at_ms: at,
        }
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::create_schema(&conn).unwrap();
        create_schema(&conn).unwrap();
        create_schema(&conn).unwrap();
    }

    #[test]
    fn removing_an_account_drops_its_defaults_and_bindings() {
        let db = store();
        db.insert_provider_account(&account("work", "claude", 2)).unwrap();
        db.insert_provider_account(&account("side", "claude", 1)).unwrap();
        assert_eq!(
            db.provider_accounts().unwrap().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["side", "work"]
        );
        db.set_provider_account_default("", "claude", Some("work")).unwrap();
        db.set_provider_account_default("ws-1", "claude", Some("side")).unwrap();
        db.set_provider_account_binding("t1", "claude", "work").unwrap();
        db.upsert_agent_chat_session("t1", "ws-1", Some("/repo"), "claude").unwrap();
        db.set_agent_chat_sdk_session_id("t1", "native-1").unwrap();
        assert_eq!(
            db.delete_provider_account("work").unwrap(),
            vec![("t1".to_string(), "native-1".to_string())]
        );
        assert_eq!(db.get_agent_chat_session("t1").unwrap().sdk_session_id, None);
        assert_eq!(db.provider_account_default("", "claude").unwrap(), None);
        assert_eq!(db.provider_account_default("ws-1", "claude").unwrap().as_deref(), Some("side"));
        assert_eq!(db.provider_account_binding("t1", "claude").unwrap(), None);
        assert!(db.provider_account("work").unwrap().is_none());
    }

    #[test]
    fn bindings_are_kept_per_provider_and_follow_a_rekeyed_thread() {
        let db = store();
        db.set_provider_account_binding("t", "claude", "work").unwrap();
        db.set_provider_account_binding("t", "codex", "side").unwrap();
        assert_eq!(db.provider_account_binding("t", "claude").unwrap().as_deref(), Some("work"));
        assert_eq!(db.provider_account_binding("t", "codex").unwrap().as_deref(), Some("side"));
        db.copy_provider_account_bindings("t", "t2").unwrap();
        assert_eq!(db.provider_account_binding("t2", "claude").unwrap().as_deref(), Some("work"));
    }

    #[test]
    fn clearing_a_default_removes_it() {
        let db = store();
        db.set_provider_account_default("", "codex", Some("a")).unwrap();
        db.set_provider_account_default("", "codex", Some("b")).unwrap();
        assert_eq!(db.provider_account_defaults("").unwrap()["codex"], "b");
        db.set_provider_account_default("", "codex", None).unwrap();
        assert!(db.provider_account_defaults("").unwrap().is_empty());
    }

    #[test]
    fn a_usage_row_keeps_the_account_that_first_produced_it() {
        let db = store();
        let row = |account: Option<&str>, output: i64| crate::database::ProviderUsageCacheRow {
            import_key: "claude:msg-1".into(),
            created_at: 1_800_000_000_000,
            thread_id: "claude:s1".into(),
            provider: "claude".into(),
            model: None,
            subagent: false,
            input_tokens: 1,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: None,
            cost_source: None,
            account_id: account.map(str::to_string),
        };
        db.upsert_provider_usage_rows(&[row(Some("work"), 1)]).unwrap();
        // A later scan sees the copy in another home: totals update, the
        // owner does not.
        db.upsert_provider_usage_rows(&[row(None, 2)]).unwrap();
        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].output_tokens, 2);
        assert_eq!(rows[0].account_id.as_deref(), Some("work"));
    }

    #[test]
    fn a_removed_accounts_usage_survives_a_cache_rebuild() {
        let db = store();
        db.insert_provider_account(&account("work", "claude", 1)).unwrap();
        db.insert_provider_account(&account("gone", "claude", 2)).unwrap();
        let row = |key: &str, account: Option<&str>| crate::database::ProviderUsageCacheRow {
            import_key: key.into(),
            created_at: 1_800_000_000_000,
            thread_id: "claude:s1".into(),
            provider: "claude".into(),
            model: None,
            subagent: false,
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: None,
            cost_source: None,
            account_id: account.map(str::to_string),
        };
        db.upsert_provider_usage_rows(&[row("a", None), row("b", Some("work")), row("c", Some("gone"))])
            .unwrap();
        db.delete_provider_account("gone").unwrap();
        db.reset_usage_history().unwrap();
        let kept = db.usage_rows_since(0).unwrap();
        assert_eq!(kept.iter().map(|r| r.account_id.as_deref()).collect::<Vec<_>>(), vec![Some("gone")]);
    }
}
