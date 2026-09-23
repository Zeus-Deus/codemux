//! Removal is committed before best-effort cleanup. Retrying never re-enables code.
use super::{
    manager::{Installation, Manager},
    ErrorCode, ProtocolError, Result,
};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Files {
    installation: String,
    plugin: String,
    digests: Vec<String>,
    state: bool,
}
fn unavailable() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on cleanup journal is unavailable",
    )
}
pub(super) fn queue(
    tx: &Transaction<'_>,
    installation: &Installation,
    keep_data: bool,
) -> Result<()> {
    // Every attempted persistent write is indexed before touching the OS store,
    // including credentials removed from later manifests. A declaration alone
    // is not evidence of a saved credential: querying an unavailable OS service
    // for unused optional fields creates spurious permanent cleanup warnings.
    tx.execute("INSERT OR IGNORE INTO cleanup(installation,credential) SELECT installation,id FROM credential_entries WHERE installation=?1",[&installation.installation_id]).map_err(|_|unavailable())?;
    let mut digests = vec![installation.digest.clone()];
    if let Some(previous) = &installation.previous {
        if previous.digest != installation.digest {
            digests.push(previous.digest.clone());
        }
    }
    let files = Files {
        installation: installation.installation_id.clone(),
        plugin: installation.manifest.id.clone(),
        digests,
        state: !keep_data,
    };
    tx.execute(
        "INSERT OR REPLACE INTO file_cleanup(installation,record) VALUES(?1,?2)",
        rusqlite::params![
            installation.installation_id,
            serde_json::to_string(&files).unwrap()
        ],
    )
    .map_err(|_| unavailable())?;
    tx.execute(
        "DELETE FROM credential_entries WHERE installation=?1",
        [&installation.installation_id],
    )
    .map_err(|_| unavailable())?;
    // Settings are private data: "Keep data for reinstall" keeps them with
    // the orphaned installation until a matching-source restore claims them.
    if !keep_data {
        tx.execute(
            "DELETE FROM settings WHERE installation=?1",
            [&installation.installation_id],
        )
        .map_err(|_| unavailable())?;
    }
    Ok(())
}
/// Copy retained settings to the installation that confirmed the restore,
/// keeping only values the new manifest still declares and accepts.
pub(super) fn restore_settings(
    tx: &Transaction<'_>,
    retained: &Installation,
    candidate: &Installation,
) -> Result<()> {
    use rusqlite::OptionalExtension;
    let saved: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE installation=?1",
            [&retained.installation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| unavailable())?;
    let Some(saved) = saved.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else {
        return Ok(());
    };
    let values: serde_json::Map<_, _> = candidate
        .manifest
        .settings
        .iter()
        .filter_map(|setting| {
            saved
                .get(setting.id())
                .filter(|value| setting.accepts(value))
                .map(|value| (setting.id().to_owned(), value.clone()))
        })
        .collect();
    tx.execute(
        "INSERT OR REPLACE INTO settings(installation,value) VALUES(?1,?2)",
        rusqlite::params![
            candidate.installation_id,
            serde_json::Value::Object(values).to_string()
        ],
    )
    .map_err(|_| unavailable())?;
    Ok(())
}
impl Manager {
    /// Startup-only sweep, after all journals are recovered and before any
    /// host exists. Never inspect or remove paths outside the private root.
    pub(super) fn prune_unreferenced(&self) -> Result<()> {
        let mut retained = self.list()?;
        {
            let db = self.registry.lock().unwrap();
            let mut query = db
                .prepare("SELECT value FROM metadata WHERE key LIKE 'orphan:%'")
                .map_err(|_| unavailable())?;
            for row in query
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|_| unavailable())?
            {
                let record: Installation = serde_json::from_str(&row.map_err(|_| unavailable())?)
                    .map_err(|_| unavailable())?;
                record.validate_record()?;
                retained.push(record);
            }
        }
        let mut live_state = std::collections::HashSet::new();
        let mut live_packages = std::collections::HashSet::new();
        for record in retained {
            live_state.insert((record.installation_id.clone(), record.data_generation));
            live_packages.insert((record.manifest.id.clone(), record.digest));
            if let Some(previous) = record.previous {
                live_state.insert((record.installation_id, previous.data_generation));
                live_packages.insert((record.manifest.id, previous.digest));
            }
        }
        let uuid = |s: &str| uuid::Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s);
        let mut remove = Vec::new();
        let children = |root: &std::path::Path| -> Result<Vec<std::fs::DirEntry>> {
            match std::fs::read_dir(root) {
                Ok(entries) => entries.map(|e| e.map_err(|_| unavailable())).collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(_) => Err(unavailable()),
            }
        };
        for entry in children(&self.root.join("staging"))? {
            if uuid(&entry.file_name().to_string_lossy()) {
                remove.push(entry.path());
            }
        }
        for entry in children(&self.root.join("state"))? {
            let installation = entry.file_name().to_string_lossy().into_owned();
            if !uuid(&installation) || !entry.file_type().map_err(|_| unavailable())?.is_dir() {
                continue;
            }
            for data in children(&entry.path())? {
                let generation = data.file_name().to_string_lossy().into_owned();
                if uuid(&generation) && !live_state.contains(&(installation.clone(), generation)) {
                    remove.push(data.path());
                }
            }
        }
        for entry in children(&self.root.join("packages"))? {
            let plugin = entry.file_name().to_string_lossy().into_owned();
            if !codemux_addon_protocol::manifest::plugin_id(&plugin)
                || !entry.file_type().map_err(|_| unavailable())?.is_dir()
            {
                continue;
            }
            for package in children(&entry.path())? {
                let digest = package.file_name().to_string_lossy().into_owned();
                if codemux_addon_protocol::catalog::hex(&digest, 64)
                    && !live_packages.contains(&(plugin.clone(), digest))
                {
                    remove.push(package.path());
                }
            }
        }
        let mut failed = false;
        for path in remove {
            // read_dir never follows a directory symlink; remove_dir_all on a
            // symlink is likewise forbidden rather than relying on platform behavior.
            match path.symlink_metadata() {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    failed |= std::fs::remove_dir_all(path).is_err();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => failed = true,
            }
        }
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('gc-warning',?1)",
                [if failed { "true" } else { "false" }],
            )
            .map_err(|_| unavailable())?;
        Ok(())
    }
    pub fn cleanup_warnings(&self) -> Result<Vec<String>> {
        let db = self.registry.lock().unwrap();
        let credentials: bool = db
            .query_row("SELECT EXISTS(SELECT 1 FROM cleanup)", [], |r| r.get(0))
            .map_err(|_| unavailable())?;
        let files: bool = db
            .query_row("SELECT EXISTS(SELECT 1 FROM file_cleanup)", [], |r| {
                r.get(0)
            })
            .map_err(|_| unavailable())?;
        let mut warnings = Vec::new();
        let stale: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='gc-warning' AND value='true')",
                [],
                |r| r.get(0),
            )
            .map_err(|_| unavailable())?;
        if stale {
            warnings.push("Some obsolete add-on files could not be removed. They remain inert; cleanup retries on the next launch.".into());
        }

        if credentials {
            warnings.push("Removed add-on credentials need cleanup. Unlock the OS credential store and retry.".into());
        }
        if files {
            warnings.push("Removed add-on files need cleanup. The package remains inert; retry after storage is available.".into());
        }
        Ok(warnings)
    }
    pub async fn retry_cleanup(&self) -> Result<Vec<String>> {
        let credentials: Vec<(String, String)> = {
            let db = self.registry.lock().unwrap();
            let mut query = db
                .prepare("SELECT installation,credential FROM cleanup LIMIT 1000")
                .map_err(|_| unavailable())?;
            let values = query
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|_| unavailable())?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| unavailable())?;
            values
        };
        for (installation, credential) in credentials {
            if uuid::Uuid::parse_str(&installation).is_err()
                || !codemux_addon_protocol::catalog::hex(&credential, 64)
            {
                return Err(unavailable());
            }
            if self
                .credentials
                .delete(&installation, &credential)
                .await
                .is_ok()
            {
                self.registry
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM cleanup WHERE installation=?1 AND credential=?2",
                        rusqlite::params![installation, credential],
                    )
                    .map_err(|_| unavailable())?;
            }
        }
        let pending: Vec<String> = {
            let db = self.registry.lock().unwrap();
            let mut query = db
                .prepare("SELECT record FROM file_cleanup LIMIT 1000")
                .map_err(|_| unavailable())?;
            let values = query
                .query_map([], |r| r.get(0))
                .map_err(|_| unavailable())?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| unavailable())?;
            values
        };
        for value in pending {
            let files: Files = serde_json::from_str(&value).map_err(|_| unavailable())?;
            if !uuid::Uuid::parse_str(&files.installation)
                .is_ok_and(|u| u.to_string() == files.installation)
                || !codemux_addon_protocol::manifest::plugin_id(&files.plugin)
                || files.digests.len() > 2
                || files
                    .digests
                    .iter()
                    .any(|d| !codemux_addon_protocol::catalog::hex(d, 64))
            {
                return Err(unavailable());
            }
            let operation = self.operation(&files.plugin).await;
            let _lock = operation.lock().await;
            let installed = self.list()?;
            if !installed
                .iter()
                .any(|i| i.installation_id == files.installation)
            {
                // Session-only values never touched the OS credential store.
                // Remove them even when the user retained private plugin data.
                self.credentials.clear_session(&files.installation).await;
            }
            let mut paths = Vec::new();
            for digest in files.digests {
                if !installed.iter().any(|i| {
                    i.manifest.id == files.plugin
                        && (i.digest == digest
                            || i.previous.as_ref().is_some_and(|p| p.digest == digest))
                }) {
                    paths.push(self.root.join("packages").join(&files.plugin).join(digest));
                }
            }
            if files.state
                && !installed
                    .iter()
                    .any(|i| i.installation_id == files.installation)
            {
                paths.push(self.root.join("state").join(&files.installation));
            }
            let mut success = true;
            for path in paths {
                match std::fs::symlink_metadata(&path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                        if std::fs::remove_dir_all(path).is_err() {
                            success = false
                        }
                    }
                    _ => success = false,
                }
            }
            if success {
                self.registry
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM file_cleanup WHERE installation=?1",
                        [files.installation],
                    )
                    .map_err(|_| unavailable())?;
            }
        }
        self.cleanup_warnings()
    }
}
