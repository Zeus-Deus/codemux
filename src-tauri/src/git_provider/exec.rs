//! One deadline-bounded subprocess runner, shared by every CLI shell-out
//! in the hosting integration.
//!
//! The naive shape — spawn with piped stdio, poll `try_wait`, read the
//! pipes once the child is gone — deadlocks. A pipe holds ~64KB before
//! the writer blocks, so a child that produces more than that (a large
//! merge-request diff, say) fills the buffer, blocks in `write`, never
//! exits, and the poll loop reports a timeout that never happened while
//! the output is still sitting there. Draining both streams on their own
//! threads for the whole life of the child is what makes the deadline
//! mean what it says.
//!
//! Errors are returned as a typed [`TimedFailure`] rather than a string
//! so each caller keeps the exact message its callers already parse.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often the loop checks whether the child has exited. Matches what
/// the per-CLI runners used before they were unified.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) struct TimedOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) enum TimedFailure {
    /// The binary could not be started (missing, not executable, …).
    Spawn(std::io::Error),
    /// Waiting on the child itself failed.
    Wait(std::io::Error),
    /// The deadline lapsed; the child was killed and reaped.
    Timeout,
}

/// Run `cmd` to completion or to `timeout`, whichever comes first.
///
/// `cmd` arrives fully configured (args, working directory, environment)
/// — only the stdio wiring is imposed here, because draining the pipes
/// is the entire point.
pub(crate) fn run_timed(
    mut cmd: Command,
    timeout: Duration,
) -> Result<TimedOutput, TimedFailure> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(TimedFailure::Spawn)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(TimedOutput {
                    success: status.success(),
                    stdout: join(stdout),
                    stderr: join(stderr),
                })
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Deliberately not joined: a timeout has no output to
                    // report, and a grandchild still holding the write
                    // end would make the join the very hang the deadline
                    // exists to prevent. The readers end at EOF.
                    drop(stdout);
                    drop(stderr);
                    return Err(TimedFailure::Timeout);
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(TimedFailure::Wait(e)),
        }
    }
}

/// Read one pipe to EOF on its own thread.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Option<std::thread::JoinHandle<String>> {
    let mut pipe = pipe?;
    Some(std::thread::spawn(move || {
        let mut buf = Vec::new();
        // Bytes, not `read_to_string`: a CLI that emits invalid UTF-8
        // (a diff of a binary file) must degrade to replacement
        // characters rather than losing the whole read.
        pipe.read_to_end(&mut buf).ok();
        String::from_utf8_lossy(&buf).into_owned()
    }))
}

fn join(handle: Option<std::thread::JoinHandle<String>>) -> String {
    handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[test]
    fn small_output_round_trips_on_both_streams() {
        let out = run_timed(sh("echo out; echo err 1>&2"), Duration::from_secs(10))
            .ok()
            .expect("should complete");
        assert!(out.success);
        assert_eq!(out.stdout.trim_end(), "out");
        assert_eq!(out.stderr.trim_end(), "err");
    }

    #[test]
    fn a_non_zero_exit_is_reported_not_treated_as_a_failure_to_run() {
        let out = run_timed(sh("echo partial; exit 3"), Duration::from_secs(10))
            .ok()
            .expect("should complete");
        assert!(!out.success);
        assert_eq!(out.stdout.trim_end(), "partial");
    }

    /// The regression this helper exists for: more output than a pipe
    /// buffer holds. Polling without draining blocks the child forever
    /// and reports a bogus timeout.
    #[test]
    fn output_larger_than_a_pipe_buffer_completes_with_everything_intact() {
        let out = run_timed(
            sh("head -c 200000 /dev/zero | tr '\\0' x"),
            Duration::from_secs(20),
        )
        .ok()
        .expect("must not time out on a large payload");
        assert!(out.success);
        assert_eq!(out.stdout.len(), 200_000);
        assert!(out.stdout.chars().all(|c| c == 'x'));
    }

    #[test]
    fn a_large_payload_on_stderr_is_drained_too() {
        let out = run_timed(
            sh("head -c 200000 /dev/zero | tr '\\0' y 1>&2"),
            Duration::from_secs(20),
        )
        .ok()
        .expect("must not time out on a large payload");
        assert_eq!(out.stderr.len(), 200_000);
    }

    #[test]
    fn a_child_that_outlives_the_deadline_is_killed_and_reported_as_a_timeout() {
        let started = Instant::now();
        let result = run_timed(sh("sleep 30"), Duration::from_millis(300));
        assert!(matches!(result, Err(TimedFailure::Timeout)));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn a_missing_binary_is_a_spawn_failure_not_a_timeout() {
        let result = run_timed(
            Command::new("codemux-definitely-not-a-real-binary-xyz"),
            Duration::from_secs(5),
        );
        assert!(matches!(result, Err(TimedFailure::Spawn(_))));
    }

    #[test]
    fn invalid_utf8_degrades_instead_of_losing_the_output() {
        let out = run_timed(sh("printf 'a\\377b'"), Duration::from_secs(10))
            .ok()
            .expect("should complete");
        assert!(out.stdout.starts_with('a'));
        assert!(out.stdout.ends_with('b'));
    }
}
