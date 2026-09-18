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
}
