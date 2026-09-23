use super::{ErrorCode, ProtocolError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::Path;
fn unavailable(_: rusqlite::Error) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Plugin storage is unavailable",
    )
}
pub struct Storage {
    connection: Connection,
}
impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).map_err(unavailable)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS kv(scope TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(scope,key));").map_err(unavailable)?;
        // Earlier builds keyed project data by a reusable AppState workspace
        // ID. Those rows cannot be attributed to a project safely, so they are
        // dropped rather than migrated; scopes now use workspace::storage_scope.
        connection
            .execute("DELETE FROM kv WHERE scope LIKE 'workspace:%'", [])
            .map_err(unavailable)?;
        Ok(Self { connection })
    }
    fn key(key: &str) -> Result<()> {
        if key.is_empty()
            || key.len() > 128
            || !key.is_ascii()
            || key.bytes().any(|b| b.is_ascii_control())
        {
            Err(ProtocolError::invalid("Invalid storage key"))
        } else {
            Ok(())
        }
    }
    pub fn get(&self, scope: &str, key: &str) -> Result<Value> {
        Self::key(key)?;
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM kv WHERE scope=?1 AND key=?2",
                params![scope, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(unavailable)?;
        value
            .map(|v| {
                serde_json::from_str(&v).map_err(|_| {
                    ProtocolError::new(ErrorCode::StorageUnavailable, "Invalid stored JSON")
                })
            })
            .unwrap_or(Ok(Value::Null))
    }
    pub fn set(&mut self, scope: &str, key: &str, value: &Value) -> Result<()> {
        Self::key(key)?;
        let bytes = serde_json::to_string(value)
            .map_err(|_| ProtocolError::invalid("Invalid storage value"))?;
        if bytes.len() > 65536 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Storage value exceeds 64 KiB",
            ));
        }
        let transaction = self.connection.transaction().map_err(unavailable)?;
        transaction.execute("INSERT INTO kv(scope,key,value) VALUES(?1,?2,?3) ON CONFLICT(scope,key) DO UPDATE SET value=excluded.value",params![scope,key,bytes]).map_err(unavailable)?;
        let size:i64=transaction.query_row("SELECT COALESCE(SUM(length(CAST(scope AS BLOB))+length(CAST(key AS BLOB))+length(CAST(value AS BLOB))),0) FROM kv",[],|r|r.get(0)).map_err(unavailable)?;
        if size > 5 * 1024 * 1024 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Plugin storage exceeds 5 MiB",
            ));
        }
        transaction.commit().map_err(unavailable)
    }
    pub fn delete(&mut self, scope: &str, key: &str) -> Result<()> {
        Self::key(key)?;
        let transaction = self.connection.transaction().map_err(unavailable)?;
        transaction
            .execute(
                "DELETE FROM kv WHERE scope=?1 AND key=?2",
                params![scope, key],
            )
            .map_err(unavailable)?;
        transaction.commit().map_err(unavailable)
    }
    pub fn snapshot(&self, path: &Path) -> Result<()> {
        self.connection
            .backup(rusqlite::DatabaseName::Main, path, None)
            .map_err(unavailable)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_scope_persists_and_rejects_oversize_without_changing_value() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let mut store = Storage::open(&path).unwrap();
        store
            .set("global", "key", &serde_json::json!("safe"))
            .unwrap();
        assert!(store
            .set("global", "key", &serde_json::json!("x".repeat(65536)))
            .is_err());
        assert_eq!(store.get("global", "key").unwrap(), "safe");
        assert_eq!(store.get("workspace:another", "key").unwrap(), Value::Null);
        drop(store);
        let store = Storage::open(&path).unwrap();
        assert_eq!(store.get("global", "key").unwrap(), "safe");
    }
    #[test]
    fn keys_and_total_size_are_bounded_without_losing_committed_values() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Storage::open(&root.path().join("state.sqlite")).unwrap();
        let long = "k".repeat(129);
        for key in ["", long.as_str(), "clé", "line\nbreak"] {
            assert_eq!(
                store
                    .set("global", key, &serde_json::json!(1))
                    .unwrap_err()
                    .data
                    .code,
                ErrorCode::InvalidMessage,
                "{key:?}"
            );
        }
        store
            .set("global", &"k".repeat(128), &serde_json::json!(1))
            .unwrap();
        let value = serde_json::json!("x".repeat(60 * 1024));
        let mut stored = 0;
        let error = loop {
            match store.set("global", &format!("fill-{stored}"), &value) {
                Ok(()) => stored += 1,
                Err(error) => break error,
            }
        };
        assert_eq!(error.data.code, ErrorCode::ResourceLimit);
        assert!((80..=90).contains(&stored), "{stored} values stored");
        assert_eq!(
            store.get("global", &format!("fill-{stored}")).unwrap(),
            Value::Null
        );
        assert_eq!(store.get("global", "fill-0").unwrap(), value);
        store
            .set("global", "fill-0", &serde_json::json!("smaller"))
            .unwrap();
    }
    #[test]
    fn project_data_keyed_by_a_reusable_workspace_id_is_dropped_on_open() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let mut store = Storage::open(&path).unwrap();
        for scope in ["workspace:workspace-1", "workspace-root:current", "global"] {
            store.set(scope, "key", &serde_json::json!(scope)).unwrap();
        }
        drop(store);
        let store = Storage::open(&path).unwrap();
        assert_eq!(
            store.get("workspace:workspace-1", "key").unwrap(),
            Value::Null
        );
        assert_eq!(
            store.get("workspace-root:current", "key").unwrap(),
            "workspace-root:current"
        );
        assert_eq!(store.get("global", "key").unwrap(), "global");
    }
}
