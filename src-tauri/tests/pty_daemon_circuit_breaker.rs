//! Crash-circuit-breaker unit tests for the PTY daemon supervisor.
//!
//! The breaker is the guarantee that a broken daemon (binary missing, no
//! permissions in $HOME, kernel refusing to spawn detached processes, etc.)
//! does not turn into a tight respawn loop that burns CPU and battery. We
//! cap at 3 failures within 60 seconds; past that, `circuit_is_open`
//! returns true and the spawn paths in `terminal/mod.rs` fall back to
//! in-process for the rest of the process lifetime.
//!
//! Tests use the internal `reset_circuit` test hook because the breaker
//! state is process-global by design (it tracks "this app instance has
//! given up on the daemon"). Running them with `--test-threads=1` keeps
//! the parallel-test runner from interleaving resets.

#![cfg(unix)]

use codemux_lib::pty_daemon::supervisor;

#[test]
fn circuit_starts_closed() {
    supervisor::reset_circuit();
    assert!(!supervisor::circuit_is_open());
    assert_eq!(supervisor::total_failures(), 0);
}

#[test]
fn ensure_daemon_failure_into_bogus_dir_trips_circuit_after_three_strikes() {
    supervisor::reset_circuit();
    // Point the manifest dir at an unwritable path so every ensure_daemon
    // call fails the same way (manifest write fails, daemon spawn fails,
    // socket never appears). On Linux, `/proc/self/root/..` style traps
    // are not portable, so we use a path inside `/sys` which is read-only
    // on essentially every running system (refuses mkdir).
    std::env::set_var("CODEMUX_PTY_DAEMON_DIR", "/sys/codemux-test-bogus");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // First three failures: circuit stays closed, errors propagate.
    for i in 1..=3 {
        let result = rt.block_on(supervisor::ensure_daemon());
        assert!(
            result.is_err(),
            "iteration {i}: ensure_daemon must fail against /sys/"
        );
    }
    assert!(supervisor::circuit_is_open(), "circuit should be open by now");

    // Subsequent calls fast-fail with the sentinel error and DO NOT
    // attempt another spawn — total_failures stays at 3 (the budget),
    // not 4 or higher.
    let pre = supervisor::total_failures();
    let result = rt.block_on(supervisor::ensure_daemon());
    let err = match result {
        Ok(_) => panic!("ensure_daemon must fast-fail once circuit is open"),
        Err(e) => e,
    };
    assert!(
        format!("{err}").contains("circuit breaker open"),
        "expected fast-fail sentinel, got: {err}"
    );
    assert_eq!(
        supervisor::total_failures(),
        pre,
        "fast-fail should NOT count against the failure budget"
    );

    supervisor::reset_circuit();
    std::env::remove_var("CODEMUX_PTY_DAEMON_DIR");
}

#[test]
fn reset_circuit_clears_state() {
    supervisor::reset_circuit();
    // Trip it manually via the failure recorder, then verify reset clears.
    std::env::set_var("CODEMUX_PTY_DAEMON_DIR", "/sys/codemux-test-bogus");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for _ in 0..3 {
        let _ = rt.block_on(supervisor::ensure_daemon());
    }
    assert!(supervisor::circuit_is_open());
    supervisor::reset_circuit();
    assert!(!supervisor::circuit_is_open());
    assert_eq!(supervisor::total_failures(), 0);
    std::env::remove_var("CODEMUX_PTY_DAEMON_DIR");
}
