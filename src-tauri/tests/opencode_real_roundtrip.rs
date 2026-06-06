//! Live round-trip against a GENUINELY-RUN OpenCode session (issue #16,
//! verification tier (a)). Unlike `opencode_sync_roundtrip` — which seeds the
//! laptop DB by importing a bundle — this drives the real
//! `ssh::sync_opencode_session` / `pull_opencode_session` against a session
//! created by actually running `opencode` (real model turns), and proves the
//! pushed session **resumes on the host with full conversation context** via a
//! real model turn on the host, then pulls that host continuation back.
//!
//! Proves end-to-end, on real opencode data:
//!   - push transfers the genuinely-run session intact (message count matches);
//!   - a real `opencode --session <id>` turn ON THE HOST recalls a secret
//!     established on the laptop (context carried across the push);
//!   - pull brings the host's continuation back to the laptop;
//!   - the host's unrelated session is never clobbered.
//!
//! GATED on `CMX_OC_E2E_REAL=1` plus the env below — stood up by
//! `scripts/e2e/opencode-real-session-e2e.sh`, which needs OpenCode auth and a
//! (free) model, so it never runs in CI. Unix-only.
//!
//! Env: CODEMUX_E2E_SSH_HOST, CMX_OC_E2E_LAPTOP_XDG, CMX_OC_E2E_LOCAL_WS,
//! CMX_OC_E2E_REMOTE_WS, CMX_OC_E2E_SID_A, CMX_OC_E2E_SID_B,
//! CMX_OC_E2E_MODEL, CMX_OC_E2E_SECRET.

#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use codemux_lib::ssh::{pull_opencode_session, sync_opencode_session};

fn env_ne(k: &str) -> Option<String> {
    match std::env::var(k) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

fn parse_count(stdout: &str) -> i64 {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<i64>().ok())
        .last()
        .unwrap_or(-1)
}

/// Run a script on the host via a login shell over stdin (opencode on PATH).
fn remote(host: &str, script: &str) -> String {
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "bash -ls"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ssh login shell");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .expect("write remote script");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("ssh wait");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn laptop_count(sql: &str) -> i64 {
    let out = Command::new("opencode")
        .arg("db")
        .arg(sql)
        .output()
        .expect("spawn opencode db");
    parse_count(&String::from_utf8_lossy(&out.stdout))
}

fn laptop_msg_count(sid: &str) -> i64 {
    laptop_count(&format!(
        "SELECT COUNT(*) FROM message WHERE session_id = '{sid}'"
    ))
}

fn remote_msg_count(host: &str, sid: &str) -> i64 {
    parse_count(&remote(
        host,
        &format!(
            "opencode db \"SELECT COUNT(*) FROM message WHERE session_id = '{sid}'\" 2>/dev/null\n"
        ),
    ))
}

/// How many `part` rows of a session mention `needle` — used to detect that a
/// resume turn produced a NEW context-derived answer (count goes up).
fn remote_parts_like(host: &str, sid: &str, needle: &str) -> i64 {
    parse_count(&remote(
        host,
        &format!(
            "opencode db \"SELECT COUNT(*) FROM part WHERE session_id = '{sid}' AND data LIKE '%{needle}%'\" 2>/dev/null\n"
        ),
    ))
}

fn laptop_parts_like(sid: &str, needle: &str) -> i64 {
    laptop_count(&format!(
        "SELECT COUNT(*) FROM part WHERE session_id = '{sid}' AND data LIKE '%{needle}%'"
    ))
}

#[test]
fn real_session_round_trip_against_real_host() {
    if env_ne("CMX_OC_E2E_REAL").as_deref() != Some("1") {
        eprintln!("SKIP: set CMX_OC_E2E_REAL=1 (see scripts/e2e/opencode-real-session-e2e.sh)");
        return;
    }
    let (
        Some(host),
        Some(laptop_xdg),
        Some(local_ws),
        Some(remote_ws),
        Some(sid_a),
        Some(sid_b),
        Some(model),
        Some(secret),
    ) = (
        env_ne("CODEMUX_E2E_SSH_HOST"),
        env_ne("CMX_OC_E2E_LAPTOP_XDG"),
        env_ne("CMX_OC_E2E_LOCAL_WS"),
        env_ne("CMX_OC_E2E_REMOTE_WS"),
        env_ne("CMX_OC_E2E_SID_A"),
        env_ne("CMX_OC_E2E_SID_B"),
        env_ne("CMX_OC_E2E_MODEL"),
        env_ne("CMX_OC_E2E_SECRET"),
    )
    else {
        eprintln!("SKIP: missing CMX_OC_E2E_* env for the real-session round-trip");
        return;
    };

    // Isolated, but auth-bearing, laptop DB (the harness copied auth in).
    std::env::set_var("XDG_DATA_HOME", &laptop_xdg);
    let local_ws = PathBuf::from(&local_ws);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let laptop0 = laptop_msg_count(&sid_a);
    assert!(laptop0 > 0, "precondition: genuinely-run session A exists locally");
    let b0 = remote_msg_count(&host, &sid_b);
    assert!(b0 > 0, "precondition: host has an unrelated session B");

    // ── PUSH the genuinely-run session.
    let sid = rt
        .block_on(sync_opencode_session(&host, &local_ws, Path::new(&remote_ws)))
        .expect("push ok")
        .expect("push synced a session");
    assert_eq!(sid, sid_a, "pushed session A");
    assert_eq!(
        remote_msg_count(&host, &sid_a),
        laptop0,
        "real session transferred to host intact"
    );
    assert_eq!(
        remote_msg_count(&host, &sid_b),
        b0,
        "unrelated host session B untouched by push"
    );

    // ── RESUME ON THE HOST with a real model turn — the pushed session must
    //    recall the secret established on the laptop (context carried).
    let secret_before = remote_parts_like(&host, &sid_a, &secret);
    let _ = remote(
        &host,
        &format!(
            "cd '{remote_ws}' && opencode run --pure -m '{model}' -s '{sid_a}' \
             'What is the secret code I gave you earlier? Reply with ONLY the code.' \
             2>/dev/null\n"
        ),
    );
    let secret_after = remote_parts_like(&host, &sid_a, &secret);
    assert!(
        secret_after > secret_before,
        "host resume turn produced a NEW mention of the secret ({secret_before} → {secret_after}) \
         — the pushed session genuinely resumed WITH context on the host"
    );
    let host_count = remote_msg_count(&host, &sid_a);
    assert!(host_count > laptop0, "host turn extended the session");

    // ── PULL the host continuation back.
    let sid2 = rt
        .block_on(pull_opencode_session(&host, Path::new(&remote_ws), &local_ws))
        .expect("pull ok")
        .expect("pull returned a session");
    assert_eq!(sid2, sid_a, "pulled session A");
    assert_eq!(
        laptop_msg_count(&sid_a),
        host_count,
        "host continuation pulled back to the laptop"
    );
    assert!(
        laptop_parts_like(&sid_a, &secret) >= secret_after,
        "laptop now holds the host's context-derived continuation"
    );

    // ── No-clobber, stated plainly.
    assert_eq!(
        remote_msg_count(&host, &sid_b),
        b0,
        "after the full round-trip, the host's unrelated session is intact"
    );

    eprintln!(
        "OK: genuinely-run OpenCode session A pushed to host, RESUMED there with \
         full context (recalled the secret), continuation pulled back to laptop; \
         unrelated session held at {b0} msgs."
    );
}
