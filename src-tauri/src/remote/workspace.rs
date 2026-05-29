//! Workspace registry for the headless daemon.
//!
//! Standalone, intentionally not the desktop's much larger
//! `AppStateStore` — that one is wired to Tauri events and is the
//! right shape for a UI, not for a server. Here we just need: a
//! list of workspaces, where their worktrees live, the agent we'd
//! spawn for them, and a nullable `owner_id` for a future cloud
//! relay to populate.
//!
//! Storage: SQLite at `<state_dir>/codemux.db`, schema applied
//! idempotently on first open. The schema is intentionally narrow
//! and decoupled from the desktop's much wider schema — the desktop
//! will *import* workspaces from this registry over the wire when
//! the user pulls, it does not share the table.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// UUID v4. Globally unique across hosts so the desktop can
    /// import without collisions.
    pub id: String,
    /// Human-readable name. Defaults to the basename of `path` when
    /// the caller doesn't supply one.
    pub name: String,
    /// Absolute path to the worktree on this host.
    pub path: String,
    /// Git branch checked out in the worktree. May be `None` for
    /// non-git workspaces.
    pub branch: Option<String>,
    /// Originating project root if this workspace was created from
    /// an existing repo (so we know where it was cloned/worktreed
    /// from). May be `None` for blank workspaces.
    pub project_root: Option<String>,
    /// Stable, deterministic project identity — `UUIDv5(canonical
    /// remote ?? project_root)`. Shared by a project's main checkout
    /// and all its worktrees, and identical on every host/device that
    /// has the same repo, so the desktop can group siblings without
    /// coordination. See `crate::project_identity`.
    #[serde(default)]
    pub project_uid: Option<String>,
    /// Human-readable project name (basename of `project_root`).
    #[serde(default)]
    pub project_name: Option<String>,
    /// `"main"` for the repo root checkout, `"worktree"` for a
    /// per-branch git worktree. Lets the desktop label and group
    /// without inferring from path shape.
    #[serde(default)]
    pub kind: Option<String>,
    /// Canonical git remote (`canonical_remote(...)`) when the repo has
    /// one. Null for local-only repos. Lets other devices converge on
    /// the same `project_uid` for an independently-cloned copy.
    #[serde(default)]
    pub repo_remote: Option<String>,
    /// RFC 3339 UTC timestamp.
    pub created_at: String,
    /// RFC 3339 UTC timestamp.
    pub updated_at: String,
    /// Always `None` in v1 — reserved for a future cloud relay
    /// integration to record the authenticated user that created
    /// the workspace. Nullable column today so populating it later
    /// is not a destructive migration.
    pub owner_id: Option<String>,
    /// Hostname-derived id of the host that created this workspace.
    /// Recorded so an imported workspace can show "from <host>" in
    /// the desktop UI.
    pub origin_host_id: String,
    /// Free-form notes. Reserved for the desktop to attach context
    /// when it pulls — empty in v1.
    pub notes: Option<String>,
}

/// Connection pool of size 1. SQLite serialises writes anyway, and
/// rusqlite's `Connection` is `!Send` if you don't pull in the
/// `serialized` feature; wrapping in a `Mutex` is the cheap, correct
/// path for our concurrency level (low — we're a control plane, not
/// a query engine).
pub struct WorkspaceStore {
    conn: Mutex<Connection>,
    host_id: String,
    workspaces_root: PathBuf,
}

#[derive(Debug)]
pub enum WorkspaceError {
    NotFound(String),
    Db(String),
    Invalid(String),
    Io(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "workspace not found: {s}"),
            Self::Db(s) => write!(f, "database error: {s}"),
            Self::Invalid(s) => write!(f, "invalid input: {s}"),
            Self::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl WorkspaceStore {
    /// Open or create the SQLite database at `db_path` and apply
    /// the schema. `host_id` is recorded on every new workspace as
    /// `origin_host_id`. `workspaces_root` is where blank workspaces
    /// are materialised when a caller doesn't supply a path.
    pub fn open(db_path: &Path, host_id: String, workspaces_root: PathBuf) -> Result<Self, WorkspaceError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| WorkspaceError::Io(format!("create db dir: {e}")))?;
        }
        std::fs::create_dir_all(&workspaces_root)
            .map_err(|e| WorkspaceError::Io(format!("create workspaces root: {e}")))?;

        let conn = Connection::open(db_path).map_err(|e| WorkspaceError::Db(e.to_string()))?;
        // Reasonable defaults for a single-process daemon.
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            ",
        )
        .map_err(|e| WorkspaceError::Db(e.to_string()))?;

        create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            host_id,
            workspaces_root,
        })
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn workspaces_root(&self) -> &Path {
        &self.workspaces_root
    }

    /// Insert a new workspace row. Caller has already decided the
    /// path (either an existing dir they want to register, or one
    /// the caller materialised on disk).
    pub fn create(
        &self,
        name: Option<String>,
        path: String,
        branch: Option<String>,
        project_root: Option<String>,
    ) -> Result<Workspace, WorkspaceError> {
        if path.trim().is_empty() {
            return Err(WorkspaceError::Invalid("path is required".into()));
        }
        let now = chrono::Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let resolved_name = name.unwrap_or_else(|| basename(&path));

        // Stamp a project identity at the moment of record creation,
        // mirroring every desktop create path (`commands/workspace.rs`).
        // The `workspace_create` MCP tool only receives `project_root`
        // for worktrees; a plain root/main checkout an agent builds on
        // this host arrives with `project_root = None`, which used to
        // leave the workspace with a blank project name on every dev
        // device. Deriving it here — the git root of the workspace's
        // own directory, or the directory itself when it isn't (yet) a
        // repo — guarantees a non-null `project_root` at the source, so
        // the cross-device registry never carries an anonymous row.
        // `find_git_root` follows a worktree's `.git` pointer back to
        // the parent repo, so a worktree resolves to its project root
        // (not the per-branch dir), exactly as the desktop does.
        let project_root = project_root.or_else(|| {
            let p = std::path::Path::new(&path);
            let derived = if p.exists() {
                crate::config::workspace_config::find_git_root(p)
            } else {
                None
            };
            Some(
                derived
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| path.clone()),
            )
        });

        // Derive the first-class project identity (see
        // `crate::project_identity`). The canonical remote (when the
        // repo has one) makes the uid converge with copies of the same
        // repo on other hosts/devices; otherwise it falls back to the
        // project-root path. `kind` distinguishes the root checkout
        // from a per-branch worktree.
        let repo_remote = project_root
            .as_deref()
            .and_then(git_remote_origin_url)
            .as_deref()
            .and_then(crate::project_identity::canonical_remote);
        let project_uid = project_root.as_deref().map(|pr| {
            crate::project_identity::project_uid_for(repo_remote.as_deref(), pr)
        });
        let project_name = project_root.as_deref().map(basename);
        let kind = Some(crate::project_identity::derive_kind(std::path::Path::new(&path)).to_string());

        let ws = Workspace {
            id: id.clone(),
            name: resolved_name,
            path,
            branch,
            project_root,
            project_uid,
            project_name,
            kind,
            repo_remote,
            created_at: now.clone(),
            updated_at: now,
            owner_id: None,
            origin_host_id: self.host_id.clone(),
            notes: None,
        };

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO workspaces (
                id, name, path, branch, project_root,
                created_at, updated_at, owner_id, origin_host_id, notes,
                project_uid, project_name, kind, repo_remote
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                ws.id,
                ws.name,
                ws.path,
                ws.branch,
                ws.project_root,
                ws.created_at,
                ws.updated_at,
                ws.owner_id,
                ws.origin_host_id,
                ws.notes,
                ws.project_uid,
                ws.project_name,
                ws.kind,
                ws.repo_remote,
            ],
        )
        .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        Ok(ws)
    }

    pub fn get(&self, id: &str) -> Result<Workspace, WorkspaceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, branch, project_root,
                        created_at, updated_at, owner_id, origin_host_id, notes,
                        project_uid, project_name, kind, repo_remote
                 FROM workspaces WHERE id = ?1",
            )
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        match rows
            .next()
            .map_err(|e| WorkspaceError::Db(e.to_string()))?
        {
            Some(row) => row_to_workspace(row),
            None => Err(WorkspaceError::NotFound(id.to_string())),
        }
    }

    pub fn list(&self) -> Result<Vec<Workspace>, WorkspaceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, branch, project_root,
                        created_at, updated_at, owner_id, origin_host_id, notes,
                        project_uid, project_name, kind, repo_remote
                 FROM workspaces
                 ORDER BY created_at DESC",
            )
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        // rusqlite::query_map wants a closure returning Result<T,
        // rusqlite::Error>, not our WorkspaceError. We map rusqlite
        // errors to a sentinel here and re-translate after; the
        // borrow-checker prevents reaching for our own error directly
        // inside the closure because Row<'_> borrows from rusqlite.
        let rows = stmt
            .query_map([], |row| {
                // SQLite's column reads return rusqlite::Error
                // directly; row_to_workspace returns WorkspaceError.
                // Translate at the boundary.
                row_to_workspace(row).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
                    )
                })
            })
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| WorkspaceError::Db(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn close(&self, id: &str) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute("DELETE FROM workspaces WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        if affected == 0 {
            return Err(WorkspaceError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Update mutable fields on a workspace. Only fields supplied are
    /// changed; `None` means "leave as-is". `updated_at` is touched.
    pub fn update(
        &self,
        id: &str,
        name: Option<String>,
        branch: Option<String>,
        notes: Option<String>,
    ) -> Result<Workspace, WorkspaceError> {
        // Easier than building a dynamic UPDATE: fetch, mutate in
        // memory, write back. Single-process daemon, no concurrent
        // writers worth worrying about.
        let mut ws = self.get(id)?;
        if let Some(n) = name {
            ws.name = n;
        }
        if branch.is_some() {
            ws.branch = branch;
        }
        if notes.is_some() {
            ws.notes = notes;
        }
        ws.updated_at = chrono::Utc::now().to_rfc3339();

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces
             SET name = ?2, branch = ?3, notes = ?4, updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![ws.id, ws.name, ws.branch, ws.notes, ws.updated_at],
        )
        .map_err(|e| WorkspaceError::Db(e.to_string()))?;
        Ok(ws)
    }

    /// Idempotent startup normalization: backfill the project-identity
    /// columns (`project_uid`, `project_name`, `kind`, `repo_remote`)
    /// for rows created before those columns existed. Safe to run on
    /// every daemon boot — it only touches rows whose identity is still
    /// missing, so it does real work exactly once after an upgrade.
    /// Returns the number of rows updated. Mirrors Superset's
    /// `runMainWorkspaceSweep` (the "no anonymous / no
    /// worktrees-without-a-root" guarantee), adapted to our registry.
    pub fn sweep_backfill_identity(&self) -> Result<usize, WorkspaceError> {
        // Snapshot the rows first (this drops the lock), then update.
        let rows = self.list()?;
        let mut updated = 0usize;
        let conn = self.conn.lock().unwrap();
        for ws in rows {
            if ws.project_uid.is_some() && ws.kind.is_some() {
                continue;
            }
            let project_root = ws.project_root.clone().unwrap_or_else(|| ws.path.clone());
            let repo_remote = git_remote_origin_url(&project_root)
                .as_deref()
                .and_then(crate::project_identity::canonical_remote);
            let project_uid = Some(crate::project_identity::project_uid_for(
                repo_remote.as_deref(),
                &project_root,
            ));
            let project_name = Some(basename(&project_root));
            let kind = Some(
                crate::project_identity::derive_kind(std::path::Path::new(&ws.path)).to_string(),
            );
            conn.execute(
                "UPDATE workspaces
                 SET project_uid = ?2, project_name = ?3, kind = ?4, repo_remote = ?5
                 WHERE id = ?1",
                rusqlite::params![ws.id, project_uid, project_name, kind, repo_remote],
            )
            .map_err(|e| WorkspaceError::Db(e.to_string()))?;
            updated += 1;
        }
        Ok(updated)
    }
}

fn create_schema(conn: &Connection) -> Result<(), WorkspaceError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS workspaces (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            path            TEXT NOT NULL,
            branch          TEXT,
            project_root    TEXT,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            owner_id        TEXT,            -- nullable; populated by future cloud relay
            origin_host_id  TEXT NOT NULL,
            notes           TEXT,
            project_uid     TEXT,            -- deterministic project identity (UUIDv5)
            project_name    TEXT,
            kind            TEXT,            -- 'main' | 'worktree'
            repo_remote     TEXT             -- canonical git remote, null for local-only
        );
        CREATE INDEX IF NOT EXISTS idx_workspaces_origin
            ON workspaces (origin_host_id);
        CREATE INDEX IF NOT EXISTS idx_workspaces_owner
            ON workspaces (owner_id);
        ",
    )
    .map_err(|e| WorkspaceError::Db(e.to_string()))?;

    // Additive column migrations for DBs created before the first-class
    // project-identity columns existed. The daemon DB lives on user
    // machines and `CREATE TABLE IF NOT EXISTS` is a no-op on an
    // existing table, so columns added later need explicit, idempotent
    // ALTERs — mirroring the desktop's `database.rs` migration loop. A
    // re-run yields "duplicate column name", which we swallow.
    for stmt in [
        "ALTER TABLE workspaces ADD COLUMN project_uid TEXT",
        "ALTER TABLE workspaces ADD COLUMN project_name TEXT",
        "ALTER TABLE workspaces ADD COLUMN kind TEXT",
        "ALTER TABLE workspaces ADD COLUMN repo_remote TEXT",
    ] {
        if let Err(e) = conn.execute(stmt, []) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(WorkspaceError::Db(format!("migration failed ({stmt}): {msg}")));
            }
        }
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_workspaces_project_uid
            ON workspaces (project_uid);",
    )
    .map_err(|e| WorkspaceError::Db(e.to_string()))?;
    Ok(())
}

fn row_to_workspace(row: &rusqlite::Row<'_>) -> Result<Workspace, WorkspaceError> {
    let map = |i: usize| -> Result<Option<String>, _> { row.get::<_, Option<String>>(i) };
    Ok(Workspace {
        id: row.get::<_, String>(0).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        name: row.get::<_, String>(1).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        path: row.get::<_, String>(2).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        branch: map(3).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        project_root: map(4).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        created_at: row.get::<_, String>(5).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        updated_at: row.get::<_, String>(6).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        owner_id: map(7).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        origin_host_id: row.get::<_, String>(8).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        notes: map(9).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        project_uid: map(10).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        project_name: map(11).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        kind: map(12).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        repo_remote: map(13).map_err(|e| WorkspaceError::Db(e.to_string()))?,
    })
}

/// Best-effort `git remote get-url origin` for a repo dir. Returns
/// `None` on any failure (not a repo, no origin, git missing) — the
/// project still gets a path-based identity.
fn git_remote_origin_url(dir: &str) -> Option<String> {
    // Cheap gate: skip the subprocess entirely when the dir has no
    // `.git` (not-yet-materialised registrations and the many fake
    // test paths). Spawning git just to have it fail is wasted time —
    // and on Windows it's slow enough to blow the CI time budget.
    if !std::path::Path::new(dir).join(".git").exists() {
        return None;
    }
    let out = std::process::Command::new("git")
        .args(["-C", dir, "config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

fn basename(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "workspace".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_store(dir: &TempDir) -> WorkspaceStore {
        let db_path = dir.path().join("codemux.db");
        let ws_root = dir.path().join("workspaces");
        WorkspaceStore::open(&db_path, "test-host".into(), ws_root).unwrap()
    }

    #[test]
    fn create_stamps_project_identity_and_kind() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);

        // A root checkout (real .git dir, no remote → path-based uid).
        let repo = dir.path().join("passpage");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let repo_str = repo.display().to_string();

        let ws = store
            .create(None, repo_str.clone(), Some("main".into()), None)
            .unwrap();

        assert_eq!(ws.kind.as_deref(), Some("main"));
        assert_eq!(ws.project_name.as_deref(), Some("passpage"));
        assert_eq!(ws.repo_remote, None, "no origin remote in this temp repo");
        let expected_uid =
            crate::project_identity::project_uid_for(None, &repo_str);
        assert_eq!(ws.project_uid.as_deref(), Some(expected_uid.as_str()));

        // Round-trips through get/list (column order intact).
        assert_eq!(store.get(&ws.id).unwrap().project_uid, ws.project_uid);
        assert_eq!(store.list().unwrap()[0].kind.as_deref(), Some("main"));
    }

    #[test]
    fn create_marks_worktree_kind_and_shares_uid_with_main() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);

        // Main checkout.
        let repo = dir.path().join("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let main = store
            .create(None, repo.display().to_string(), Some("main".into()), None)
            .unwrap();

        // A worktree: .git is a FILE; project_root passed = the parent
        // repo (as the desktop worktree-create path does).
        let wt = dir.path().join("wt-feature");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /app/.git/worktrees/feature\n").unwrap();
        let worktree = store
            .create(
                Some("feature".into()),
                wt.display().to_string(),
                Some("feature".into()),
                Some(repo.display().to_string()),
            )
            .unwrap();

        assert_eq!(main.kind.as_deref(), Some("main"));
        assert_eq!(worktree.kind.as_deref(), Some("worktree"));
        assert_eq!(
            main.project_uid, worktree.project_uid,
            "main + worktree of the same repo share a project_uid"
        );
    }

    #[test]
    fn sweep_backfills_legacy_rows_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);

        // Simulate a legacy row: insert directly with NULL identity
        // columns (as a pre-migration `create` would have).
        let repo = dir.path().join("legacy");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let repo_str = repo.display().to_string();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, path, branch, project_root,
                    created_at, updated_at, owner_id, origin_host_id, notes)
                 VALUES ('legacy-1', 'legacy', ?1, 'main', ?1,
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL, 'test-host', NULL)",
                rusqlite::params![repo_str],
            )
            .unwrap();
        }

        let before = store.get("legacy-1").unwrap();
        assert_eq!(before.project_uid, None, "legacy row has no identity yet");

        let updated = store.sweep_backfill_identity().unwrap();
        assert_eq!(updated, 1);

        let after = store.get("legacy-1").unwrap();
        assert_eq!(after.kind.as_deref(), Some("main"));
        assert_eq!(after.project_name.as_deref(), Some("legacy"));
        assert!(after.project_uid.is_some());

        // Idempotent: a second sweep touches nothing.
        assert_eq!(store.sweep_backfill_identity().unwrap(), 0);
    }

    #[test]
    fn migration_is_idempotent_across_reopen() {
        // Opening an existing DB again must not fail on the ALTER loop
        // (duplicate column) — the daemon DB upgrade path.
        let dir = TempDir::new().unwrap();
        {
            let _ = open_store(&dir);
        }
        let store = open_store(&dir); // re-open: ALTERs already applied
        let repo = dir.path().join("r");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let ws = store
            .create(None, repo.display().to_string(), None, None)
            .unwrap();
        assert!(ws.project_uid.is_some());
    }

    #[test]
    fn create_list_close_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        assert!(store.list().unwrap().is_empty());

        let ws = store
            .create(
                Some("foo".into()),
                "/tmp/repo".into(),
                Some("feat/x".into()),
                Some("/tmp/repo-origin".into()),
            )
            .unwrap();
        assert_eq!(ws.name, "foo");
        assert_eq!(ws.path, "/tmp/repo");
        assert_eq!(ws.branch.as_deref(), Some("feat/x"));
        assert_eq!(ws.origin_host_id, "test-host");
        assert!(ws.owner_id.is_none(), "owner_id is null in v1");

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, ws.id);

        let got = store.get(&ws.id).unwrap();
        assert_eq!(got.id, ws.id);

        store.close(&ws.id).unwrap();
        assert!(store.list().unwrap().is_empty());

        match store.get(&ws.id) {
            Err(WorkspaceError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn create_defaults_name_from_path_basename() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let ws = store.create(None, "/tmp/my-cool-repo".into(), None, None).unwrap();
        assert_eq!(ws.name, "my-cool-repo");
    }

    #[test]
    fn create_derives_project_root_from_git_root_when_omitted() {
        // A root/main project an agent builds (e.g. `git init`) and then
        // registers via `workspace_create` WITHOUT passing project_root
        // must still carry a project identity. The daemon derives it
        // from the path's git root so the cross-device registry never
        // gets an anonymous row (the blank-project-name bug).
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);

        let repo = dir.path().join("passpage");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let repo_str = repo.display().to_string();

        let ws = store
            .create(None, repo_str.clone(), Some("main".into()), None)
            .unwrap();

        assert_eq!(
            ws.project_root.as_deref(),
            Some(repo_str.as_str()),
            "project_root must be derived from the path's git root"
        );
    }

    #[test]
    fn create_falls_back_to_path_when_no_git_root() {
        // A registered path that isn't (yet) a git repo still gets a
        // non-null project_root: the path itself. Never blank.
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let missing = dir.path().join("not-a-repo");
        let missing_str = missing.display().to_string();

        let ws = store.create(None, missing_str.clone(), None, None).unwrap();

        assert_eq!(ws.project_root.as_deref(), Some(missing_str.as_str()));
    }

    #[test]
    fn create_preserves_explicit_project_root_for_worktrees() {
        // The worktree case: when the caller DOES pass project_root
        // (pointing at the parent repo), it wins — we must not overwrite
        // it with the worktree's own directory.
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let ws = store
            .create(
                Some("ui-polish".into()),
                "/home/agent/.codemux/worktrees/passpage/ui-polish".into(),
                Some("ui-polish-v1".into()),
                Some("/home/agent/projects/passpage".into()),
            )
            .unwrap();
        assert_eq!(
            ws.project_root.as_deref(),
            Some("/home/agent/projects/passpage"),
            "an explicitly-passed project_root (the parent repo) must be preserved"
        );
    }

    #[test]
    fn create_rejects_empty_path() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let err = store.create(None, "  ".into(), None, None).unwrap_err();
        assert!(matches!(err, WorkspaceError::Invalid(_)));
    }

    #[test]
    fn update_changes_name_and_touches_timestamp() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let ws = store.create(None, "/tmp/a".into(), None, None).unwrap();
        let old_updated = ws.updated_at.clone();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let updated = store
            .update(&ws.id, Some("renamed".into()), None, Some("hello".into()))
            .unwrap();
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.notes.as_deref(), Some("hello"));
        assert_ne!(updated.updated_at, old_updated);
    }

    #[test]
    fn close_missing_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        match store.close("does-not-exist") {
            Err(WorkspaceError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_orders_newest_first() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let a = store.create(Some("a".into()), "/a".into(), None, None).unwrap();
        // SQLite RFC3339 strings sort lexicographically same as
        // chronological. Sleep just enough to bump the timestamp.
        std::thread::sleep(std::time::Duration::from_millis(15));
        let b = store.create(Some("b".into()), "/b".into(), None, None).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed[0].id, b.id);
        assert_eq!(listed[1].id, a.id);
    }
}
