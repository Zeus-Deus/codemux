//! OpenCode conversation sync across push / pull (issue #16).
//!
//! The cloud-push series already syncs **Claude Code** conversations: push
//! rsyncs the laptop's per-project JSONLs to the remote and the remote
//! relaunch uses `claude --resume <uuid>`; pull-back reverses it (see
//! `commands::hosts::sync_claude_projects` / `pull_claude_projects`). This
//! module does the equivalent for **OpenCode** so a pushed OpenCode pane
//! continues the laptop's conversation on the host (and vice-versa on
//! pull-back).
//!
//! ## Why not raw SQLite extract+merge (the original issue plan)
//!
//! OpenCode stores every conversation in one SQLite DB
//! (`~/.local/share/opencode/opencode.db`), so we can't rsync the file —
//! that would clobber the remote's other history. The issue proposed
//! hand-extracting this workspace's rows via `rusqlite` and re-INSERTing
//! them on the remote. That schema, however, is large, undocumented, and
//! moves fast (1.16.0 alone added `session_message`, `event` /
//! `event_sequence` event-sourcing, `workspace`, `session_input`, `todo`,
//! `project_directory`, … on top of the `session` / `message` / `part`
//! tables the issue named). Re-implementing a transitive row extractor
//! against that surface — and keeping it correct as OpenCode evolves, and
//! across laptop/host version skew — is fragile.
//!
//! OpenCode ships **official, version-stable** commands that do exactly
//! this, and we use them instead:
//!
//! - `opencode export <sessionID>` → a portable JSON bundle
//!   (`{info:{id,directory,…}, messages:[{info,parts}]}`). Read-only.
//! - `opencode import <file>` → inserts/updates that one session. It
//!   **preserves the session id**, is **idempotent** on re-import, and
//!   importing a longer (continued) bundle over a shorter one merges in
//!   the new messages — exactly the monotonic-continuation property the
//!   acceptance criteria need.
//! - `opencode db "<sql>"` → headless query, used to find which session to
//!   export.
//!
//! Crucially `import` sets the session's `directory` to the **current
//! working directory** it runs in (it ignores `info.directory` in the
//! bundle). So we run `import` *from the workspace directory* on the
//! receiving side, which associates the session with that cwd — and, just
//! as importantly, **touches only the one imported session id**, never the
//! remote's other OpenCode sessions. That satisfies issue #16's "do NOT
//! clobber unrelated sessions" criterion by construction.
//!
//! ## PATH on the remote
//!
//! `opencode` usually lives under the user's `~/.local/bin` or
//! `~/.opencode/bin`, which a non-interactive SSH command does NOT have on
//! PATH (only login shells source `~/.profile` / `~/.bashrc`). So every
//! remote `opencode` invocation runs through a **login shell fed over
//! stdin** (`ssh host bash -ls`), which both finds the binary and sidesteps
//! nested-quoting hazards. Plain `cat` up/downloads (always on PATH) move
//! the JSON bundle.
//!
//! Everything here is best-effort: a failure only loses conversation
//! continuity (the agent still launches), so callers warn-and-continue,
//! mirroring the Claude sync.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Generous per-step ceiling. `opencode export`/`import` spin up the full
/// OpenCode runtime and a large active session can be tens of MB; 3 minutes
/// covers any realistic transcript without letting a hung host stall us
/// forever.
const SYNC_TIMEOUT: Duration = Duration::from_secs(180);

// ── Pure helpers (unit-tested; no process / network) ──────────────────────

/// Escape a string for embedding inside a single-quoted SQL string literal
/// (`'…'`): SQLite doubles an embedded single quote.
pub(crate) fn sql_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// POSIX single-quote a value for safe embedding in a shell script body.
pub(crate) fn shell_sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// SQL that picks the newest **resumable** (root, `parent_id IS NULL`)
/// session for a directory. Child / sub-agent sessions share a directory
/// with their parent but aren't what `--continue` / the TUI resumes, so we
/// exclude them and target the conversation root.
pub(crate) fn newest_session_sql(dir: &str) -> String {
    format!(
        "SELECT id FROM session WHERE directory = '{}' AND parent_id IS NULL \
         ORDER BY time_updated DESC LIMIT 1",
        sql_quote(dir)
    )
}

/// Pull the first `ses_…` token out of `opencode db` TSV output (which
/// carries a header line and may be prefixed by login-shell noise).
pub(crate) fn parse_session_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("ses_"))
        .map(str::to_string)
}

/// Pull `ses_…` out of a `CMX_OC_SID=<id>` marker line emitted by the
/// remote extract script (tolerates surrounding login-shell noise).
pub(crate) fn parse_sid_marker(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix("CMX_OC_SID="))
        .find(|s| s.starts_with("ses_"))
        .map(str::to_string)
}

/// Owner-only folder on the remote host where bundles wait between the SSH
/// steps of one transfer. Bundles are whole transcripts, so they never go in
/// the shared `/tmp`, where other users could read them or plant the name.
const REMOTE_BUNDLE_DIR: &str = "~/.codemux/opencode-sync";

/// Quote a remote bundle path, keeping its leading `~/` expandable.
fn remote_quote(path: &str) -> String {
    super::push::shell_escape(path)
}

/// The same ancestor check is run on the remote before creation, reads, and
/// deletion. Cleanup never creates a directory if the path has changed.
fn remote_bundle_dir_guard(create: bool) -> String {
    crate::private_dir::shell_private_dir_guard(&[".codemux", "opencode-sync"], create)
}

/// A shell command that checks the remote directory, then performs a single
/// operation on one bundle. The filename is restricted to literal shell-safe
/// characters before it is put into the script.
fn remote_bundle_command(
    remote_path: &str,
    create: bool,
    operation: &str,
) -> Result<String, String> {
    let name = remote_bundle_name(remote_path)
        .ok_or_else(|| format!("not an OpenCode sync bundle: {remote_path}"))?;
    let script = format!(
        "{}cmx_file=\"$cmx_dir/{name}\"\n{operation}\n",
        remote_bundle_dir_guard(create),
    );
    Ok(format!("sh -c {}", shell_sq(&script)))
}

fn remote_bundle_cleanup_command(remote_path: &str) -> Result<String, String> {
    remote_bundle_command(remote_path, false, "rm -f \"$cmx_file\"")
}

/// Login-shell script (fed to `bash -ls` over stdin) that imports a
/// transferred bundle so the session associates with `remote_cwd`, then
/// removes the bundle. Exits with `opencode import`'s status so the caller
/// can tell success from failure.
pub(crate) fn remote_import_script(remote_cwd: &str, remote_bundle: &str) -> String {
    // The exit trap checks the directory again before removing the bundle,
    // including after the early `exit 3`. `mkdir -p` defends against the rare case where the
    // workspace dir isn't there yet (the push rsync normally creates it
    // first); `cd` is load-bearing because `opencode import` derives the
    // session directory from the process cwd.
    let cleanup =
        remote_bundle_cleanup_command(remote_bundle).expect("bundle path is generated locally");
    let check = remote_bundle_command(
        remote_bundle,
        false,
        "[ -f \"$cmx_file\" ] && [ ! -L \"$cmx_file\" ]",
    )
    .expect("bundle path is generated locally");
    format!(
        "codemux_cleanup() {{ {cleanup} || :; }}\n\
         trap codemux_cleanup EXIT\n\
         mkdir -p {cwd} && cd {cwd} || exit 3\n\
         {check} || exit 4\n\
         opencode import {bundle} 2>&1\n\
         rc=$?\n\
         exit $rc\n",
        cwd = shell_sq(remote_cwd),
        bundle = remote_quote(remote_bundle),
    )
}

/// Login-shell script that finds the newest root session for `remote_cwd`
/// and exports it to `remote_bundle`, echoing `CMX_OC_SID=<id>` so the
/// caller knows the id and that a bundle was written. The absence of the
/// marker means "nothing to pull" (no session or a failed export); an unsafe
/// bundle directory exits unsuccessfully so the caller can report it.
pub(crate) fn remote_extract_script(remote_cwd: &str, remote_bundle: &str) -> String {
    // Build the query with the shared SQL helper (which SQL-escapes the
    // directory) and pass it as a single shell-quoted argument. Splicing the
    // path through a `$dir` shell variable into the SQL literal would break
    // the query for any path containing an apostrophe (e.g. /Users/O'Brien/…),
    // silently yielding "nothing to pull".
    // A failed export removes its partial bundle; a lost SSH connection is
    // handled by the caller's bounded cleanup attempt.
    let name = remote_bundle_name(remote_bundle).expect("bundle path is generated locally");
    let prep = remote_bundle_dir_guard(true);
    let cleanup =
        remote_bundle_cleanup_command(remote_bundle).expect("bundle path is generated locally");
    format!(
        "sid=$(opencode db {sql} 2>/dev/null | grep -m1 '^ses_')\n\
         if [ -n \"$sid\" ]; then\n\
           {prep}cmx_file=\"$cmx_dir/{name}\"\n\
           set -C\n\
           if opencode export \"$sid\" > \"$cmx_file\" 2>/dev/null; then echo \"CMX_OC_SID=$sid\"; else {cleanup} || :; fi\n\
         fi\n",
        sql = shell_sq(&newest_session_sql(remote_cwd)),
    )
}

/// Unique path in [`REMOTE_BUNDLE_DIR`] for one transfer. The nanosecond
/// clock + pid is plenty unique for a transient file we delete immediately.
fn unique_remote_bundle() -> String {
    format!(
        "{REMOTE_BUNDLE_DIR}/codemux-opencode-{}.json",
        unique_token()
    )
}

/// A new owner-only local bundle with a random name, removed when dropped so
/// every early return cleans it up.
fn local_bundle() -> Result<tempfile::NamedTempFile, String> {
    tempfile::Builder::new()
        .prefix("codemux-opencode-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| format!("create local bundle: {e}"))
}

fn unique_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Atomic counter guarantees distinctness even when two calls land in the
    // same nanosecond tick.
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), nanos, seq)
}

/// Resolve the local `opencode` binary the same way the chat provider does
/// (`which`). `None` ⇒ OpenCode isn't installed locally, so there's no
/// conversation to sync — callers treat it as a benign skip.
fn local_opencode_bin() -> Option<PathBuf> {
    which::which("opencode").ok()
}

// ── Push direction (laptop → remote) ───────────────────────────────────────

/// Export the laptop's active OpenCode session for `local_cwd` and import it
/// into the remote's DB associated with `remote_cwd`, so a relaunch on the
/// host (`opencode --session <id>` / `--continue`) continues the laptop's
/// conversation.
///
/// Returns `Ok(Some(session_id))` when a session was synced (the id is
/// stashed in the pane's adapter captures so the relaunch can target it),
/// `Ok(None)` when there was nothing to sync. `Err` only on a real
/// export/transfer/import failure — best-effort, so the caller logs and
/// continues.
pub async fn sync_opencode_session(
    ssh_target: &str,
    local_cwd: &Path,
    remote_cwd: &Path,
) -> Result<Option<String>, String> {
    let Some(bin) = local_opencode_bin() else {
        eprintln!("[hosts] opencode not found locally; skipping OpenCode session sync");
        return Ok(None);
    };
    let dir = local_cwd.to_string_lossy().to_string();

    let session_id = match local_newest_session_id(&bin, &dir).await? {
        Some(id) => id,
        None => {
            eprintln!("[hosts] no local OpenCode session for {dir}; skipping OpenCode sync");
            return Ok(None);
        }
    };

    let local_bundle = local_bundle()?;
    export_session_local(&bin, &session_id, &local_bundle).await?;

    let remote_bundle = unique_remote_bundle();
    let upload = upload_file(ssh_target, local_bundle.path(), &remote_bundle).await;
    drop(local_bundle);
    if upload.is_err() {
        remove_remote_bundle(ssh_target, &remote_bundle).await;
    }
    upload?;

    let script = remote_import_script(&remote_cwd.to_string_lossy(), &remote_bundle);
    let import = match run_remote_login_script(ssh_target, &script).await {
        Ok((true, _)) => Ok(()),
        Ok((false, output)) => Err(format!(
            "remote `opencode import` failed: {}",
            output.trim()
        )),
        Err(error) => Err(error),
    };
    if let Err(error) = import {
        // A non-zero exit also covers ssh itself failing (say the connection
        // dropped) before the script's exit trap ran, so try once more and
        // report the original failure.
        remove_remote_bundle(ssh_target, &remote_bundle).await;
        return Err(error);
    }
    eprintln!(
        "[hosts] synced OpenCode session {session_id} → {ssh_target}:{}",
        remote_cwd.display()
    );
    Ok(Some(session_id))
}

// ── Pull direction (remote → laptop) ───────────────────────────────────────

/// Symmetric to `sync_opencode_session`: export the remote's active OpenCode
/// session for `remote_cwd` and import it back into the laptop's DB
/// associated with `local_cwd`, so any conversation that continued on the
/// host comes home with the workspace.
///
/// Like the Claude pull-back this is scoped to exactly one session id and
/// merges (never deletes), so local-only history survives. Returns the
/// resumed session id (when present) for the relaunch.
pub async fn pull_opencode_session(
    ssh_target: &str,
    remote_cwd: &Path,
    local_cwd: &Path,
) -> Result<Option<String>, String> {
    let Some(bin) = local_opencode_bin() else {
        eprintln!("[hosts] opencode not found locally; skipping OpenCode pull-back");
        return Ok(None);
    };

    // Created before the remote export so a local failure cannot strand the
    // exported transcript on the host.
    let local_bundle = local_bundle()?;
    let remote_bundle = unique_remote_bundle();
    let script = remote_extract_script(&remote_cwd.to_string_lossy(), &remote_bundle);
    let output = match run_remote_login_script(ssh_target, &script).await {
        Ok((true, output)) => output,
        Ok((false, output)) => {
            remove_remote_bundle(ssh_target, &remote_bundle).await;
            return Err(format!(
                "remote `opencode export` failed: {}",
                output.trim()
            ));
        }
        Err(error) => {
            remove_remote_bundle(ssh_target, &remote_bundle).await;
            return Err(error);
        }
    };
    let Some(session_id) = parse_sid_marker(&output) else {
        eprintln!(
            "[hosts] no remote OpenCode session at {}; skipping OpenCode pull-back",
            remote_cwd.display()
        );
        return Ok(None);
    };

    let download = download_file(ssh_target, &remote_bundle, local_bundle.as_file()).await;
    remove_remote_bundle(ssh_target, &remote_bundle).await;
    download?;

    import_session_local(&bin, local_cwd, local_bundle.path()).await?;

    eprintln!(
        "[hosts] pulled OpenCode session {session_id} back from {ssh_target} → {}",
        local_cwd.display()
    );
    Ok(Some(session_id))
}

// ── Local opencode invocations ─────────────────────────────────────────────

async fn local_newest_session_id(bin: &Path, dir: &str) -> Result<Option<String>, String> {
    let out = Command::new(bin)
        .arg("db")
        .arg(newest_session_sql(dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("spawn `opencode db`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`opencode db` query failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_session_id(&String::from_utf8_lossy(&out.stdout)))
}

async fn export_session_local(
    bin: &Path,
    session_id: &str,
    bundle: &tempfile::NamedTempFile,
) -> Result<(), String> {
    let out_path = bundle.path();
    let file = bundle
        .reopen()
        .map_err(|e| format!("open export bundle {}: {e}", out_path.display()))?;
    let status = Command::new(bin)
        .arg("export")
        .arg(session_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("spawn `opencode export`: {e}"))?;
    if !status.success() {
        return Err(format!("`opencode export {session_id}` failed ({status})"));
    }
    // A zero-byte bundle means export produced nothing — treat as failure so
    // we never upload an empty file the remote import would choke on.
    match std::fs::metadata(out_path) {
        Ok(m) if m.len() > 0 => Ok(()),
        Ok(_) => Err("`opencode export` produced an empty bundle".to_string()),
        Err(e) => Err(format!("stat export bundle: {e}")),
    }
}

async fn import_session_local(bin: &Path, cwd: &Path, bundle: &Path) -> Result<(), String> {
    // `import` derives the session's directory from the process cwd, so run
    // it from the workspace dir to associate the conversation locally.
    let out = Command::new(bin)
        .current_dir(cwd)
        .arg("import")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("spawn `opencode import`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`opencode import` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ── SSH transport helpers ──────────────────────────────────────────────────

/// Upload a local file to `remote_path` via a `cat >` pipeline (same shape
/// as the binary-bootstrap upload; `cat` is always on PATH and the remote
/// shell expands the path's `~/`).
async fn upload_file(ssh_target: &str, local: &Path, remote_path: &str) -> Result<(), String> {
    let upload = remote_bundle_command(remote_path, true, "set -C\ncat > \"$cmx_file\"")?;
    let data = tokio::fs::read(local)
        .await
        .map_err(|e| format!("read local bundle {}: {e}", local.display()))?;
    let mut child = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(upload)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("ssh upload spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&data)
            .await
            .map_err(|e| format!("stream bundle to remote: {e}"))?;
    }
    let out = timeout(SYNC_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "ssh upload timed out".to_string())?
        .map_err(|e| format!("ssh upload failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ssh upload exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Download `remote_path` into `local` via `cat`.
async fn download_file(
    ssh_target: &str,
    remote_path: &str,
    local: &std::fs::File,
) -> Result<(), String> {
    let download = remote_bundle_command(
        remote_path,
        false,
        "[ -f \"$cmx_file\" ] && [ ! -L \"$cmx_file\" ] && cat \"$cmx_file\"",
    )?;
    let out = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(download)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("ssh download spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ssh download exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    std::io::Write::write_all(&mut &*local, &out.stdout)
        .map_err(|e| format!("write local bundle: {e}"))?;
    Ok(())
}

/// Ceiling for one cleanup attempt. It runs after a failure, often on a
/// connection that just dropped, so it must never stall the caller for long:
/// the SSH connect itself already gets `ConnectTimeout=10`.
const REMOTE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether `path` names one bundle file directly inside [`REMOTE_BUNDLE_DIR`],
/// the only remote files cleanup may delete.
fn remote_bundle_name(path: &str) -> Option<&str> {
    path.strip_prefix(REMOTE_BUNDLE_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|name| {
            !name.is_empty()
                && !name.contains("..")
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

/// Best-effort removal of a remote bundle, bounded by
/// [`REMOTE_CLEANUP_TIMEOUT`]. Refuses any path outside the bundle folder.
async fn remove_remote_bundle(ssh_target: &str, remote_path: &str) {
    let cleanup = match remote_bundle_cleanup_command(remote_path) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("[hosts] refusing to remove {remote_path}: {error}");
            return;
        }
    };
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(cleanup);
    if let Err(error) = run_bounded(cmd, REMOTE_CLEANUP_TIMEOUT).await {
        eprintln!("[hosts] could not remove remote bundle {remote_path}: {error}");
    }
}

/// Run `cmd` to completion within `limit`, killing it if it runs over.
async fn run_bounded(mut cmd: Command, limit: Duration) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = timeout(limit, cmd.status())
        .await
        .map_err(|_| format!("timed out after {}s", limit.as_secs_f32()))?
        .map_err(|e| format!("spawn: {e}"))?;
    if !status.success() {
        return Err(format!("exited {status}"));
    }
    Ok(())
}

/// Run a script on the remote via a **login** shell (`bash -ls`) so the
/// user's PATH (where `opencode` lives) is in scope, feeding the script over
/// stdin to avoid nested-quoting hazards. Returns `(exit_ok, stdout)`.
async fn run_remote_login_script(ssh_target: &str, script: &str) -> Result<(bool, String), String> {
    let mut child = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg("bash -ls")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("ssh login-script spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| format!("stream script to remote shell: {e}"))?;
        // drop closes stdin → the remote `bash -ls` sees EOF and runs.
    }
    let out = timeout(SYNC_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "ssh login-script timed out".to_string())?
        .map_err(|e| format!("ssh login-script failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        eprintln!(
            "[hosts] remote opencode script exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok((out.status.success(), stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_quote_doubles_single_quotes() {
        assert_eq!(sql_quote("/home/me/proj"), "/home/me/proj");
        assert_eq!(sql_quote("a'b"), "a''b");
    }

    #[test]
    fn newest_session_sql_targets_root_session_for_dir() {
        let sql = newest_session_sql("/home/me/.codemux/worktrees/api-1234/main");
        assert!(sql.contains("directory = '/home/me/.codemux/worktrees/api-1234/main'"));
        // Root sessions only — children share a directory but aren't resumed.
        assert!(sql.contains("parent_id IS NULL"));
        // Newest first, one row.
        assert!(sql.contains("ORDER BY time_updated DESC"));
        assert!(sql.contains("LIMIT 1"));
    }

    #[test]
    fn parse_session_id_skips_header_and_noise() {
        // `opencode db` emits a TSV header then rows; login shells may add noise.
        let out = "Welcome to host\nid\nses_337aedd99ffevumcj5CPWLphDl\n";
        assert_eq!(
            parse_session_id(out).as_deref(),
            Some("ses_337aedd99ffevumcj5CPWLphDl")
        );
    }

    #[test]
    fn parse_session_id_none_when_no_row() {
        assert_eq!(parse_session_id("id\n\n"), None);
        assert_eq!(parse_session_id(""), None);
    }

    #[test]
    fn parse_sid_marker_extracts_id() {
        let out = "motd line\nImporting...\nCMX_OC_SID=ses_abc123\n";
        assert_eq!(parse_sid_marker(out).as_deref(), Some("ses_abc123"));
    }

    #[test]
    fn parse_sid_marker_none_when_absent() {
        assert_eq!(parse_sid_marker("nothing here\n"), None);
        // A malformed marker (no ses_ prefix) is ignored.
        assert_eq!(parse_sid_marker("CMX_OC_SID=garbage\n"), None);
    }

    #[test]
    fn shell_sq_quotes_and_escapes() {
        assert_eq!(shell_sq("/tmp/x.json"), "'/tmp/x.json'");
        assert_eq!(shell_sq("a'b"), "'a'\\''b'");
    }

    #[test]
    fn remote_import_script_cds_then_imports_then_cleans_up() {
        let s = remote_import_script("/home/me/ws", "~/.codemux/opencode-sync/b1.json");
        assert!(s.contains("mkdir -p '/home/me/ws' && cd '/home/me/ws' || exit 3"));
        // `~/` stays unquoted so the remote shell expands it.
        assert!(s.contains("opencode import ~/'.codemux/opencode-sync/b1.json'"));
        assert!(s.contains("rm -f \"$cmx_file\""));
        assert!(s.contains("cmx_check_ancestor"));
        // Cleanup is installed before anything can exit early.
        assert!(s.starts_with("codemux_cleanup() {"), "{s}");
        assert!(s.find("trap codemux_cleanup EXIT").unwrap() < s.find("|| exit 3").unwrap());
        // Propagates import's exit status so callers can detect failure.
        assert!(s.contains("exit $rc"));
    }

    #[test]
    fn remote_import_script_removes_the_bundle_when_it_exits_early() {
        let home = tempfile::tempdir().unwrap();
        let bundle_dir = home.path().join(".codemux/opencode-sync");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        std::fs::write(bundle_dir.join("b3.json"), "transcript").unwrap();
        // A workspace path under a regular file can't be created, so `cd`
        // fails and the script takes its `exit 3` path.
        std::fs::write(home.path().join("file"), "").unwrap();
        let script = remote_import_script(
            &home.path().join("file/ws").to_string_lossy(),
            "~/.codemux/opencode-sync/b3.json",
        );
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path().canonicalize().unwrap())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(
            !bundle_dir.join("b3.json").exists(),
            "the transcript was left behind"
        );
    }

    #[test]
    fn remote_extract_script_queries_then_exports_with_marker() {
        let s = remote_extract_script("/home/me/ws", "~/.codemux/opencode-sync/b2.json");
        // The query is built by the shared SQL helper and passed as one
        // shell-quoted argument (no raw $dir splice into the SQL literal).
        assert!(s.contains(&shell_sq(&newest_session_sql("/home/me/ws"))));
        assert!(s.contains("opencode db"));
        assert!(s.contains("parent_id IS NULL"));
        assert!(s.contains("opencode export \"$sid\" > \"$cmx_file\""));
        assert!(s.contains("echo \"CMX_OC_SID=$sid\""));
        // A failed export leaves no partial transcript behind.
        assert!(s.contains("rm -f \"$cmx_file\""));
    }

    #[test]
    fn remote_extract_removes_a_failed_exports_partial_bundle() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let opencode = bin.join("opencode");
        std::fs::write(
            &opencode,
            "#!/bin/sh\nif [ \"$1\" = db ]; then echo ses_test; else printf partial; exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&opencode, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bundle = unique_remote_bundle();
        let script = remote_extract_script("/home/me/ws", &bundle);
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path().canonicalize().unwrap())
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CMX_OC_SID="));
        assert!(!home
            .path()
            .join(bundle.strip_prefix("~/").unwrap())
            .exists());
    }

    #[test]
    fn remote_bundles_are_private_and_out_of_shared_tmp() {
        let bundle = unique_remote_bundle();
        assert!(bundle.starts_with("~/.codemux/opencode-sync/codemux-opencode-"));
        // The export checks ancestors before creating the private folder.
        let s = remote_extract_script("/home/me/ws", &bundle);
        assert!(s.contains("cmx_check_ancestor"), "{s}");
        assert!(s.find("cmx_check_ancestor").unwrap() < s.find("mkdir -m 700").unwrap());
        assert!(s.contains("set -C"));
        assert!(!s.contains("/tmp"), "{s}");
    }

    #[test]
    fn remote_bundle_dir_prep_runs_in_a_shell() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join(".cache")).unwrap();
        let bundle = unique_remote_bundle();
        let script =
            remote_bundle_command(&bundle, true, "set -C\nprintf data > \"$cmx_file\"").unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path().canonicalize().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        let dir = home.path().join(".codemux/opencode-sync");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        let file = home.path().join(bundle.strip_prefix("~/").unwrap());
        assert_eq!(mode(&file), 0o600);
        let retry = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path().canonicalize().unwrap())
            .status()
            .unwrap();
        assert!(
            !retry.success(),
            "an existing bundle must not be overwritten"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "data");
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    }

    #[test]
    fn remote_bundle_dir_prep_refuses_a_symlinked_folder() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::set_permissions(elsewhere.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(home.path().join(".codemux")).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path(),
            home.path().join(".codemux/opencode-sync"),
        )
        .unwrap();
        let bundle = unique_remote_bundle();
        let script =
            remote_bundle_command(&bundle, true, "set -C\nprintf data > \"$cmx_file\"").unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home.path().canonicalize().unwrap())
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(
            std::fs::read_dir(elsewhere.path()).unwrap().count(),
            0,
            "nothing written through the link"
        );
        let mode = std::fs::metadata(elsewhere.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "the link target keeps its mode");
    }

    #[test]
    fn remote_bundle_operations_refuse_writable_parent_and_redirected_cleanup() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let app_dir = home.path().join(".codemux");
        std::fs::create_dir(&app_dir).unwrap();
        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let bundle = unique_remote_bundle();
        let write =
            remote_bundle_command(&bundle, true, "set -C\nprintf data > \"$cmx_file\"").unwrap();
        let run = |script: &str| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .env("HOME", home.path().canonicalize().unwrap())
                .status()
                .unwrap()
        };
        assert!(!run(&write).success());
        assert!(
            !app_dir.join("opencode-sync").exists(),
            "nothing created below the writable parent"
        );

        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(run(&write).success());
        let file = home.path().join(bundle.strip_prefix("~/").unwrap());
        std::fs::rename(&app_dir, home.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(home.path().join("elsewhere"), &app_dir).unwrap();
        let cleanup = remote_bundle_cleanup_command(&bundle).unwrap();
        assert!(!run(&cleanup).success());
        assert!(home
            .path()
            .join("elsewhere/opencode-sync")
            .join(file.file_name().unwrap())
            .exists());
    }

    #[test]
    fn remote_extract_script_sql_escapes_apostrophe_paths() {
        // A path with an apostrophe must be SQL-escaped into the query (the
        // apostrophe doubled), exactly like the local-side newest_session_sql —
        // never spliced raw via a shell variable, which breaks the SQL literal.
        let cwd = "/Users/O'Brien/ws";
        let s = remote_extract_script(cwd, "~/.codemux/opencode-sync/b.json");
        assert!(
            s.contains(&shell_sq(&newest_session_sql(cwd))),
            "must embed the SQL-escaped query; got:\n{s}"
        );
        assert!(
            !s.contains("'$dir'"),
            "must not splice the raw path into the SQL string literal"
        );
    }

    #[test]
    fn unique_tokens_differ() {
        // Two bundles requested back to back must not collide.
        assert_ne!(unique_remote_bundle(), unique_remote_bundle());
        let a = local_bundle().unwrap();
        let b = local_bundle().unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a.path().to_string_lossy().contains("codemux-opencode-"));
    }

    #[test]
    fn cleanup_only_targets_bundles_inside_the_bundle_folder() {
        assert!(remote_bundle_name(&unique_remote_bundle()).is_some());
        assert!(remote_bundle_name("~/.codemux/opencode-sync/b.json").is_some());
        for path in [
            "~/.codemux/opencode-sync",
            "~/.codemux/opencode-sync/",
            "~/.codemux/opencode-sync/../x.json",
            "~/.codemux/opencode-sync/a/../../x.json",
            "~/.codemux/opencode-sync/sub/b.json",
            "~/.codemux/opencode-syncx/b.json",
            "/tmp/codemux-opencode-1.json",
            "~/.ssh/authorized_keys",
            "~/.codemux/opencode-sync/$(touch bad)",
            "",
        ] {
            assert!(remote_bundle_name(path).is_none(), "{path}");
        }
    }

    #[tokio::test]
    async fn bounded_cleanup_gives_up_on_a_hung_command() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let result = run_bounded(cmd, Duration::from_millis(200)).await;
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn bounded_cleanup_reports_a_failed_command() {
        assert!(run_bounded(Command::new("false"), Duration::from_secs(5))
            .await
            .is_err());
        assert!(run_bounded(Command::new("true"), Duration::from_secs(5))
            .await
            .is_ok());
    }

    #[test]
    fn local_bundle_is_owner_only_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let bundle = local_bundle().unwrap();
        let path = bundle.path().to_path_buf();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(bundle);
        assert!(!path.exists());
    }
}
