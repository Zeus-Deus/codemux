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
//! remote `opencode` invocation runs through a login shell. Each transfer
//! uses a private scratch directory created and removed by that shell.
//! A remote deadline bounds the transfer even if the SSH client disconnects.
//! The bundle travels over SSH on stdin for imports and stdout for exports.
//!
//! Everything here is best-effort: a failure only loses conversation
//! continuity (the agent still launches), so callers warn-and-continue,
//! mirroring the Claude sync.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

use super::exec::sh_quote;
use super::history_process::{output, ssh_command};

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

/// The NUL frame keeps login banners out of the JSON bundle. A JSON string
/// can encode a NUL with an escape sequence, never as a raw byte.
fn remote_export_bundle(stdout: &[u8]) -> Result<Option<(&str, &[u8])>, String> {
    let mut segments = stdout.splitn(3, |&byte| byte == 0);
    segments.next();
    let Some(frame) = segments.next() else {
        return Ok(None);
    };
    let separator = segments
        .next()
        .and_then(|_| frame.iter().position(|&byte| byte == b'\n'))
        .ok_or("Incomplete OpenCode export frame")?;
    let (id, bundle) = frame.split_at(separator);
    let session_id = std::str::from_utf8(id)
        .ok()
        .and_then(|id| id.strip_prefix("CMX_OC_SID="))
        .filter(|id| id.starts_with("ses_"))
        .ok_or("Invalid OpenCode export session ID")?;
    Ok(Some((session_id, &bundle[1..])))
}

const REMOTE_TRANSFER_TIMEOUT_SECS: u64 = 60;

// Keep cleanup in the parent shell. The supervisor can kill the entire
// transfer process group without killing the shell that removes the bundle.
const REMOTE_TRANSFER: &str = r#"run_transfer() {
    seconds=$1
    shift
    # Other timeout implementations can leave descendants running after a deadline.
    if [[ $(LC_ALL=C command timeout --version 2>/dev/null) == 'timeout (GNU coreutils)'* ]]; then
        command timeout -s KILL "$seconds" "$@"
    else
        command perl -MIO::Select -e '
            getpgrp() == $$ || setpgrp(0, 0) or die "setpgrp: $!";
            my $seconds = shift @ARGV;
            my $group = $$;
            pipe(my $reader, my $completed) or die "pipe: $!";
            my $watchdog = fork();
            defined $watchdog or die "fork: $!";
            if ($watchdog == 0) {
                close $completed;
                $SIG{HUP} = $SIG{INT} = $SIG{TERM} = "IGNORE";
                # The deadline must fire while the supervisor is still in
                # system(), even when a signal handler would be deferred.
                exit 0 if IO::Select->new($reader)->can_read($seconds)
                    && sysread($reader, my $finished, 1);
                kill "KILL", -$group;
                exit 1;
            }
            close $reader;
            my $status = system { $ARGV[0] } @ARGV;
            syswrite($completed, "1");
            close $completed;
            waitpid($watchdog, 0);
            exit($status == -1 ? 127 : ($status & 127) ? 128 + ($status & 127) : $status >> 8);
        ' "$seconds" "$@"
    fi
}
umask 077
scratch=$(mktemp -d /tmp/codemux-opencode.XXXXXXXX) || exit 1
bundle="$scratch/session.json"
trap 'rm -f "$bundle"; rmdir "$scratch"' EXIT
trap 'exit 1' HUP INT TERM PIPE
"#;

fn remote_transfer_script(body: &str, argument: &str, timeout_secs: u64) -> String {
    format!(
        "{REMOTE_TRANSFER}run_transfer {timeout_secs} bash -c {} codemux-opencode \"$bundle\" {}\n",
        shell_sq(body),
        shell_sq(argument)
    )
}

fn remote_import_script(remote_cwd: &str) -> String {
    // `mkdir -p` defends against the rare case where the workspace dir
    // isn't there yet (the push rsync normally creates it first); `cd` is
    // load-bearing because `opencode import` derives the session directory
    // from the process cwd.
    remote_transfer_script(
        r#"mkdir -p "$2" && cd "$2" || exit 3
cat > "$1" || exit 1
opencode import "$1""#,
        remote_cwd,
        REMOTE_TRANSFER_TIMEOUT_SECS,
    )
}

/// Find the newest root session and emit its ID and export in a single
/// frame. An empty successful response means there was no session.
fn remote_extract_script(remote_cwd: &str, timeout_secs: u64) -> String {
    // Build the query with the shared SQL helper (which SQL-escapes the
    // directory) and pass it as a single shell-quoted argument. Splicing the
    // path through a `$dir` shell variable into the SQL literal would break
    // the query for any path containing an apostrophe (e.g. /Users/O'Brien/…),
    // silently yielding "nothing to pull".
    remote_transfer_script(
        r#"sid=$(opencode db "$2" 2>/dev/null | grep -m1 '^ses_') || exit 0
opencode export "$sid" > "$1" || exit $?
printf '\000CMX_OC_SID=%s\n' "$sid"
cat "$1" && printf '\000'"#,
        &newest_session_sql(remote_cwd),
        timeout_secs,
    )
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

    let bundle = export_session_local(&bin, &session_id).await?;
    let script = remote_import_script(&remote_cwd.to_string_lossy());
    let result = run_remote_login_script(ssh_target, &script, Some(&bundle)).await?;
    if !result.status.success() {
        return Err(format!(
            "remote `opencode import` failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
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

    let script = remote_extract_script(&remote_cwd.to_string_lossy(), REMOTE_TRANSFER_TIMEOUT_SECS);
    let result = run_remote_login_script(ssh_target, &script, None).await?;
    if !result.status.success() {
        return Err(format!(
            "remote `opencode export` failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let Some((session_id, bundle)) = remote_export_bundle(&result.stdout)? else {
        eprintln!(
            "[hosts] no remote OpenCode session at {}; skipping OpenCode pull-back",
            remote_cwd.display()
        );
        return Ok(None);
    };

    // OpenCode imports from a path. Keep the bundle in a private directory
    // that is also removed if the history deadline cancels this future.
    let scratch = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|e| format!("create OpenCode import directory: {e}"))?;
    let local_bundle = scratch.path().join("session.json");
    tokio::fs::write(&local_bundle, bundle)
        .await
        .map_err(|error| format!("Write local OpenCode bundle: {error}"))?;

    import_session_local(&bin, local_cwd, &local_bundle).await?;

    eprintln!(
        "[hosts] pulled OpenCode session {session_id} back from {ssh_target} → {}",
        local_cwd.display()
    );
    Ok(Some(session_id.to_owned()))
}

// ── Local opencode invocations ─────────────────────────────────────────────

async fn local_newest_session_id(bin: &Path, dir: &str) -> Result<Option<String>, String> {
    let mut command = Command::new(bin);
    command.arg("db").arg(newest_session_sql(dir));
    let out = output(&mut command, None)
        .await
        .map_err(|e| format!("run `opencode db`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`opencode db` query failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_session_id(&String::from_utf8_lossy(&out.stdout)))
}

async fn export_session_local(bin: &Path, session_id: &str) -> Result<Vec<u8>, String> {
    let mut command = Command::new(bin);
    command.arg("export").arg(session_id);
    let out = output(&mut command, None)
        .await
        .map_err(|e| format!("run `opencode export`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`opencode export {session_id}` failed ({})",
            out.status
        ));
    }
    if out.stdout.is_empty() {
        return Err("`opencode export` produced an empty bundle".into());
    }
    Ok(out.stdout)
}

async fn import_session_local(bin: &Path, cwd: &Path, bundle: &Path) -> Result<(), String> {
    // `import` derives the session's directory from the process cwd, so run
    // it from the workspace dir to associate the conversation locally.
    let mut command = Command::new(bin);
    command.current_dir(cwd).arg("import").arg(bundle);
    let out = output(&mut command, None)
        .await
        .map_err(|e| format!("run `opencode import`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`opencode import` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ── SSH transport helpers ──────────────────────────────────────────────────

/// Run the transfer in a login shell so the user's OpenCode is on PATH.
/// Keep stdin available for the bundle and force pipes for its raw bytes.
async fn run_remote_login_script(
    ssh_target: &str,
    script: &str,
    input: Option<&[u8]>,
) -> Result<Output, String> {
    let mut command = ssh_command(ssh_target);
    command.arg(format!("bash -lc {}", sh_quote(script)));
    output(&mut command, input)
        .await
        .map_err(|e| format!("SSH OpenCode transfer failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt as _, BufReader};

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
    fn remote_export_frame_excludes_shell_output() {
        let output = b"shell banner\n\0CMX_OC_SID=ses_abc123\n{\"fixture\":1}\n\0shell footer\n";
        let (id, bundle) = remote_export_bundle(output).unwrap().unwrap();
        assert_eq!(id, "ses_abc123");
        assert_eq!(bundle, b"{\"fixture\":1}\n");
    }

    #[test]
    fn remote_export_frame_requires_both_delimiters_and_session_id() {
        assert_eq!(remote_export_bundle(b"nothing here\n").unwrap(), None);
        assert!(remote_export_bundle(b"\0CMX_OC_SID=ses_fixture\n{}").is_err());
        assert!(remote_export_bundle(b"\0CMX_OC_SID=bad_id\n{}\0").is_err());
    }

    #[test]
    fn shell_sq_quotes_and_escapes() {
        assert_eq!(shell_sq("/tmp/x.json"), "'/tmp/x.json'");
        assert_eq!(shell_sq("a'b"), "'a'\\''b'");
    }

    fn transfer_fixture(script: &str, opencode: &str) -> Command {
        let mut command = Command::new("bash");
        // Keep the fixture's scratch files in the test runner's temporary directory.
        let fixture = format!(
            r#"mktemp() {{
    directory=$(command mktemp "$1" "${{TMPDIR:-/tmp}}/${{2##*/}}") || return 1
    printf '%s\n' "$directory" >&2
    ls -ld "$directory" >&2
    printf '%s\n' "$directory"
}}
opencode() {{
{opencode}
}}
export -f opencode
{script}"#
        );
        command.args(["--noprofile", "--norc", "-c", &fixture]);
        command
    }

    async fn run_transfer_fixture(script: &str, opencode: &str, input: Option<&[u8]>) -> Output {
        let mut command = transfer_fixture(script, opencode);
        output(&mut command, input).await.unwrap()
    }

    fn non_gnu_timeout_with_blocked_perl_alarm(script: &str) -> String {
        // A compatible version flag doesn't establish group-kill behavior.
        // Programs started over SSH can also inherit a blocked SIGALRM.
        let prefix = r#"command() {
    case "$1" in
        timeout)
            if [[ "$2" == --version ]]; then
                printf 'timeout (uutils coreutils) 0.5.0\n'
                return 0
            fi
            printf 'unexpected non-GNU timeout invocation\n' >&2
            return 99
            ;;
        perl)
            shift
            builtin command perl -MPOSIX -e '
                sigprocmask(SIG_BLOCK, POSIX::SigSet->new(SIGALRM)) or die "sigprocmask: $!";
                exec "perl", @ARGV or die "exec perl: $!";
            ' -- "$@"
            ;;
        *) builtin command "$@" ;;
    esac
}
"#;
        [prefix, script].concat()
    }

    fn assert_private_scratch_removed(output: &Output) {
        let diagnostics = std::str::from_utf8(&output.stderr).unwrap();
        let mut lines = diagnostics.lines();
        let directory = Path::new(lines.next().unwrap());
        assert!(lines.next().unwrap().starts_with("drwx------"));
        assert!(!directory.exists(), "scratch remains at {directory:?}");
    }

    #[tokio::test]
    async fn remote_import_uses_private_scratch_and_cleans_up_on_failure() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = fixture.path().join("user's workspace");
        let script = remote_import_script(workspace.to_str().unwrap());
        let fake_opencode = r#"
test "$1" = import && test "${PWD##*/}" = "user's workspace" || return 3
ls -l "$2" >&2
cat "$2"
return 17
"#;
        let contents = b"{\"fixture\":1}\n";
        let result = run_transfer_fixture(&script, fake_opencode, Some(contents)).await;
        assert_eq!(result.status.code(), Some(17));
        assert_eq!(result.stdout, contents);
        assert_private_scratch_removed(&result);
        let diagnostics = std::str::from_utf8(&result.stderr).unwrap();
        assert!(diagnostics
            .lines()
            .nth(2)
            .unwrap()
            .starts_with("-rw-------"));
    }

    #[tokio::test]
    async fn remote_export_sends_bundle_and_cleans_up_private_scratch() {
        let script = remote_extract_script("/home/me/ws", REMOTE_TRANSFER_TIMEOUT_SECS);
        // The query is built by the shared SQL helper and passed as one
        // shell-quoted argument (no raw $dir splice into the SQL literal).
        assert!(script.contains(&shell_sq(&newest_session_sql("/home/me/ws"))));
        let fake_opencode = r#"
case "$1" in
    db) printf 'id\nses_fixture\n' ;;
    export) test "$2" = ses_fixture && printf '{"fixture":1}\n' ;;
    *) return 3 ;;
esac
"#;
        let fallback = non_gnu_timeout_with_blocked_perl_alarm(&script);
        for script in [&script, &fallback] {
            let result = run_transfer_fixture(script, fake_opencode, None).await;
            assert!(result.status.success(), "{result:?}");
            let (id, bundle) = remote_export_bundle(&result.stdout).unwrap().unwrap();
            assert_eq!(id, "ses_fixture");
            assert_eq!(bundle, b"{\"fixture\":1}\n");
            assert_private_scratch_removed(&result);
        }
    }

    #[tokio::test]
    async fn remote_export_cleans_up_after_disconnect_during_stall() {
        // GNU timeout can report the deadline (124) or SIGKILL (137).
        assert_remote_export_cleanup(
            &remote_extract_script("/home/me/ws", 2),
            &[124, 128 + libc::SIGKILL],
        )
        .await;
    }

    #[tokio::test]
    async fn perl_export_deadline_works_with_a_blocked_alarm_signal() {
        let script = remote_extract_script("/home/me/ws", 2);
        assert_remote_export_cleanup(
            &non_gnu_timeout_with_blocked_perl_alarm(&script),
            &[128 + libc::SIGKILL],
        )
        .await;
    }

    async fn assert_remote_export_cleanup(script: &str, timeout_statuses: &[i32]) {
        let fake_opencode = r#"
case "$1" in
    db) printf 'id\nses_fixture\n' ;;
    export)
        printf 'partial export\n'
        printf 'export started\n' >&2
        sleep 60
        ;;
    *) return 3 ;;
esac
"#;
        let mut child = transfer_fixture(script, fake_opencode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut diagnostics = BufReader::new(child.stderr.take().unwrap()).lines();
        let directory = tokio::time::timeout(Duration::from_secs(10), async {
            let directory = PathBuf::from(diagnostics.next_line().await.unwrap().unwrap());
            assert!(diagnostics
                .next_line()
                .await
                .unwrap()
                .unwrap()
                .starts_with("drwx------"));
            assert_eq!(
                diagnostics.next_line().await.unwrap().unwrap(),
                "export started"
            );
            directory
        })
        .await
        .expect("export started before disconnect");
        assert!(directory.join("session.json").is_file());

        // A disconnected SSH client can close the pipe without signaling
        // the waiting shell. Leave stderr open to observe worker exit.
        drop(child.stdout.take().unwrap());
        let status = tokio::time::timeout(Duration::from_secs(15), async {
            let status = child.wait().await.unwrap();
            while diagnostics.next_line().await.unwrap().is_some() {}
            status
        })
        .await
        .expect("the remote transfer and its children exited");
        assert!(!directory.exists(), "scratch remains at {directory:?}");
        assert!(
            status.code().is_some_and(|code| timeout_statuses.contains(&code)),
            "unexpected timeout status: {status}"
        );
    }

    #[test]
    fn remote_extract_script_sql_escapes_apostrophe_paths() {
        // A path with an apostrophe must be SQL-escaped into the query (the
        // apostrophe doubled), exactly like the local-side newest_session_sql —
        // never spliced raw via a shell variable, which breaks the SQL literal.
        let cwd = "/Users/O'Brien/ws";
        let s = remote_extract_script(cwd, REMOTE_TRANSFER_TIMEOUT_SECS);
        assert!(
            s.contains(&shell_sq(&newest_session_sql(cwd))),
            "must embed the SQL-escaped query; got:\n{s}"
        );
        assert!(
            !s.contains("'$dir'"),
            "must not splice the raw path into the SQL string literal"
        );
    }
}
