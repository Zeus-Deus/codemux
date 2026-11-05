//! Independent MCP client registrations and hash-at-rest grants. No web sessions.
//!
//! A grant row is one sign-in. Its access token and every refresh token are
//! stored only as `credential_hash`es; callers pass hashes in and plaintext
//! never reaches this module. A refresh rotates the same grant row, so the
//! grant id and principal survive it. Personal tokens are grants of a
//! synthetic client, one client per label.
use super::DatabaseStore;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

pub const MAX_CLIENTS: i64 = 256;
pub const UNAPPROVED_CLIENT_TTL_MS: i64 = 600_000;
// Registration metadata survives short-lived tokens, but is not bearer authority.
pub const APPROVED_CLIENT_RETENTION_MS: i64 = 7 * 86_400_000;
/// Personal tokens always expire: between one hour and a year after issue.
pub const PERSONAL_TOKEN_MIN_TTL_MS: i64 = 3_600_000;
pub const PERSONAL_TOKEN_MAX_TTL_MS: i64 = 365 * 86_400_000;
pub const PERSONAL_TOKEN_DEFAULT_TTL_MS: i64 = 30 * 86_400_000;
/// Synthetic client ids for personal tokens start with this.
pub const PERSONAL_CLIENT_PREFIX: &str = "cmcp_token_";
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
CREATE TABLE IF NOT EXISTS agent_mcp_refresh_tokens (
    token_hash TEXT PRIMARY KEY NOT NULL,
    grant_id TEXT NOT NULL REFERENCES agent_mcp_grants(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER
);
CREATE INDEX IF NOT EXISTS agent_mcp_refresh_tokens_grant ON agent_mcp_refresh_tokens(grant_id);
CREATE TABLE IF NOT EXISTS agent_mcp_personal_clients (
    client_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_mcp_clients(id) ON DELETE CASCADE,
    label TEXT NOT NULL UNIQUE COLLATE NOCASE
);
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
    /// When the current access token expires.
    pub expires_at: i64,
    /// The personal-token label; `None` for OAuth sign-ins.
    pub label: Option<String>,
    /// Expiry of the unspent refresh token; `None` when the grant cannot renew.
    pub refresh_expires_at: Option<i64>,
}

/// A refresh token to store with a grant. `token_hash` is `credential_hash(token)`.
pub struct NewMcpRefresh {
    pub token_hash: String,
    pub expires_at: i64,
}

/// One `refresh_token` grant request, already shape-checked by `oauth::RefreshInput`.
pub struct RefreshRotation<'a> {
    pub client_id: &'a str,
    pub presented_hash: &'a str,
    pub resource: &'a str,
    pub now: i64,
    /// The replacement access token's hash and expiry.
    pub access_hash: String,
    pub access_expires_at: i64,
    /// The replacement refresh token.
    pub refresh: NewMcpRefresh,
    /// How long a spent refresh token may be presented again without
    /// revoking the grant (two processes refreshing one stored token).
    pub reuse_grace_ms: i64,
}

pub enum RefreshOutcome {
    /// The grant now holds the new access and refresh tokens.
    Rotated(McpGrant),
    /// Unknown, expired, spent within the grace window, revoked, or bound to
    /// another client or resource. Nothing changed.
    Invalid,
    /// A spent token came back after the grace window. The grant, its access
    /// token and every refresh token are revoked.
    FamilyRevoked {
        grant_id: String,
        client_name: String,
    },
}

/// A personal token to issue. `token_hash` is `credential_hash(token)`.
pub struct NewPersonalToken {
    /// Trimmed by the store before the label rule applies.
    pub label: String,
    pub access: String,
    pub resource: String,
    pub token_hash: String,
    pub now: i64,
    pub expires_at: i64,
    /// Clients with an authorization in flight, which registering the label's
    /// synthetic client must not prune or displace (as for OAuth registration).
    pub protected_clients: Vec<String>,
}

pub struct IssuedPersonalToken {
    pub client_id: String,
    pub grant_id: String,
    /// An unexpired, unrevoked token for this label was revoked by this issue.
    pub replaced: bool,
}

/// Personal-token labels: 1-64 ASCII letters, digits, spaces and `._@-`,
/// starting with a letter or digit and with no trailing space. Unique without
/// regard to case. Pass the trimmed label, as `mcp_issue_personal_token` does.
pub fn personal_label_valid(label: &str) -> bool {
    let mut chars = label.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || " ._@-".contains(c))
        && label.len() <= 64
        && !label.ends_with(' ')
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
        label: row.get(8)?,
        refresh_expires_at: row.get(9)?,
    })
}
const GRANT_SELECT: &str = "SELECT g.id,g.client_id,c.name,g.redirect_uri,g.resource,g.access,g.created_at,g.expires_at,p.label,(SELECT MAX(r.expires_at) FROM agent_mcp_refresh_tokens r WHERE r.grant_id=g.id AND r.used_at IS NULL) FROM agent_mcp_grants g JOIN agent_mcp_clients c ON c.id=g.client_id LEFT JOIN agent_mcp_personal_clients p ON p.client_id=g.client_id";

/// The one definition of a grant that still carries authority, over alias `g`:
/// not revoked, and its access token or an unspent refresh token is unexpired.
/// `now` is the SQL placeholder for the current time, such as `"?3"`. Every
/// other grant is dead and is deleted on the next prune.
fn alive(now: &str) -> String {
    format!("(g.revoked=0 AND (g.expires_at>{now} OR EXISTS(SELECT 1 FROM agent_mcp_refresh_tokens r WHERE r.grant_id=g.id AND r.used_at IS NULL AND r.expires_at>{now})))")
}
/// Deletes every dead grant at `?1`; their refresh tokens cascade.
fn delete_dead_grants_sql() -> String {
    format!(
        "DELETE FROM agent_mcp_grants WHERE id IN (SELECT g.id FROM agent_mcp_grants g WHERE NOT {})",
        alive("?1")
    )
}

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
    // A grant whose access token lapsed but which can still refresh is alive.
    conn.execute(&delete_dead_grants_sql(), [now])
        .map_err(|_| "MCP pruning failed")?;
    // Spent tokens stay until their own expiry so a replay is still detected.
    conn.execute(
        "DELETE FROM agent_mcp_refresh_tokens WHERE expires_at<=?1",
        [now],
    )
    .map_err(|_| "MCP pruning failed")?;
    conn.execute("DELETE FROM agent_mcp_clients WHERE ((approved_at IS NULL AND created_at<=?1) OR approved_at<=?3) AND id NOT IN (SELECT value FROM json_each(?2)) AND NOT EXISTS(SELECT 1 FROM agent_mcp_grants g WHERE g.client_id=agent_mcp_clients.id)", params![now.saturating_sub(UNAPPROVED_CLIENT_TTL_MS), protected, now.saturating_sub(APPROVED_CLIENT_RETENTION_MS)])
        .map_err(|_| "MCP pruning failed")?;
    Ok(())
}

/// Registers a client inside the caller's transaction. `protected` is a JSON
/// array of client ids that pruning and flood displacement must keep.
fn register_client_tx(
    tx: &rusqlite::Connection,
    client: &NewMcpClient,
    now: i64,
    protected: &str,
) -> Result<(), String> {
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
    prune_clients(tx, now, protected)?;
    let count: i64 = tx
        .query_row("SELECT COUNT(*) FROM agent_mcp_clients", [], |r| r.get(0))
        .map_err(|_| "Client registration failed")?;
    if count >= MAX_CLIENTS {
        // Recover from an unauthenticated registration flood without
        // displacing a retained approved client. Admission stays bounded.
        let removed = tx.execute("DELETE FROM agent_mcp_clients WHERE id=(SELECT c.id FROM agent_mcp_clients c WHERE c.approved_at IS NULL AND NOT EXISTS(SELECT 1 FROM agent_mcp_grants g WHERE g.client_id=c.id) AND c.id NOT IN (SELECT value FROM json_each(?1)) ORDER BY c.created_at,c.id LIMIT 1)", [protected])
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
    Ok(())
}

/// Mints a grant inside the caller's transaction, revoking the client's
/// earlier grants and their refresh families.
fn mint_grant_tx(
    tx: &rusqlite::Connection,
    grant: &NewMcpGrant,
    refresh: Option<&NewMcpRefresh>,
) -> Result<(), String> {
    // One active grant per public client. Reauthorization cannot leave an
    // earlier, more privileged token alive. Roll back on insertion failure.
    tx.execute(
        "UPDATE agent_mcp_grants SET revoked=1 WHERE client_id=?1",
        [&grant.client_id],
    )
    .map_err(|_| "Grant persistence failed")?;
    tx.execute("INSERT INTO agent_mcp_grants(id,client_id,token_hash,redirect_uri,resource,access,created_at,expires_at,revoked) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",params![grant.id,grant.client_id,grant.token_hash,grant.redirect_uri,grant.resource,grant.access,grant.created_at,grant.expires_at]).map_err(|_|"Grant persistence failed".to_string())?;
    if let Some(refresh) = refresh {
        tx.execute(
            "INSERT INTO agent_mcp_refresh_tokens(token_hash,grant_id,created_at,expires_at) VALUES(?1,?2,?3,?4)",
            params![refresh.token_hash, grant.id, grant.created_at, refresh.expires_at],
        )
        .map_err(|_| "Grant persistence failed")?;
    }
    tx.execute(
        "UPDATE agent_mcp_clients SET approved_at=?2 WHERE id=?1",
        params![grant.client_id, grant.created_at],
    )
    .map_err(|_| "Grant persistence failed")?;
    // No tombstone is required for an opaque grant: absence fails closed.
    // The revoked grants' refresh tokens cascade with them.
    tx.execute(&delete_dead_grants_sql(), [grant.created_at])
        .map_err(|_| "Grant persistence failed")?;
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
        register_client_tx(&tx, &client, now, &protected)?;
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
        self.mcp_mint_grant_with_refresh(grant, None)
    }
    /// Mints an owner-approved grant, optionally with its first refresh token.
    pub fn mcp_mint_grant_with_refresh(
        &self,
        grant: NewMcpGrant,
        refresh: Option<NewMcpRefresh>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let tx = conn.transaction().map_err(|_| "Grant persistence failed")?;
        mint_grant_tx(&tx, &grant, refresh.as_ref())?;
        tx.commit().map_err(|_| "Grant persistence failed")?;
        Ok(())
    }
    /// Spends one refresh token and installs the replacement access and
    /// refresh tokens on the same grant row. Never bumps `approved_at`: a
    /// refresh is not owner approval.
    pub fn mcp_rotate_refresh(&self, r: RefreshRotation<'_>) -> Result<RefreshOutcome, String> {
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        // Immediate: the read below decides the write, and a spurious BUSY
        // here would cost the client its sign-in.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "Grant refresh failed")?;
        let Some((grant_id, client_id, resource, revoked, client_name, expires_at, used_at)) = tx
            .query_row(
                "SELECT g.id,g.client_id,g.resource,g.revoked,c.name,r.expires_at,r.used_at FROM agent_mcp_refresh_tokens r JOIN agent_mcp_grants g ON g.id=r.grant_id JOIN agent_mcp_clients c ON c.id=g.client_id WHERE r.token_hash=?1",
                [r.presented_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| "Grant refresh failed")?
        else {
            return Ok(RefreshOutcome::Invalid);
        };
        if revoked || client_id != r.client_id || resource != r.resource {
            return Ok(RefreshOutcome::Invalid);
        }
        if let Some(used_at) = used_at {
            if r.now.saturating_sub(used_at) <= r.reuse_grace_ms {
                return Ok(RefreshOutcome::Invalid);
            }
            // A spent token is back: assume it leaked and end the whole family.
            tx.execute(
                "UPDATE agent_mcp_grants SET revoked=1 WHERE id=?1",
                [&grant_id],
            )
            .map_err(|_| "Grant refresh failed")?;
            tx.commit().map_err(|_| "Grant refresh failed")?;
            return Ok(RefreshOutcome::FamilyRevoked {
                grant_id,
                client_name,
            });
        }
        if expires_at <= r.now {
            return Ok(RefreshOutcome::Invalid);
        }
        tx.execute(
            "UPDATE agent_mcp_refresh_tokens SET used_at=?2 WHERE token_hash=?1",
            params![r.presented_hash, r.now],
        )
        .map_err(|_| "Grant refresh failed")?;
        tx.execute(
            "INSERT INTO agent_mcp_refresh_tokens(token_hash,grant_id,created_at,expires_at) VALUES(?1,?2,?3,?4)",
            params![r.refresh.token_hash, grant_id, r.now, r.refresh.expires_at],
        )
        .map_err(|_| "Grant refresh failed")?;
        tx.execute(
            "UPDATE agent_mcp_grants SET token_hash=?2,expires_at=?3 WHERE id=?1",
            params![grant_id, r.access_hash, r.access_expires_at],
        )
        .map_err(|_| "Grant refresh failed")?;
        let grant = tx
            .query_row(
                &format!("{GRANT_SELECT} WHERE g.id=?1"),
                [&grant_id],
                grant_row,
            )
            .map_err(|_| "Grant refresh failed")?;
        tx.commit().map_err(|_| "Grant refresh failed")?;
        Ok(RefreshOutcome::Rotated(grant))
    }
    /// Issues a personal token for `t.label`. The first issue for a label
    /// registers its synthetic client; a later one keeps that client id, so
    /// receipts survive, and revokes only that label's previous token.
    pub fn mcp_issue_personal_token(
        &self,
        t: NewPersonalToken,
    ) -> Result<IssuedPersonalToken, String> {
        // A pasted label's surrounding whitespace is not part of its name.
        let label = t.label.trim();
        if !personal_label_valid(label) {
            return Err("Token labels are 1-64 letters, digits, spaces or . _ @ -, starting with a letter or digit".into());
        }
        let lifetime = t.expires_at.saturating_sub(t.now);
        if !(PERSONAL_TOKEN_MIN_TTL_MS..=PERSONAL_TOKEN_MAX_TTL_MS).contains(&lifetime) {
            return Err("Token expiry must be between 1 hour and 365 days".into());
        }
        let protected =
            serde_json::to_string(&t.protected_clients).map_err(|_| "Invalid protected clients")?;
        let mut conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "Token persistence failed")?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT client_id FROM agent_mcp_personal_clients WHERE label=?1",
                [label],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Token persistence failed")?;
        let (client_id, replaced) = match existing {
            Some(client_id) => {
                let replaced = tx
                    .query_row(
                        &format!(
                            "SELECT EXISTS(SELECT 1 FROM agent_mcp_grants g WHERE g.client_id=?1 AND {})",
                            alive("?2")
                        ),
                        params![client_id, t.now],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(|_| "Token persistence failed")?;
                (client_id, replaced)
            }
            None => {
                let client = NewMcpClient {
                    id: format!(
                        "{PERSONAL_CLIENT_PREFIX}{}",
                        crate::mcp_connector::oauth::opaque()
                    ),
                    name: label.to_string(),
                    redirects: Vec::new(),
                    created_at: t.now,
                };
                register_client_tx(&tx, &client, t.now, &protected)?;
                tx.execute(
                    "INSERT INTO agent_mcp_personal_clients(client_id,label) VALUES(?1,?2)",
                    params![client.id, label],
                )
                .map_err(|_| "Token persistence failed")?;
                (client.id, false)
            }
        };
        let grant = NewMcpGrant {
            id: crate::mcp_connector::oauth::opaque(),
            client_id: client_id.clone(),
            token_hash: t.token_hash,
            redirect_uri: String::new(),
            resource: t.resource,
            access: t.access,
            created_at: t.now,
            expires_at: t.expires_at,
        };
        mint_grant_tx(&tx, &grant, None)?;
        tx.commit().map_err(|_| "Token persistence failed")?;
        Ok(IssuedPersonalToken {
            client_id,
            grant_id: grant.id,
            replaced,
        })
    }
    /// Revokes every unrevoked grant: OAuth sign-ins with their refresh
    /// families, and personal tokens. Returns how many rows changed, which can
    /// include dead grants the last prune has not removed yet.
    pub fn mcp_revoke_all(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.execute("UPDATE agent_mcp_grants SET revoked=1 WHERE revoked=0", [])
            .map_err(|_| "Grant revocation failed".into())
    }
    /// Revokes every unrevoked grant bound to another resource, after the
    /// issuer changed. Counts like `mcp_revoke_all`.
    pub fn mcp_revoke_grants_not_bound_to(&self, resource: &str) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.execute(
            "UPDATE agent_mcp_grants SET revoked=1 WHERE revoked=0 AND resource<>?1",
            [resource],
        )
        .map_err(|_| "Grant revocation failed".into())
    }
    /// When the owner last approved this client, if ever. A refresh never
    /// changes it.
    pub fn mcp_client_approved_at(&self, id: &str) -> Result<Option<i64>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(
            "SELECT approved_at FROM agent_mcp_clients WHERE id=?1",
            [id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .map(Option::flatten)
        .map_err(|_| "Client lookup failed".into())
    }
    /// Bearer authentication: the access token itself must be unexpired.
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
    /// Caller re-validation: the grant is alive, so work admitted under it
    /// survives an access-token lapse while the client can still refresh.
    pub fn mcp_grant(
        &self,
        id: &str,
        resource: &str,
        now: i64,
    ) -> Result<Option<McpGrant>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        conn.query_row(
            &format!(
                "{GRANT_SELECT} WHERE g.id=?1 AND g.resource=?2 AND {}",
                alive("?3")
            ),
            params![id, resource, now],
            grant_row,
        )
        .optional()
        .map_err(|_| "Grant lookup failed".to_string())
    }
    pub fn mcp_list_grants(&self, now: i64) -> Result<Vec<McpGrant>, String> {
        let conn = self.conn.lock().map_err(|_| "MCP store unavailable")?;
        let mut statement = conn
            .prepare(&format!(
                "{GRANT_SELECT} WHERE {} ORDER BY g.created_at DESC",
                alive("?1")
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
                super::super::SCHEMA_VERSION
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

    // Refresh families and personal tokens.

    const RESOURCE: &str = "https://mcp.example/mcp";
    const GRACE: i64 = 30_000;
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 86_400_000;

    /// An OAuth sign-in as the code exchange mints it: a 1 h access token and
    /// a 30-day refresh token.
    fn sign_in(db: &DatabaseStore, id: &str, access: &str, refresh: &str, now: i64) {
        let mut minted = grant(id, access);
        minted.created_at = now;
        minted.expires_at = now + HOUR;
        db.mcp_mint_grant_with_refresh(
            minted,
            Some(NewMcpRefresh {
                token_hash: credential_hash(refresh),
                expires_at: now + 30 * DAY,
            }),
        )
        .unwrap();
    }
    fn rotate_as(
        db: &DatabaseStore,
        client_id: &str,
        resource: &str,
        presented: &str,
        replacement: (&str, &str),
        now: i64,
    ) -> RefreshOutcome {
        let presented = credential_hash(presented);
        db.mcp_rotate_refresh(RefreshRotation {
            client_id,
            presented_hash: &presented,
            resource,
            now,
            access_hash: credential_hash(replacement.0),
            access_expires_at: now + HOUR,
            refresh: NewMcpRefresh {
                token_hash: credential_hash(replacement.1),
                expires_at: now + 30 * DAY,
            },
            reuse_grace_ms: GRACE,
        })
        .unwrap()
    }
    fn rotate(
        db: &DatabaseStore,
        presented: &str,
        replacement: (&str, &str),
        now: i64,
    ) -> RefreshOutcome {
        rotate_as(db, "client", RESOURCE, presented, replacement, now)
    }
    fn outcome(o: &RefreshOutcome) -> &'static str {
        match o {
            RefreshOutcome::Rotated(_) => "rotated",
            RefreshOutcome::Invalid => "invalid",
            RefreshOutcome::FamilyRevoked { .. } => "family_revoked",
        }
    }
    /// (token_hash, used_at) of a grant's refresh tokens, oldest first.
    fn refresh_rows(db: &DatabaseStore, grant_id: &str) -> Vec<(String, Option<i64>)> {
        db.conn
            .lock()
            .unwrap()
            .prepare("SELECT token_hash,used_at FROM agent_mcp_refresh_tokens WHERE grant_id=?1 ORDER BY created_at,token_hash")
            .unwrap()
            .query_map([grant_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    fn count(db: &DatabaseStore, table: &str) -> i64 {
        db.conn
            .lock()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn by_hash(db: &DatabaseStore, token: &str, now: i64) -> Option<McpGrant> {
        db.mcp_grant_by_hash(&credential_hash(token), RESOURCE, now)
            .unwrap()
    }
    fn personal(label: &str, token: &str, now: i64) -> NewPersonalToken {
        NewPersonalToken {
            label: label.into(),
            access: "supervised".into(),
            resource: RESOURCE.into(),
            token_hash: credential_hash(token),
            now,
            expires_at: now + PERSONAL_TOKEN_DEFAULT_TTL_MS,
            protected_clients: Vec::new(),
        }
    }

    #[test]
    fn refresh_rotation_replaces_access_and_issues_one_new_refresh() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 10);
        assert_eq!(
            db.mcp_list_grants(11).unwrap()[0].refresh_expires_at,
            Some(10 + 30 * DAY)
        );
        let RefreshOutcome::Rotated(rotated) =
            rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1_000)
        else {
            panic!("a live refresh token must rotate");
        };
        assert_eq!(
            (rotated.id.as_str(), rotated.client_id.as_str()),
            ("grant", "client"),
            "a refresh keeps the grant, so its principal and receipts survive"
        );
        assert_eq!(rotated.expires_at, 1_000 + HOUR);
        assert_eq!(rotated.refresh_expires_at, Some(1_000 + 30 * DAY));
        assert_eq!(rotated.label, None);
        assert!(by_hash(&db, "access-1", 1_001).is_none());
        assert_eq!(by_hash(&db, "access-2", 1_001).unwrap().id, "grant");
        assert_eq!(
            refresh_rows(&db, "grant"),
            vec![
                (credential_hash("refresh-1"), Some(1_000)),
                (credential_hash("refresh-2"), None)
            ]
        );
        assert_eq!(
            db.mcp_client_approved_at("client").unwrap(),
            Some(10),
            "a refresh is not owner approval"
        );
        assert_eq!(
            outcome(&rotate(&db, "refresh-2", ("access-3", "refresh-3"), 2_000)),
            "rotated"
        );
        assert_eq!(db.mcp_list_grants(2_001).unwrap().len(), 1);
    }

    #[test]
    fn refresh_reuse_after_grace_revokes_the_whole_family() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1_000)),
            "rotated"
        );
        let replay = 1_000 + GRACE + 1;
        match rotate(&db, "refresh-1", ("access-x", "refresh-x"), replay) {
            RefreshOutcome::FamilyRevoked {
                grant_id,
                client_name,
            } => assert_eq!(
                (grant_id.as_str(), client_name.as_str()),
                ("grant", "Untrusted agent")
            ),
            other => panic!("replay after the grace window was {}", outcome(&other)),
        }
        assert!(db.mcp_grant("grant", RESOURCE, replay).unwrap().is_none());
        assert!(db.mcp_list_grants(replay).unwrap().is_empty());
        assert!(by_hash(&db, "access-2", replay).is_none());
        assert!(by_hash(&db, "access-x", replay).is_none());
        assert_eq!(
            outcome(&rotate(&db, "refresh-2", ("access-3", "refresh-3"), replay)),
            "invalid"
        );
        // A revoked family stays revoked; a second replay changes nothing.
        assert_eq!(
            outcome(&rotate(
                &db,
                "refresh-1",
                ("access-4", "refresh-4"),
                replay + 1
            )),
            "invalid"
        );
        assert!(!refresh_rows(&db, "grant")
            .iter()
            .any(|(hash, _)| hash == &credential_hash("refresh-x")));
        db.mcp_prune_clients(replay, &[]).unwrap();
        assert_eq!(count(&db, "agent_mcp_grants"), 0);
        assert_eq!(count(&db, "agent_mcp_refresh_tokens"), 0);
    }

    #[test]
    fn refresh_reuse_inside_grace_fails_without_revoking() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1_000)),
            "rotated"
        );
        for replay in [2_000, 1_000 + GRACE] {
            assert_eq!(
                outcome(&rotate(&db, "refresh-1", ("access-x", "refresh-x"), replay)),
                "invalid"
            );
        }
        assert!(db
            .mcp_grant("grant", RESOURCE, 1_000 + GRACE)
            .unwrap()
            .is_some());
        assert!(by_hash(&db, "access-2", 1_000 + GRACE).is_some());
        assert_eq!(
            outcome(&rotate(
                &db,
                "refresh-2",
                ("access-3", "refresh-3"),
                1_000 + GRACE + 1
            )),
            "rotated"
        );
    }

    #[test]
    fn refresh_is_bound_to_client_and_resource() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        db.mcp_register_client(client("other"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        for (client_id, resource) in [
            ("other", RESOURCE),
            ("client", "https://evil.example/mcp"),
            ("client", "https://mcp.example/mcp/"),
        ] {
            assert_eq!(
                outcome(&rotate_as(
                    &db,
                    client_id,
                    resource,
                    "refresh-1",
                    ("access-x", "refresh-x"),
                    1_000
                )),
                "invalid",
                "{client_id} at {resource}"
            );
        }
        assert!(db.mcp_grant("grant", RESOURCE, 1_000).unwrap().is_some());
        assert!(by_hash(&db, "access-1", 1_000).is_some());
        assert_eq!(
            refresh_rows(&db, "grant"),
            vec![(credential_hash("refresh-1"), None)],
            "a mismatch must not spend the token"
        );
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1_000)),
            "rotated"
        );
    }

    #[test]
    fn refreshable_grants_survive_access_expiry_in_prune_list_and_validation() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        let lapsed = HOUR + 1;
        db.mcp_prune_clients(lapsed, &[]).unwrap();
        let listed = db.mcp_list_grants(lapsed).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].expires_at, HOUR);
        assert_eq!(listed[0].refresh_expires_at, Some(30 * DAY));
        assert!(db.mcp_grant("grant", RESOURCE, lapsed).unwrap().is_some());
        assert!(
            by_hash(&db, "access-1", lapsed).is_none(),
            "an expired access token is still refused as a bearer"
        );
        // Past client retention the registration stays, because its grant can renew.
        db.mcp_prune_clients(8 * DAY, &[]).unwrap();
        assert!(db.mcp_client("client").unwrap().is_some());
        assert_eq!(
            outcome(&rotate(
                &db,
                "refresh-1",
                ("access-2", "refresh-2"),
                8 * DAY
            )),
            "rotated"
        );
        assert!(by_hash(&db, "access-2", 8 * DAY + 1).is_some());
    }

    #[test]
    fn refresh_family_lapses_when_unused_for_its_ttl() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), DAY)),
            "rotated"
        );
        // The spent token is kept for replay detection until its own expiry.
        db.mcp_prune_clients(30 * DAY, &[]).unwrap();
        assert_eq!(
            refresh_rows(&db, "grant"),
            vec![(credential_hash("refresh-2"), None)]
        );
        assert!(db.mcp_grant("grant", RESOURCE, 30 * DAY).unwrap().is_some());
        let lapsed = 31 * DAY;
        assert!(db.mcp_grant("grant", RESOURCE, lapsed).unwrap().is_none());
        assert!(db.mcp_list_grants(lapsed).unwrap().is_empty());
        assert_eq!(
            outcome(&rotate(&db, "refresh-2", ("access-3", "refresh-3"), lapsed)),
            "invalid"
        );
        db.mcp_prune_clients(lapsed, &[]).unwrap();
        assert_eq!(count(&db, "agent_mcp_grants"), 0);
        assert_eq!(count(&db, "agent_mcp_refresh_tokens"), 0);
        assert!(db.mcp_client("client").unwrap().is_none());
    }

    #[test]
    fn reauthorization_revokes_the_previous_refresh_family() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "first", "access-1", "refresh-1", 0);
        sign_in(&db, "second", "access-2", "refresh-2", 1_000);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-x", "refresh-x"), 2_000)),
            "invalid"
        );
        assert!(refresh_rows(&db, "first").is_empty());
        let listed = db.mcp_list_grants(2_000).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "second");
        assert_eq!(
            outcome(&rotate(&db, "refresh-2", ("access-3", "refresh-3"), 2_000)),
            "rotated"
        );
    }

    /// Fails if any stored text contains one of the token bodies.
    fn assert_no_plaintext(db: &DatabaseStore, tokens: &[&str]) {
        let bodies: Vec<&str> = tokens.iter().map(|t| &t[5..]).collect();
        let conn = db.conn.lock().unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut cells = 0;
        for table in tables {
            let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..columns {
                    let text = match row.get::<_, rusqlite::types::Value>(i).unwrap() {
                        rusqlite::types::Value::Text(text) => text,
                        rusqlite::types::Value::Blob(blob) => {
                            String::from_utf8_lossy(&blob).into_owned()
                        }
                        _ => continue,
                    };
                    cells += 1;
                    for body in &bodies {
                        assert!(
                            !text.contains(body),
                            "{table} column {i} holds a plaintext token"
                        );
                    }
                }
            }
        }
        assert!(cells > 0);
    }

    #[test]
    fn refresh_tokens_are_hashed_at_rest() {
        use crate::mcp_connector::oauth::{access_token, refresh_token};
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        let (access, refresh) = (access_token(), refresh_token());
        sign_in(&db, "grant", &access, &refresh, 0);
        let (next_access, next_refresh) = (access_token(), refresh_token());
        assert_eq!(
            outcome(&rotate(
                &db,
                &refresh,
                (next_access.as_str(), next_refresh.as_str()),
                1_000
            )),
            "rotated"
        );
        let personal_token = access_token();
        db.mcp_issue_personal_token(personal("hermes", &personal_token, 1_000))
            .unwrap();
        let stored: Vec<String> = refresh_rows(&db, "grant")
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(
            stored,
            vec![credential_hash(&refresh), credential_hash(&next_refresh)]
        );
        assert!(by_hash(&db, &personal_token, 1_001).is_some());
        assert_no_plaintext(
            &db,
            &[
                access.as_str(),
                refresh.as_str(),
                next_access.as_str(),
                next_refresh.as_str(),
                personal_token.as_str(),
            ],
        );
    }

    #[test]
    fn personal_tokens_get_one_synthetic_client_per_label() {
        let db = db();
        let issue = |label: &str, token: &str, now: i64| {
            db.mcp_issue_personal_token(personal(label, token, now))
                .unwrap()
        };
        let hermes = issue("hermes", "token-hermes-1", 0);
        let ci = issue("ci-bot", "token-ci", 0);
        assert!(!hermes.replaced && !ci.replaced);
        assert!(hermes.client_id.starts_with(PERSONAL_CLIENT_PREFIX));
        assert_ne!(hermes.client_id, ci.client_id);
        let stored = db.mcp_client(&hermes.client_id).unwrap().unwrap();
        assert_eq!(stored.name, "hermes");
        assert!(stored.redirects.is_empty());

        let reissued = issue("hermes", "token-hermes-2", 1_000);
        assert!(reissued.replaced);
        assert_eq!(
            reissued.client_id, hermes.client_id,
            "the principal, and so its receipts, survive a reissue"
        );
        assert_ne!(reissued.grant_id, hermes.grant_id);
        assert!(by_hash(&db, "token-hermes-1", 1_001).is_none());
        let current = by_hash(&db, "token-hermes-2", 1_001).unwrap();
        assert_eq!(current.id, reissued.grant_id);
        assert_eq!(current.label.as_deref(), Some("hermes"));
        assert_eq!(current.access, "supervised");
        assert_eq!(current.redirect_uri, "");
        assert_eq!(current.refresh_expires_at, None);
        assert_eq!(current.expires_at, 1_000 + PERSONAL_TOKEN_DEFAULT_TTL_MS);
        assert_eq!(
            by_hash(&db, "token-ci", 1_001).unwrap().id,
            ci.grant_id,
            "reissuing one label leaves other bots alone"
        );
        let mut labels: Vec<_> = db
            .mcp_list_grants(1_001)
            .unwrap()
            .into_iter()
            .map(|g| g.label.unwrap())
            .collect();
        labels.sort();
        assert_eq!(labels, ["ci-bot", "hermes"]);
        assert_eq!(
            db.mcp_client_approved_at(&hermes.client_id).unwrap(),
            Some(1_000)
        );

        // A label whose token was revoked keeps its client, with nothing replaced.
        db.mcp_revoke_client(&ci.client_id).unwrap();
        let ci_again = issue("ci-bot", "token-ci-2", 2_000);
        assert!(!ci_again.replaced);
        assert_eq!(ci_again.client_id, ci.client_id);
    }

    #[test]
    fn personal_labels_are_case_insensitive_unique_and_validated() {
        let db = db();
        let first = db
            .mcp_issue_personal_token(personal("Hermes Bot", "token-1", 0))
            .unwrap();
        let again = db
            .mcp_issue_personal_token(personal("hermes BOT", "token-2", 1))
            .unwrap();
        assert_eq!(again.client_id, first.client_id);
        assert!(again.replaced);
        assert_eq!(
            by_hash(&db, "token-2", 2).unwrap().label.as_deref(),
            Some("Hermes Bot"),
            "the first spelling stays the label"
        );
        for bad in [
            "".to_string(),
            "   ".into(),
            " -hermes ".into(),
            "-hermes".into(),
            ".hermes".into(),
            "a".repeat(65),
            "bad\nlabel".into(),
            "tab\tlabel".into(),
            "semi;colon".into(),
            "slash/label".into(),
            "ünicode".into(),
        ] {
            assert!(
                db.mcp_issue_personal_token(personal(&bad, "token-bad", 2))
                    .is_err(),
                "accepted {bad:?}"
            );
        }
        assert_eq!(count(&db, "agent_mcp_personal_clients"), 1);
        for good in [
            "a".to_string(),
            "9".into(),
            "ci-bot".into(),
            "x@y.z_1".into(),
            "Hermes Bot 2".into(),
            "a".repeat(64),
        ] {
            db.mcp_issue_personal_token(personal(&good, &format!("token-{good}"), 3))
                .unwrap_or_else(|e| panic!("refused {good:?}: {e}"));
        }
        // Surrounding whitespace is trimmed, so a pasted label names one bot.
        let padded = db
            .mcp_issue_personal_token(personal(" hermes ", "token-padded", 4))
            .unwrap();
        let stored = by_hash(&db, "token-padded", 5).unwrap();
        assert_eq!(
            (stored.label.as_deref(), stored.client_name.as_str()),
            (Some("hermes"), "hermes")
        );
        let reissued = db
            .mcp_issue_personal_token(personal("Hermes\n", "token-padded-2", 5))
            .unwrap();
        assert_eq!(reissued.client_id, padded.client_id);
        assert!(reissued.replaced);
        // The store itself refuses a second spelling for another client.
        db.mcp_register_client(client("other"), 0).unwrap();
        assert!(db
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_mcp_personal_clients(client_id,label) VALUES('other','HERMES BOT')",
                []
            )
            .is_err());
    }

    #[test]
    fn personal_clients_are_never_displaced_by_registration_floods() {
        let db = db();
        let hermes = db
            .mcp_issue_personal_token(personal("hermes", "token-hermes", 0))
            .unwrap();
        for i in 1..MAX_CLIENTS {
            db.mcp_register_client(client(&format!("spam-{i:03}")), 1)
                .unwrap();
        }
        db.mcp_register_client(client("overflow"), 2).unwrap();
        // A new label is admitted into a pool full of unapproved clients.
        let ci = db
            .mcp_issue_personal_token(personal("ci-bot", "token-ci", 3))
            .unwrap();
        assert_eq!(count(&db, "agent_mcp_clients"), MAX_CLIENTS);
        for id in [&hermes.client_id, &ci.client_id] {
            assert!(db.mcp_client(id).unwrap().is_some());
        }
        assert!(by_hash(&db, "token-hermes", 4).is_some());
        assert!(by_hash(&db, "token-ci", 4).is_some());
        // Later floods, after the unapproved TTL, still leave both.
        let later = UNAPPROVED_CLIENT_TTL_MS + 10;
        for i in 0..MAX_CLIENTS {
            let mut spam = client(&format!("late-{i:03}"));
            spam.created_at = later;
            db.mcp_register_client(spam, later).unwrap();
        }
        for id in [&hermes.client_id, &ci.client_id] {
            assert!(db.mcp_client(id).unwrap().is_some());
        }

        // A pool of approved clients is never displaced: a new label is refused,
        // while an existing label can still rotate its token.
        let db = self::db();
        let kept = db
            .mcp_issue_personal_token(personal("kept", "token-kept", 0))
            .unwrap();
        for i in 1..MAX_CLIENTS {
            let id = format!("live-{i}");
            db.mcp_register_client(client(&id), 0).unwrap();
            let mut granted = grant(&format!("grant-{i}"), &format!("synthetic-{i}"));
            granted.client_id = id;
            granted.expires_at = HOUR;
            db.mcp_mint_grant(granted).unwrap();
        }
        assert!(db
            .mcp_issue_personal_token(personal("new-bot", "token-new", 1))
            .is_err());
        assert_eq!(count(&db, "agent_mcp_personal_clients"), 1);
        let rotated = db
            .mcp_issue_personal_token(personal("kept", "token-kept-2", 1))
            .unwrap();
        assert_eq!(rotated.client_id, kept.client_id);
    }

    #[test]
    fn personal_token_issue_keeps_clients_with_consent_in_flight() {
        let db = db();
        db.mcp_register_client(client("pending"), 0).unwrap();
        // Past the unapproved TTL, a first issue's registration prunes, but not
        // a client whose consent is pending.
        let now = UNAPPROVED_CLIENT_TTL_MS + 1;
        db.mcp_issue_personal_token(NewPersonalToken {
            protected_clients: vec!["pending".into()],
            ..personal("hermes", "token-hermes", now)
        })
        .unwrap();
        assert!(db.mcp_client("pending").unwrap().is_some());
        db.mcp_issue_personal_token(personal("ci-bot", "token-ci", now))
            .unwrap();
        assert!(
            db.mcp_client("pending").unwrap().is_none(),
            "without protection the stale unapproved client is pruned"
        );
    }

    #[test]
    fn personal_token_expiry_is_enforced_and_pruned() {
        let db = db();
        let short = |label: &str, token: &str, now: i64, lifetime: i64| NewPersonalToken {
            expires_at: now + lifetime,
            ..personal(label, token, now)
        };
        for lifetime in [
            i64::MIN,
            -1,
            0,
            PERSONAL_TOKEN_MIN_TTL_MS - 1,
            PERSONAL_TOKEN_MAX_TTL_MS + 1,
        ] {
            assert!(
                db.mcp_issue_personal_token(short("bot", "token-x", 0, lifetime))
                    .is_err(),
                "accepted a lifetime of {lifetime} ms"
            );
        }
        assert_eq!(count(&db, "agent_mcp_clients"), 0);
        db.mcp_issue_personal_token(short("year", "token-year", 0, PERSONAL_TOKEN_MAX_TTL_MS))
            .unwrap();
        let issued = db
            .mcp_issue_personal_token(short("bot", "token-1", 0, PERSONAL_TOKEN_MIN_TTL_MS))
            .unwrap();
        let expiry = PERSONAL_TOKEN_MIN_TTL_MS;
        assert!(by_hash(&db, "token-1", expiry - 1).is_some());
        assert!(by_hash(&db, "token-1", expiry).is_none());
        assert!(db
            .mcp_grant(&issued.grant_id, RESOURCE, expiry)
            .unwrap()
            .is_none());
        assert!(!db
            .mcp_list_grants(expiry)
            .unwrap()
            .iter()
            .any(|g| g.id == issued.grant_id));
        db.mcp_prune_clients(expiry, &[]).unwrap();
        assert!(!db
            .mcp_list_grants(0)
            .unwrap()
            .iter()
            .any(|g| g.id == issued.grant_id));
        // Within retention the label keeps its client.
        let reissued = db
            .mcp_issue_personal_token(personal("bot", "token-2", expiry + 1))
            .unwrap();
        assert_eq!(reissued.client_id, issued.client_id);
        assert!(!reissued.replaced, "nothing live was replaced");
        // Once revoked and past retention the client is pruned and the label freed.
        db.mcp_revoke_client(&issued.client_id).unwrap();
        db.mcp_prune_clients(expiry + 1 + APPROVED_CLIENT_RETENTION_MS, &[])
            .unwrap();
        assert!(db.mcp_client(&issued.client_id).unwrap().is_none());
        let fresh = db
            .mcp_issue_personal_token(personal(
                "bot",
                "token-3",
                expiry + 2 + APPROVED_CLIENT_RETENTION_MS,
            ))
            .unwrap();
        assert_ne!(fresh.client_id, issued.client_id);
    }

    #[test]
    fn issuer_change_revokes_grants_bound_to_other_resources() {
        let db = db();
        for id in ["client", "other", "moved"] {
            db.mcp_register_client(client(id), 0).unwrap();
        }
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        let mut other = grant("other-grant", "access-other");
        other.client_id = "other".into();
        other.expires_at = HOUR;
        db.mcp_mint_grant(other).unwrap();
        let mut moved = grant("moved-grant", "access-moved");
        moved.client_id = "moved".into();
        moved.resource = "http://127.0.0.1:4378/mcp".into();
        moved.expires_at = HOUR;
        db.mcp_mint_grant_with_refresh(
            moved,
            Some(NewMcpRefresh {
                token_hash: credential_hash("refresh-moved"),
                expires_at: 30 * DAY,
            }),
        )
        .unwrap();
        assert_eq!(db.mcp_revoke_grants_not_bound_to(RESOURCE).unwrap(), 1);
        let mut kept: Vec<_> = db
            .mcp_list_grants(1)
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        kept.sort();
        assert_eq!(kept, ["grant", "other-grant"]);
        assert!(db
            .mcp_grant("moved-grant", "http://127.0.0.1:4378/mcp", 1)
            .unwrap()
            .is_none());
        assert_eq!(
            outcome(&rotate_as(
                &db,
                "moved",
                "http://127.0.0.1:4378/mcp",
                "refresh-moved",
                ("access-x", "refresh-x"),
                1
            )),
            "invalid"
        );
        assert_eq!(db.mcp_revoke_grants_not_bound_to(RESOURCE).unwrap(), 0);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1)),
            "rotated"
        );
    }

    #[test]
    fn revoke_all_ends_refresh_families_and_personal_tokens() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        let bot = db
            .mcp_issue_personal_token(personal("hermes", "token-hermes", 0))
            .unwrap();
        assert_eq!(db.mcp_revoke_all().unwrap(), 2);
        assert!(db.mcp_list_grants(1).unwrap().is_empty());
        assert!(by_hash(&db, "access-1", 1).is_none());
        assert!(by_hash(&db, "token-hermes", 1).is_none());
        assert!(db.mcp_grant(&bot.grant_id, RESOURCE, 1).unwrap().is_none());
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1)),
            "invalid"
        );
        assert_eq!(db.mcp_revoke_all().unwrap(), 0);
    }

    #[test]
    fn client_approval_time_is_recorded_only_by_owner_consent() {
        let db = db();
        db.mcp_register_client(client("client"), 0).unwrap();
        assert_eq!(db.mcp_client_approved_at("client").unwrap(), None);
        assert_eq!(db.mcp_client_approved_at("unknown").unwrap(), None);
        sign_in(&db, "grant", "access-1", "refresh-1", 7);
        assert_eq!(db.mcp_client_approved_at("client").unwrap(), Some(7));
        db.mcp_revoke_client("client").unwrap();
        assert_eq!(
            db.mcp_client_approved_at("client").unwrap(),
            Some(7),
            "a revoked client was still approved before"
        );
    }

    #[test]
    fn existing_stores_gain_the_refresh_and_personal_token_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("upgrade.db");
        let conn = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&conn).unwrap();
        conn.execute_batch(
            "DROP TABLE agent_mcp_refresh_tokens; DROP TABLE agent_mcp_personal_clients;",
        )
        .unwrap();
        drop(conn);
        let conn = super::super::open_connection(&path).unwrap();
        super::super::create_schema(&conn).unwrap();
        let db = DatabaseStore {
            conn: Mutex::new(conn),
        };
        db.mcp_register_client(client("client"), 0).unwrap();
        sign_in(&db, "grant", "access-1", "refresh-1", 0);
        assert_eq!(
            outcome(&rotate(&db, "refresh-1", ("access-2", "refresh-2"), 1)),
            "rotated"
        );
        db.mcp_issue_personal_token(personal("hermes", "token-hermes", 1))
            .unwrap();
        assert_eq!(db.mcp_list_grants(2).unwrap().len(), 2);
    }
}
