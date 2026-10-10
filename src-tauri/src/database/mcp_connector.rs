//! Independent MCP client registrations and hash-at-rest grants. No web sessions.
use super::DatabaseStore;
use rusqlite::{params, OptionalExtension};

pub const MAX_CLIENTS: i64 = 256;
pub const UNAPPROVED_CLIENT_TTL_MS: i64 = 600_000;
// Registration metadata survives short-lived tokens, but is not bearer authority.
pub const APPROVED_CLIENT_RETENTION_MS: i64 = 7 * 86_400_000;
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agent_mcp_config (
    id INTEGER PRIMARY KEY CHECK(id=1),
    config_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_mcp_clients (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    redirects_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    approved_at INTEGER
);
CREATE TABLE IF NOT EXISTS agent_mcp_grants (
    id TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL REFERENCES agent_mcp_clients(id) ON DELETE CASCADE,
    token_hash TEXT UNIQUE NOT NULL,
    redirect_uri TEXT NOT NULL,
    resource TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only','supervised','full_access')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1))
);
CREATE INDEX IF NOT EXISTS agent_mcp_grants_client ON agent_mcp_grants(client_id);
"#;

pub struct NewMcpClient {
    pub id: String,
    pub name: String,
    pub redirects: Vec<String>,
    pub created_at: i64,
}
pub type McpClient = NewMcpClient;

pub struct NewMcpGrant {
    pub id: String,
    pub client_id: String,
    pub token_hash: String,
    pub redirect_uri: String,
    pub resource: String,
    pub access: String,
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct McpGrant {
    pub id: String,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub resource: String,
    pub access: String,
    pub created_at: i64,
    pub expires_at: i64,
}

fn grant_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpGrant> {
    Ok(McpGrant {
        id: row.get(0)?,
        client_id: row.get(1)?,
        client_name: row.get(2)?,
        redirect_uri: row.get(3)?,
        resource: row.get(4)?,
        access: row.get(5)?,
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}
const GRANT_SELECT: &str = "SELECT g.id,g.client_id,c.name,g.redirect_uri,g.resource,g.access,g.created_at,g.expires_at FROM agent_mcp_grants g JOIN agent_mcp_clients c ON c.id=g.client_id";

pub(super) fn migrate(conn: &rusqlite::Connection) -> Result<(), String> {
    // Reserve the writer before schema reads retain a WAL snapshot. A Deferred
    // read-to-write upgrade can otherwise fail with BUSY_SNAPSHOT after another
    // connection commits, even with the normal busy timeout.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let has_marker: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_mcp_clients') WHERE name='approved_at')",
        [], |r| r.get(0),
    ).map_err(|e| e.to_string())?;
    if !has_marker {
        tx.execute(
            "ALTER TABLE agent_mcp_clients ADD COLUMN approved_at INTEGER",
            [],
        )
        .map_err(|e| e.to_string())?;
        // Only a persisted grant proves earlier owner consent. Preserve its
        // historical time; never invent approval for anonymous registrations.
        tx.execute("UPDATE agent_mcp_clients SET approved_at=(SELECT MAX(g.created_at) FROM agent_mcp_grants g WHERE g.client_id=agent_mcp_clients.id)", [])
            .map_err(|e| e.to_string())?;
    }
    // Preserve only legacy receipt owners that still have an exact persisted
    // grant-to-registration mapping and no competing receipt for the stable key.
    // Missing mappings/collisions stay untouched; never guess or merge receipts.
    tx.execute(
        "UPDATE agent_control_operations SET principal=(
            SELECT 'mcp-client:' || g.client_id FROM agent_mcp_grants g
            WHERE agent_control_operations.principal='mcp:' || g.id
         ) WHERE id IN (
            SELECT o.id FROM agent_control_operations o JOIN agent_mcp_grants g
                ON o.principal='mcp:' || g.id
            WHERE NOT EXISTS (
                SELECT 1 FROM agent_control_operations sibling
                WHERE sibling.id<>o.id AND sibling.request_key=o.request_key
                  AND (sibling.principal='mcp-client:' || g.client_id
                    OR sibling.principal IN (
                        SELECT 'mcp:' || peer.id FROM agent_mcp_grants peer
                        WHERE peer.client_id=g.client_id
                    ))
            )
         )",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
fn prune_clients(conn: &rusqlite::Connection, now: i64, protected: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM agent_mcp_grants WHERE revoked=1 OR expires_at<=?1",
        [now],
    )
    .map_err(|_| "MCP pruning failed")?;
    conn.execute("DELETE FROM agent_mcp_clients WHERE ((approved_at IS NULL AND created_at<=?1) OR approved_at<=?3) AND id NOT IN (SELECT value FROM json_each(?2)) AND NOT EXISTS(SELECT 1 FROM agent_mcp_grants g WHERE g.client_id=agent_mcp_clients.id)", params![now.saturating_sub(UNAPPROVED_CLIENT_TTL_MS), protected, now.saturating_sub(APPROVED_CLIENT_RETENTION_MS)])
        .map_err(|_| "MCP pruning failed")?;
    Ok(())
}

impl DatabaseStore {
    pub fn mcp_prune_clients(&self, now: i64, protected: &[String]) -> Result<(), String> {
        let protected =
            serde_json::to_string(protected).map_err(|_| "Invalid protected clients")?;
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let tx = conn.transaction().map_err(|_| "MCP pruning failed")?;
        prune_clients(&tx, now, &protected)?;
        tx.commit().map_err(|_| "MCP pruning failed".into())
    }
    pub fn mcp_connector_config(&self) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(
            "SELECT config_json FROM agent_mcp_config WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "Connector configuration lookup failed".into())
    }
    pub fn mcp_connector_set_config(&self, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.execute("INSERT INTO agent_mcp_config(id,config_json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config_json=excluded.config_json", [value])
            .map_err(|_| "Connector configuration persistence failed")?;
        Ok(())
    }
    pub fn mcp_register_client(&self, client: NewMcpClient, now: i64) -> Result<(), String> {
        self.mcp_register_client_preserving(client, now, &[])
    }
    pub fn mcp_register_client_preserving(
        &self,
        client: NewMcpClient,
        now: i64,
        protected: &[String],
    ) -> Result<(), String> {
        let protected =
            serde_json::to_string(protected).map_err(|_| "Invalid protected clients")?;
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let tx = conn
            .transaction()
            .map_err(|_| "Client registration failed")?;
        if tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_mcp_clients WHERE id=?1)",
                [&client.id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|_| "Client registration failed")?
        {
            return Err("Client registration failed".into());
        }
        prune_clients(&tx, now, &protected)?;
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM agent_mcp_clients", [], |r| r.get(0))
            .map_err(|_| "Client registration failed")?;
        if count >= MAX_CLIENTS {
            // Recover from an unauthenticated registration flood without
            // displacing a retained approved client. Admission stays bounded.
            let removed = tx.execute("DELETE FROM agent_mcp_clients WHERE id=(SELECT c.id FROM agent_mcp_clients c WHERE c.approved_at IS NULL AND NOT EXISTS(SELECT 1 FROM agent_mcp_grants g WHERE g.client_id=c.id) AND c.id NOT IN (SELECT value FROM json_each(?1)) ORDER BY c.created_at,c.id LIMIT 1)", [&protected])
                .map_err(|_| "Client registration failed")?;
            if removed == 0 {
                return Err("Client registration limit reached".into());
            }
        }
        tx.execute(
            "INSERT INTO agent_mcp_clients(id,name,redirects_json,created_at) VALUES(?1,?2,?3,?4)",
            params![
                client.id,
                client.name,
                serde_json::to_string(&client.redirects).map_err(|_| "Invalid redirects")?,
                client.created_at
            ],
        )
        .map_err(|_| "Client registration failed".to_string())?;
        tx.commit().map_err(|_| "Client registration failed")?;
        Ok(())
    }
    pub fn mcp_client(&self, id: &str) -> Result<Option<McpClient>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(
            "SELECT id,name,redirects_json,created_at FROM agent_mcp_clients WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| "Client lookup failed".to_string())?
        .map(|(id, name, redirects, created_at)| {
            Ok(McpClient {
                id,
                name,
                redirects: serde_json::from_str(&redirects)
                    .map_err(|_| "Invalid client record".to_string())?,
                created_at,
            })
        })
        .transpose()
    }
    pub fn mcp_mint_grant(&self, grant: NewMcpGrant) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let tx = conn.transaction().map_err(|_| "Grant persistence failed")?;
        // One active grant per public client. Reauthorization cannot leave an
        // earlier, more privileged token alive. Roll back on insertion failure.
        tx.execute(
            "UPDATE agent_mcp_grants SET revoked=1 WHERE client_id=?1",
            [&grant.client_id],
        )
        .map_err(|_| "Grant persistence failed")?;
        tx.execute("INSERT INTO agent_mcp_grants(id,client_id,token_hash,redirect_uri,resource,access,created_at,expires_at,revoked) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",params![grant.id,grant.client_id,grant.token_hash,grant.redirect_uri,grant.resource,grant.access,grant.created_at,grant.expires_at]).map_err(|_|"Grant persistence failed".to_string())?;
        tx.execute("UPDATE agent_mcp_clients SET approved_at=?2 WHERE id=?1", params![grant.client_id, grant.created_at])
            .map_err(|_| "Grant persistence failed")?;
        // No tombstone is required for an opaque grant: absence fails closed.
        tx.execute(
            "DELETE FROM agent_mcp_grants WHERE revoked=1 OR expires_at<=?1",
            [grant.created_at],
        )
        .map_err(|_| "Grant persistence failed")?;
        tx.commit().map_err(|_| "Grant persistence failed")?;
        Ok(())
    }
    pub fn mcp_grant_by_hash(
        &self,
        hash: &str,
        resource: &str,
        now: i64,
    ) -> Result<Option<McpGrant>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(
            &format!("{GRANT_SELECT} WHERE g.token_hash=?1 AND g.resource=?2 AND g.revoked=0 AND g.expires_at>?3"),
            params![hash, resource, now],
            grant_row,
        )
        .optional()
        .map_err(|_| "Grant lookup failed".to_string())
    }
    pub fn mcp_grant(
        &self,
        id: &str,
        resource: &str,
        now: i64,
    ) -> Result<Option<McpGrant>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(&format!("{GRANT_SELECT} WHERE g.id=?1 AND g.resource=?2 AND g.revoked=0 AND g.expires_at>?3"), params![id, resource, now], grant_row)
            .optional()
            .map_err(|_| "Grant lookup failed".to_string())
    }
    pub fn mcp_list_grants(&self, now: i64) -> Result<Vec<McpGrant>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let mut statement = conn
            .prepare(&format!(
                "{GRANT_SELECT} WHERE g.revoked=0 AND g.expires_at>?1 ORDER BY g.created_at DESC"
            ))
            .map_err(|_| "Grant list failed")?;
        let rows = statement
            .query_map([now], grant_row)
            .map_err(|_| "Grant list failed")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "Grant list failed".to_string())?;
        Ok(rows)
    }
    pub fn mcp_revoke_client(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.execute(
            "UPDATE agent_mcp_grants SET revoked=1 WHERE client_id=?1",
            [id],
        )
        .map_err(|_| "Grant revocation failed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_connector::oauth::credential_hash;
    use rusqlite::Connection;
    use std::sync::Mutex;
    fn db() -> DatabaseStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        DatabaseStore {
            conn: Mutex::new(conn),
        }
    }
    fn client(id: &str) -> NewMcpClient {
        NewMcpClient {
            id: id.into(),
            name: "Untrusted agent".into(),
            redirects: vec!["https://client.example/cb".into()],
            created_at: 0,
        }
    }
    fn grant(id: &str, token: &str) -> NewMcpGrant {
        NewMcpGrant {
            id: id.into(),
            client_id: "client".into(),
            token_hash: credential_hash(token),
            redirect_uri: "https://client.example/cb".into(),
            resource: "https://mcp.example/mcp".into(),
            access: "read_only".into(),
            created_at: 1,
            expires_at: 100,
        }
    }
    // A scheduling-only hook on the real completed migration read. The writer
    // either commits (old Deferred mode) or signals actual lock contention and
    // waits for the schema owner to finish; no writer commit is required inside
    // an Immediate transaction, and no same-thread callback waits for that commit.
    #[test]
    fn migration_serializes_writer_after_snapshot_read() {
        use std::ffi::{c_void, CStr};
        use std::sync::mpsc;
        use std::time::Duration;
        struct WriterGate {
            observed: mpsc::Sender<String>,
            release: mpsc::Receiver<()>,
            timed_out: bool,
        }
        unsafe extern "C" fn busy(context: *mut c_void, _: i32) -> i32 {
            let gate = unsafe { &mut *(context as *mut WriterGate) };
            let _ = gate.observed.send("writer-blocked".into());
            if gate.release.recv_timeout(Duration::from_secs(5)).is_err() {
                gate.timed_out = true;
                return 0;
            }
            1
        }
        struct ReadGate {
            start: mpsc::Sender<()>,
            observed: mpsc::Receiver<String>,
            observation: Option<String>,
            fired: bool,
            write_error: i32,
        }
        unsafe extern "C" fn profile(
            kind: u32,
            context: *mut c_void,
            stmt: *mut c_void,
            _: *mut c_void,
        ) -> i32 {
            if kind != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
                return 0;
            }
            let stmt = stmt as *mut rusqlite::ffi::sqlite3_stmt;
            let ptr = unsafe { rusqlite::ffi::sqlite3_sql(stmt) };
            if ptr.is_null() {
                return 0;
            }
            let sql = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
            let gate = unsafe { &mut *(context as *mut ReadGate) };
            if sql.starts_with("UPDATE agent_control_operations SET principal=") {
                gate.write_error = unsafe {
                    rusqlite::ffi::sqlite3_extended_errcode(rusqlite::ffi::sqlite3_db_handle(stmt))
                };
            }
            if !gate.fired && sql.contains("pragma_table_info('agent_mcp_clients')") {
                gate.fired = true;
                let _ = gate.start.send(());
                gate.observation = gate.observed.recv_timeout(Duration::from_secs(5)).ok();
            }
            0
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migration.db");
        let seed = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&seed).unwrap();
        let seed = DatabaseStore {
            conn: Mutex::new(seed),
        };
        seed.mcp_register_client(client("client"), 0).unwrap();
        seed.mcp_mint_grant(grant("grant", "synthetic-migration"))
            .unwrap();
        let receipt = seed
            .admit_control_operation(
                "mcp:grant",
                "migration-key",
                "thread_read",
                "hash",
                "boot",
                None,
                None,
            )
            .unwrap()
            .receipt;
        drop(seed);
        let reader = super::super::open_connection(&path).unwrap();
        let writer = super::super::open_connection(&path).unwrap();
        let (start_tx, start_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut gate = Box::new(WriterGate {
                observed: observed_tx,
                release: release_rx,
                timed_out: false,
            });
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_busy_handler(
                        writer.handle(),
                        Some(busy),
                        &mut *gate as *mut WriterGate as *mut c_void
                    ),
                    0
                );
            }
            let writer = DatabaseStore {
                conn: Mutex::new(writer),
            };
            let started = start_rx.recv_timeout(Duration::from_secs(5));
            let result = if started.is_ok() {
                writer.set_setting("web_remote.device_id", "synthetic-owned-device")
            } else {
                Err("read hook did not start writer".into())
            };
            let _ = gate.observed.send(format!("writer-committed:{result:?}"));
            unsafe {
                rusqlite::ffi::sqlite3_busy_handler(
                    writer.conn.lock().unwrap().handle(),
                    None,
                    std::ptr::null_mut(),
                );
            }
            (result, gate.timed_out)
        });
        let mut gate = Box::new(ReadGate {
            start: start_tx,
            observed: observed_rx,
            observation: None,
            fired: false,
            write_error: 0,
        });
        unsafe {
            assert_eq!(
                rusqlite::ffi::sqlite3_trace_v2(
                    reader.handle(),
                    rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                    Some(profile),
                    &mut *gate as *mut ReadGate as *mut c_void
                ),
                0
            );
        }
        let result = super::super::create_schema(&reader);
        unsafe {
            rusqlite::ffi::sqlite3_trace_v2(reader.handle(), 0, None, std::ptr::null_mut());
        }
        let _ = release_tx.send(());
        let (written, timed_out) = worker.join().unwrap();
        println!("MIGRATION_SCHEDULE schema={result:?} writer={written:?} observation={:?} write_extended_code={} timed_out={timed_out}", gate.observation, gate.write_error);
        assert!(
            gate.fired && gate.observation.is_some(),
            "completed production schema read was observed"
        );
        assert!(
            !timed_out && written.is_ok(),
            "competing real setter must survive after schema commit"
        );
        assert!(
            result.is_ok(),
            "schema must acquire write authority before its WAL snapshot: {result:?}"
        );
        assert_eq!(gate.observation.as_deref(), Some("writer-blocked"));
        let readback = DatabaseStore {
            conn: Mutex::new(reader),
        };
        assert_eq!(
            readback.get_setting("web_remote.device_id").as_deref(),
            Some("synthetic-owned-device")
        );
        let principal: String = readback
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT principal FROM agent_control_operations WHERE id=?1",
                [&receipt.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(principal, "mcp-client:client");
    }

    #[test]
    fn migration_fresh_upgrade_and_repeated_full_reopen_preserve_owners_and_consent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("upgrade.db");
        let conn = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&conn).unwrap();
        conn.execute_batch("DROP TABLE agent_mcp_grants; DROP TABLE agent_mcp_clients; DROP TABLE agent_mcp_config;").unwrap();
        let legacy = SCHEMA.replace(
            "created_at INTEGER NOT NULL,\n    approved_at INTEGER",
            "created_at INTEGER NOT NULL",
        );
        conn.execute_batch(&legacy).unwrap();
        conn.execute_batch("INSERT INTO agent_mcp_clients VALUES('client','client','[]',0),('anonymous','anon','[]',0);
            INSERT INTO agent_mcp_grants VALUES('grant','client','hash','https://client.example/cb','https://mcp.example/mcp','read_only',7,100,0),
                ('peer','client','hash-peer','https://client.example/cb','https://mcp.example/mcp','read_only',11,100,1);
            INSERT INTO agent_control_operations(id,principal,request_key,tool,payload_hash,epoch,state) VALUES
                ('unique','mcp:grant','unique','thread_read','h','b','succeeded'),
                ('missing','mcp:deleted','missing','thread_read','h','b','succeeded'),
                ('collision','mcp:grant','collision','thread_read','h','b','succeeded'),
                ('stable','mcp-client:client','collision','thread_read','h','b','succeeded'),
                ('ambiguous-a','mcp:grant','ambiguous','thread_read','h','b','succeeded'),
                ('ambiguous-b','mcp:peer','ambiguous','thread_read','h','b','succeeded');
            UPDATE schema_version SET version=19;").unwrap();
        drop(conn);
        for _ in 0..3 {
            let conn = super::super::open_connection(&path).unwrap();
            super::super::create_schema(&conn).unwrap();
            let owners: Vec<(String, String)> = conn
                .prepare("SELECT id,principal FROM agent_control_operations ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                owners,
                vec![
                    ("ambiguous-a".into(), "mcp:grant".into()),
                    ("ambiguous-b".into(), "mcp:peer".into()),
                    ("collision".into(), "mcp:grant".into()),
                    ("missing".into(), "mcp:deleted".into()),
                    ("stable".into(), "mcp-client:client".into()),
                    ("unique".into(), "mcp-client:client".into())
                ]
            );
            let approvals: Vec<(String, Option<i64>)> = conn
                .prepare("SELECT id,approved_at FROM agent_mcp_clients ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                approvals,
                vec![("anonymous".into(), None), ("client".into(), Some(11))]
            );
            assert_eq!(
                conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                    .get::<_, u32>(0))
                    .unwrap(),
                20
            );
            assert_eq!(
                conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn migration_failure_rolls_back_and_busy_admission_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollback.db");
        let conn = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&conn).unwrap();
        conn.execute_batch("DROP TABLE agent_mcp_grants; DROP TABLE agent_mcp_clients; DROP TABLE agent_mcp_config;").unwrap();
        conn.execute_batch(&SCHEMA.replace(
            "created_at INTEGER NOT NULL,\n    approved_at INTEGER",
            "created_at INTEGER NOT NULL",
        ))
        .unwrap();
        conn.execute_batch("INSERT INTO agent_mcp_clients VALUES('client','client','[]',0);
            INSERT INTO agent_mcp_grants VALUES('grant','client','hash','r','s','read_only',7,100,0);
            INSERT INTO agent_control_operations(id,principal,request_key,tool,payload_hash,epoch,state) VALUES ('op','mcp:grant','key','thread_read','h','b','succeeded');
            CREATE TRIGGER reject_owner BEFORE UPDATE OF principal ON agent_control_operations BEGIN SELECT RAISE(ABORT,'synthetic migration failure'); END;").unwrap();
        assert!(migrate(&conn)
            .unwrap_err()
            .contains("synthetic migration failure"));
        assert!(conn.is_autocommit());
        let marker: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_mcp_clients') WHERE name='approved_at')", [], |r|r.get(0)).unwrap();
        assert!(
            !marker,
            "failed owner migration must roll back approval-column/backfill too"
        );
        assert_eq!(
            conn.query_row(
                "SELECT principal FROM agent_control_operations WHERE id='op'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "mcp:grant"
        );
        conn.execute_batch("DROP TRIGGER reject_owner;").unwrap();
        let mut blocker = super::super::open_connection(&path).unwrap();
        let held = blocker
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000, "production admission budget is unchanged");
        let start = std::time::Instant::now();
        let failure = migrate(&conn).unwrap_err();
        println!(
            "MIGRATION_BUSY budget_ms={timeout} elapsed_ms={} error={failure}",
            start.elapsed().as_millis()
        );
        assert!(failure.contains("database is locked"));
        assert!(conn.is_autocommit());
        held.rollback().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT principal FROM agent_control_operations WHERE id='op'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "mcp-client:client"
        );
        let outer = conn.unchecked_transaction().unwrap();
        assert!(migrate(&conn).unwrap_err().contains("transaction"));
        assert!(
            !conn.is_autocommit(),
            "failed nested migration must leave caller transaction owned"
        );
        outer.rollback().unwrap();
    }

    #[test]
    fn migration_concurrent_full_reopens_preserve_both_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.db");
        let seed = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&seed).unwrap();
        drop(seed);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for n in 0..2 {
            let path = path.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let conn = super::super::open_connection(&path).unwrap();
                barrier.wait();
                super::super::create_schema(&conn)?;
                DatabaseStore {
                    conn: Mutex::new(conn),
                }
                .set_setting(&format!("synthetic.reopen.{n}"), "written")
            }));
        }
        for worker in workers {
            assert!(worker.join().unwrap().is_ok());
        }
        let conn = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&conn).unwrap();
        let db = DatabaseStore {
            conn: Mutex::new(conn),
        };
        for n in 0..2 {
            assert_eq!(
                db.get_setting(&format!("synthetic.reopen.{n}")).as_deref(),
                Some("written")
            );
        }
    }

    #[test]
    fn mario_r5_approved_retention_is_independent_bounded_and_not_bearer_authority() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        let mut approved = grant("hour", "synthetic-hour-token");
        approved.expires_at = 3_600_001;
        db.mcp_mint_grant(approved).unwrap();
        let expired = 3_600_002;
        db.mcp_prune_clients(expired, &[]).unwrap();
        assert!(
            db.mcp_client("client").unwrap().is_some(),
            "Token expiry must not delete a recently approved cached registration"
        );
        assert!(db
            .mcp_grant("hour", "https://mcp.example/mcp", expired)
            .unwrap()
            .is_none());
        assert!(db.mcp_list_grants(expired).unwrap().is_empty());
        for i in 1..MAX_CLIENTS {
            let mut spam = client(&format!("spam-{i:03}"));
            spam.created_at = expired;
            db.mcp_register_client(spam, expired).unwrap();
        }
        let mut extra = client("overflow");
        extra.created_at = expired;
        db.mcp_register_client(extra, expired).unwrap();
        assert!(
            db.mcp_client("client").unwrap().is_some(),
            "Anonymous registration pressure must not evict an approved client inside retention"
        );
        let count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_mcp_clients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, MAX_CLIENTS);
        // Seven days since the last successful owner-approved mint, not infinity.
        db.mcp_prune_clients(1 + 7 * 86_400_000, &[]).unwrap();
        assert!(db.mcp_client("client").unwrap().is_none());
        assert!(db.mcp_client("overflow").unwrap().is_none());
    }
    #[test]
    fn client_collision_never_overwrites_registered_metadata() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        for i in 1..MAX_CLIENTS {
            db.mcp_register_client(client(&format!("z{i}")), 0).unwrap();
        }
        let mut changed = client("client");
        changed.name = "impersonator".into();
        assert!(db.mcp_register_client(changed, 0).is_err());
        assert_eq!(
            db.mcp_client("client").unwrap().unwrap().name,
            "Untrusted agent"
        );
    }
    #[test]
    fn registrations_are_bounded_and_recover_by_displacing_ungranted_clients() {
        let db = db();
        for i in 0..MAX_CLIENTS {
            db.mcp_register_client(client(&format!("c{i}")), 0).unwrap();
        }
        assert!(db.mcp_register_client(client("overflow"), 0).is_ok());
        db.mcp_register_client(client("fresh"), 8 * 86_400_000)
            .unwrap();
        assert!(db.mcp_client("c0").unwrap().is_none());
    }
    #[test]
    fn registration_flood_cannot_monopolize_slots_or_displace_live_grants() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        let mut live = grant("live", "synthetic-live");
        live.expires_at = 3_600_000;
        db.mcp_mint_grant(live).unwrap();
        for i in 1..MAX_CLIENTS {
            db.mcp_register_client(client(&format!("spam-{i:03}")), 1)
                .unwrap();
        }
        assert!(
            db.mcp_register_client(client("legitimate"), 2).is_ok(),
            "A full unapproved pool must admit a fresh client without waiting seven days"
        );
        assert!(db.mcp_client("client").unwrap().is_some());
        assert!(db
            .mcp_grant("live", "https://mcp.example/mcp", 2)
            .unwrap()
            .is_some());
        let count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_mcp_clients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, MAX_CLIENTS);
    }

    #[test]
    fn a_pool_of_live_grants_is_never_displaced_by_unapproved_admission() {
        let db = db();
        for i in 0..MAX_CLIENTS {
            let id = format!("live-{i}");
            db.mcp_register_client(client(&id), 0).unwrap();
            let mut granted = grant(&format!("grant-{i}"), &format!("synthetic-{i}"));
            granted.client_id = id;
            granted.expires_at = 3_600_000;
            db.mcp_mint_grant(granted).unwrap();
        }
        assert!(db
            .mcp_register_client(client("unapproved"), 600_000)
            .is_err());
        assert_eq!(
            db.mcp_list_grants(600_000).unwrap().len(),
            MAX_CLIENTS as usize
        );
    }

    #[test]
    fn connector_config_is_persistent_and_not_an_ordinary_browser_setting() {
        let db = db();
        assert!(db.mcp_connector_config().unwrap().is_none());
        let cfg = r#"{"enabled":true,"publicOrigin":"https://mcp.example"}"#;
        db.mcp_connector_set_config(cfg).unwrap();
        assert_eq!(db.mcp_connector_config().unwrap().as_deref(), Some(cfg));
        let tables: Vec<String> = db
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!tables
            .iter()
            .any(|n| n == "settings" || n == "web_remote_sessions"));
    }
    #[test]
    fn restart_preserves_grants_and_revocation_without_plaintext_tokens() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let open = || {
            let conn = Connection::open(file.path()).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            DatabaseStore {
                conn: Mutex::new(conn),
            }
        };
        let db = open();
        db.mcp_register_client(client("client"), 0).unwrap();
        db.mcp_mint_grant(grant("grant", "synthetic-restart-token"))
            .unwrap();
        let stored: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT token_hash FROM agent_mcp_grants WHERE id='grant'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, credential_hash("synthetic-restart-token"));
        drop(db);
        let db = open();
        assert!(db
            .mcp_grant("grant", "https://mcp.example/mcp", 2)
            .unwrap()
            .is_some());
        db.mcp_revoke_client("client").unwrap();
        drop(db);
        assert!(open()
            .mcp_grant("grant", "https://mcp.example/mcp", 2)
            .unwrap()
            .is_none());
    }
    #[test]
    fn tokens_are_hashed_resource_bound_expiring_and_revocable() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        let token = "synthetic-unit-token-not-a-real-credential";
        db.mcp_mint_grant(grant("grant", token)).unwrap();
        let hash = credential_hash(token);
        assert!(db
            .mcp_grant_by_hash(token, "https://mcp.example/mcp", 2)
            .unwrap()
            .is_none());
        assert!(db
            .mcp_grant_by_hash(&hash, "https://evil.example/mcp", 2)
            .unwrap()
            .is_none());
        assert!(db
            .mcp_grant("grant", "https://mcp.example/mcp", 100)
            .unwrap()
            .is_none());
        assert!(db
            .mcp_grant_by_hash(&hash, "https://mcp.example/mcp", 2)
            .unwrap()
            .is_some());
        db.mcp_revoke_client("client").unwrap();
        assert!(db
            .mcp_grant_by_hash(&hash, "https://mcp.example/mcp", 2)
            .unwrap()
            .is_none());
        assert!(db.mcp_list_grants(2).unwrap().is_empty());
    }
    #[test]
    fn failed_mint_does_not_revoke_prior_grant_but_new_mint_does() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        db.mcp_mint_grant(grant("first", "one")).unwrap();
        assert!(db.mcp_mint_grant(grant("first", "two")).is_err());
        assert!(db
            .mcp_grant("first", "https://mcp.example/mcp", 2)
            .unwrap()
            .is_some());
        db.mcp_mint_grant(grant("second", "two")).unwrap();
        assert!(db
            .mcp_grant("first", "https://mcp.example/mcp", 2)
            .unwrap()
            .is_none());
        assert_eq!(db.mcp_list_grants(2).unwrap().len(), 1);
    }
}
