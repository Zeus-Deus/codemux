//! Opt-in cloud-push diagnostic tracing.
//!
//! The push-to-host spawn path is wired with detailed per-step tracing
//! that was essential to finding the original cross-machine bug stack
//! (reattach guard, race suppression, stale-`Exited` guard, cwd
//! close-then-respawn). In normal operation those lines are pure noise —
//! a single 4-pane push emits dozens of them — so they are gated behind
//! the `CODEMUX_TRACE_CLOUD_PUSH` environment variable.
//!
//! Set `CODEMUX_TRACE_CLOUD_PUSH` to any value (e.g. `=1`) and every
//! `trace_cloud_push!` line prints to stderr again. Unset (the default),
//! each call is a single cached bool check and emits nothing. No
//! recompile and no code change is required to restore the full trace, so
//! if a future bug surfaces in the same area the diagnostics are one env
//! var away instead of forcing a re-discovery of what to log.
//!
//! The gate is process-wide and read from the environment, so it works
//! for both the desktop app and the headless `codemux-remote` daemon:
//! export the var before launching the process whose stderr you want to
//! inspect.

use std::sync::OnceLock;

/// Returns `true` when `CODEMUX_TRACE_CLOUD_PUSH` is present in the
/// environment. The result is cached on first read because the
/// environment is fixed for a process's lifetime, so hot spawn paths pay
/// the lookup cost exactly once.
pub fn cloud_push_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("CODEMUX_TRACE_CLOUD_PUSH").is_some())
}

/// `eprintln!`-compatible macro that only emits when cloud-push tracing
/// is enabled (see [`cloud_push_enabled`]).
///
/// Use it for per-spawn-step lifecycle logs — `spawn_via_daemon ENTRY`,
/// `DECISION=`, tunnel poll iterations, cwd dumps — that are noise in
/// normal operation but invaluable when debugging push-to-host. Keep
/// plain `eprintln!` for actionable errors and once-per-event landmarks
/// (relaunch commands, "tunnel did not come up", agent-not-installed).
#[macro_export]
macro_rules! trace_cloud_push {
    ($($arg:tt)*) => {
        if $crate::trace::cloud_push_enabled() {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_reflects_env_presence() {
        // The cached read is process-wide; this test only asserts the
        // function is callable and returns a stable bool (the value
        // depends on the ambient env, which `cargo test` does not set).
        let first = cloud_push_enabled();
        let second = cloud_push_enabled();
        assert_eq!(first, second, "gate must be stable within a process");
    }

    #[test]
    fn macro_compiles_and_is_silent_by_default() {
        // Exercising the macro must never panic regardless of the gate.
        trace_cloud_push!("trace probe {}", 1 + 1);
    }
}
