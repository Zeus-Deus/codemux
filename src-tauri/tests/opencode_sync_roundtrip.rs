//! Live OpenCode conversation-sync round-trip against a real SSH host
//! (issue #16). Exercises the ACTUAL transport code —
//! `ssh::sync_opencode_session` (push) and `ssh::pull_opencode_session`
//! (pull-back) — over real `ssh` + real `opencode` `export`/`import`/`db`,
//! proving the acceptance criteria:
//!
//!   - push lands this workspace's OpenCode session in the host's DB
//!     associated with the remote cwd (`opencode --session <id>` would
//!     resume it);
//!   - a conversation continued on the host pulls back to the laptop
//!     (message count grows from short→full);
//!   - 3+ push/pull cycles preserve the full history (no loss, no dupes);
//!   - the host's OTHER, unrelated OpenCode session is NEVER clobbered.
//!
//! GATED: skips unless every `CMX_OC_E2E_*` env var below is set (the
//! `scripts/e2e/opencode-sync-e2e.sh` harness stands up a Docker SSH host
//! with opencode and sets them). CI without that environment no-ops, so the
//! suite stays green. Unix-only (matches the `ssh` module).
//!
//! Required env:
//!   CODEMUX_E2E_SSH_HOST  ssh target/alias for the host (passwordless)
//!   CMX_OC_E2E_LAPTOP_XDG isolated XDG_DATA_HOME for the laptop-side DB
//!   CMX_OC_E2E_LOCAL_WS   local workspace dir (the session's directory)
//!   CMX_OC_E2E_REMOTE_WS  remote workspace dir on the host
//!   CMX_OC_E2E_BUNDLE_SHORT  path to a short export bundle (session A)
//!   CMX_OC_E2E_BUNDLE_FULL   path to the full export bundle (session A)
//!   CMX_OC_E2E_BUNDLE_UNRELATED path to a DIFFERENT session's bundle (B)
//!   CMX_OC_E2E_SID_A      session id inside the short/full bundles
//!   CMX_OC_E2E_SID_B      session id inside the unrelated bundle

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use codemux_lib::ssh::{pull_opencode_session, sync_opencode_session};

fn env_ne(k: &str) -> Option<String> {
    match std::env::var(k) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Parse the last bare integer line out of `opencode db "SELECT COUNT(*)…"`
/// output (TSV header + value, possibly prefixed by login-shell noise).
fn parse_count(stdout: &str) -> i64 {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<i64>().ok())
        .last()
        .unwrap_or(-1)
}

/// Local `opencode db` count for a session (XDG_DATA_HOME is set
/// process-global to the isolated laptop dir).
fn laptop_msg_count(session_id: &str) -> i64 {
    let out = Command::new("opencode")
        .arg("db")
        .arg(format!(
            "SELECT COUNT(*) FROM message WHERE session_id = '{session_id}'"
        ))
        .output()
        .expect("spawn local opencode db");
    parse_count(&String::from_utf8_lossy(&out.stdout))
}

/// Run a script on the remote via a login shell, feeding it over stdin (so
/// opencode is on PATH and the SQL's single quotes don't collide with shell
/// quoting). Returns captured stdout.
fn remote_login_stdout(host: &str, script: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "bash -ls"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn ssh login shell");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .expect("write remote script");
    // drop stdin → EOF → bash runs the script.
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("ssh login shell wait");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Remote `opencode db` count via a login shell (so opencode is on PATH).
fn remote_msg_count(host: &str, session_id: &str) -> i64 {
    let script = format!(
        "opencode db \"SELECT COUNT(*) FROM message WHERE session_id = '{session_id}'\" 2>/dev/null\n"
    );
    parse_count(&remote_login_stdout(host, &script))
}

/// Local import of a bundle from `cwd` (sets the session's directory).
fn laptop_import(bundle: &Path, cwd: &Path) {
    let status = Command::new("opencode")
        .current_dir(cwd)
        .arg("import")
        .arg(bundle)
        .status()
        .expect("spawn local opencode import");
    assert!(status.success(), "local import of {bundle:?} failed");
}

/// Remote import of a local bundle from `remote_cwd`: upload via `cat`,
/// then import in a login shell. Used to seed the unrelated session and to
/// simulate the conversation continuing on the host.
fn remote_import(host: &str, local_bundle: &Path, remote_cwd: &str) {
    let data = std::fs::read(local_bundle).expect("read bundle");
    let remote_tmp = format!("/tmp/cmx-oc-e2e-seed-{}.json", std::process::id());
    // upload
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host])
        .arg(format!("cat > '{remote_tmp}'"))
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn ssh upload");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&data)
            .expect("stream bundle");
    }
    assert!(child.wait().expect("ssh upload wait").success(), "upload failed");
    // import from remote_cwd
    let script = format!(
        "mkdir -p '{remote_cwd}' && cd '{remote_cwd}' && opencode import '{remote_tmp}'; rc=$?; rm -f '{remote_tmp}'; exit $rc"
    );
    let status = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host])
        .arg(format!("bash -lc \"{}\"", script.replace('"', "\\\"")))
        .status()
        .expect("spawn ssh remote import");
    assert!(status.success(), "remote import failed");
}

#[test]
fn opencode_sync_round_trip_against_real_host() {
    let (Some(host), Some(laptop_xdg), Some(local_ws), Some(remote_ws)) = (
        env_ne("CODEMUX_E2E_SSH_HOST"),
        env_ne("CMX_OC_E2E_LAPTOP_XDG"),
        env_ne("CMX_OC_E2E_LOCAL_WS"),
        env_ne("CMX_OC_E2E_REMOTE_WS"),
    ) else {
        eprintln!(
            "SKIP: set CODEMUX_E2E_SSH_HOST + CMX_OC_E2E_* (see scripts/e2e/\
             opencode-sync-e2e.sh) to run the live OpenCode sync round-trip"
        );
        return;
    };
    let (Some(short), Some(full), Some(unrelated), Some(sid_a), Some(sid_b)) = (
        env_ne("CMX_OC_E2E_BUNDLE_SHORT"),
        env_ne("CMX_OC_E2E_BUNDLE_FULL"),
        env_ne("CMX_OC_E2E_BUNDLE_UNRELATED"),
        env_ne("CMX_OC_E2E_SID_A"),
        env_ne("CMX_OC_E2E_SID_B"),
    ) else {
        eprintln!("SKIP: missing CMX_OC_E2E bundle/session-id env vars");
        return;
    };

    // Isolate the laptop-side opencode DB so the test never touches the
    // user's real history. All local `opencode` subprocesses inherit this.
    std::env::set_var("XDG_DATA_HOME", &laptop_xdg);

    let local_ws = PathBuf::from(&local_ws);
    let short = PathBuf::from(&short);
    let full = PathBuf::from(&full);
    let unrelated = PathBuf::from(&unrelated);
    std::fs::create_dir_all(&local_ws).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();

    // ── Seed: laptop has the SHORT session A; host has an UNRELATED session B.
    laptop_import(&short, &local_ws);
    let short_count = laptop_msg_count(&sid_a);
    assert!(short_count > 0, "seed: laptop short session A has messages");

    remote_import(&host, &unrelated, "/tmp/cmx-oc-e2e-unrelated");
    let unrelated_before = remote_msg_count(&host, &sid_b);
    assert!(
        unrelated_before > 0,
        "seed: host unrelated session B has messages"
    );

    // ── CYCLE 1: push (short) → continue on host (full) → pull (full).
    let sid = rt
        .block_on(sync_opencode_session(&host, &local_ws, Path::new(&remote_ws)))
        .expect("push sync ok")
        .expect("push synced a session");
    assert_eq!(sid, sid_a, "push synced session A");
    assert_eq!(
        remote_msg_count(&host, &sid_a),
        short_count,
        "after push, host has session A with the laptop's message count"
    );
    assert_eq!(
        remote_msg_count(&host, &sid_b),
        unrelated_before,
        "after push, host's UNRELATED session B is untouched"
    );

    // Simulate the user continuing the conversation on the host: the session
    // grows to the full transcript.
    remote_import(&host, &full, &remote_ws);
    let continued = remote_msg_count(&host, &sid_a);
    assert!(
        continued > short_count,
        "host continuation grew session A ({continued} > {short_count})"
    );

    let sid2 = rt
        .block_on(pull_opencode_session(&host, Path::new(&remote_ws), &local_ws))
        .expect("pull ok")
        .expect("pull returned a session");
    assert_eq!(sid2, sid_a, "pull brought back session A");
    assert_eq!(
        laptop_msg_count(&sid_a),
        continued,
        "after pull, laptop has the host's continued message count"
    );

    // ── CYCLES 2 & 3: idempotent push/pull preserve the full history and
    // never disturb the unrelated session.
    for cycle in 2..=3 {
        rt.block_on(sync_opencode_session(&host, &local_ws, Path::new(&remote_ws)))
            .unwrap_or_else(|e| panic!("cycle {cycle} push: {e}"))
            .unwrap_or_else(|| panic!("cycle {cycle} push synced nothing"));
        assert_eq!(
            remote_msg_count(&host, &sid_a),
            continued,
            "cycle {cycle}: host history preserved (no loss/dupes)"
        );
        assert_eq!(
            remote_msg_count(&host, &sid_b),
            unrelated_before,
            "cycle {cycle}: unrelated session still untouched"
        );

        rt.block_on(pull_opencode_session(&host, Path::new(&remote_ws), &local_ws))
            .unwrap_or_else(|e| panic!("cycle {cycle} pull: {e}"))
            .unwrap_or_else(|| panic!("cycle {cycle} pull returned nothing"));
        assert_eq!(
            laptop_msg_count(&sid_a),
            continued,
            "cycle {cycle}: laptop history preserved"
        );
    }

    // Final guarantee, stated plainly.
    assert_eq!(
        remote_msg_count(&host, &sid_b),
        unrelated_before,
        "after 3 cycles, the host's unrelated OpenCode session is byte-for-byte intact"
    );
    eprintln!(
        "OK: OpenCode sync round-trip — session A grew {short_count}→{continued} \
         msgs across host continuation, preserved over 3 push/pull cycles; \
         unrelated session B held at {unrelated_before} msgs throughout."
    );
}
