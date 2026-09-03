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
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

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

    match wait_with_deadline(child, Instant::now() + timeout) {
        Ok(status) => Ok(TimedOutput {
            success: status.success(),
            stdout: join(stdout),
            stderr: join(stderr),
        }),
        Err(failure) => {
            // Whichever way the wait failed, the child has been killed and
            // reaped by now. The drain threads are deliberately not joined:
            // there is no output to report, and a grandchild still holding
            // the write end would make the join the very hang the deadline
            // exists to prevent. The readers end at EOF on their own.
            drop(stdout);
            drop(stderr);
            Err(failure)
        }
    }
}

/// Block until the child exits or `deadline` passes.
///
/// Returns the exit status on a normal exit. On every error path the child
/// has been killed *and reaped* before returning, so a caller that gives up
/// on a subprocess can never leak a zombie — the same ownership rule the
/// per-CLI runners always had.
///
/// The obvious `try_wait` + `sleep` loop was what this used to be, and its
/// sleep quantum became the floor on every call: a 3ms `git config --get`
/// cost a full 50ms tick, and a PR lookup doing four of them paid 200ms
/// before any network round trip. A blocking `wait` on its own thread with
/// a `recv_timeout` on the caller's side returns the moment the child is
/// gone and still honours the deadline.
#[cfg(unix)]
fn wait_with_deadline(mut child: Child, deadline: Instant) -> Result<ExitStatus, TimedFailure> {
    use std::sync::mpsc::{self, RecvTimeoutError};

    // The caller keeps the `Child` — and with it the sole right to kill and
    // reap — exactly as before. The observer thread only *watches* for the
    // exit: `waitid(WNOWAIT)` blocks until the child is a zombie but does not
    // collect it, so the pid stays pinned to our child and a kill sent after
    // the deadline can never land on a recycled pid. Reaping is the caller's
    // `wait` below, in every branch.
    let pid = child.id() as libc::id_t;
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        // SAFETY: `info` is a valid, writable siginfo_t for the duration of
        // the call; the pid is one we spawned. Only the return matters.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        loop {
            let rc = unsafe {
                libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT)
            };
            if rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                // Exited (or already reaped by the timeout path → ECHILD;
                // the receiver is gone in that case and the send is moot).
                break;
            }
        }
        let _ = tx.send(());
    });

    let remaining = deadline.saturating_duration_since(Instant::now());
    match rx.recv_timeout(remaining) {
        // Exited before the deadline: collect it. `wait` returns at once.
        Ok(()) => reap(&mut child),
        Err(RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(TimedFailure::Timeout)
        }
        // The observer died before signalling. Nothing about the child is
        // known, so treat it like the deadline path: kill, reap, report.
        Err(RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(TimedFailure::Wait(std::io::Error::other(
                "subprocess observer thread exited without reporting",
            )))
        }
    }
}

/// Collect an exited child's status. A wait failure does not imply the child
/// is gone, so apply the same ownership rule as the deadline path — kill and
/// reap before returning — so an exceptional OS error can never leak a child
/// or zombie.
#[cfg(unix)]
fn reap(child: &mut Child) -> Result<ExitStatus, TimedFailure> {
    child.wait().map_err(|e| {
        let _ = child.kill();
        let _ = child.wait();
        TimedFailure::Wait(e)
    })
}

/// Non-unix fallback: there is no `waitid(WNOWAIT)` to observe an exit
/// without collecting it, so the caller keeps the `Child` and polls. The
/// interval starts well under a millisecond and doubles up to a 50ms
/// ceiling, so a fast child still returns almost immediately while a
/// long-running one is not busy-polled.
#[cfg(not(unix))]
fn wait_with_deadline(mut child: Child, deadline: Instant) -> Result<ExitStatus, TimedFailure> {
    const MAX_POLL_INTERVAL: Duration = Duration::from_millis(50);
    let mut interval = Duration::from_micros(250);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(TimedFailure::Timeout);
                }
                std::thread::sleep(interval);
                interval = (interval * 2).min(MAX_POLL_INTERVAL);
            }
            Err(e) => {
                // A wait failure does not imply the child exited. Apply the
                // same ownership rule as the deadline path: kill and reap
                // before returning so a background poll can never leak a
                // child or zombie on an exceptional OS error.
                let _ = child.kill();
                let _ = child.wait();
                return Err(TimedFailure::Wait(e));
            }
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

    /// The regression behind the rewrite: the old `try_wait` + 50ms sleep
    /// loop made every call cost at least one full sleep quantum, so a
    /// 3ms `git config --get` took 50ms and a PR lookup doing four of them
    /// paid 200ms before any network I/O. A trivially fast child must now
    /// return in a few milliseconds. Measured over several runs so one
    /// slow spawn on a loaded CI box does not fail the assertion alone.
    #[cfg(unix)]
    #[test]
    fn a_fast_child_returns_without_paying_a_poll_quantum() {
        // Warm up: the first spawn pays for page-cache/dynamic-loader work
        // that has nothing to do with the wait strategy under test.
        let _ = run_timed(Command::new("true"), Duration::from_secs(10));

        let mut best = Duration::MAX;
        for _ in 0..5 {
            let started = Instant::now();
            let out = run_timed(Command::new("true"), Duration::from_secs(10))
                .ok()
                .expect("`true` should complete");
            best = best.min(started.elapsed());
            assert!(out.success);
        }
        assert!(
            best < Duration::from_millis(40),
            "a fast child took {best:?} at best; the wait must return on exit, not on a poll tick"
        );
    }

    /// A child that ignores the deadline is killed promptly: the deadline
    /// is honoured to within scheduling noise, not waited out and not
    /// rounded up to some poll tick.
    #[cfg(unix)]
    #[test]
    fn a_slow_child_is_killed_at_the_deadline_not_waited_out() {
        let started = Instant::now();
        let result = run_timed(sh("exec sleep 5"), Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(TimedFailure::Timeout)));
        assert!(
            elapsed >= Duration::from_millis(200),
            "returned after {elapsed:?}, before the deadline"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "timed out after {elapsed:?}; the deadline must be enforced promptly"
        );
    }

    /// The zombie check proper: after a timeout the child must be reaped,
    /// so its pid is no longer a child of ours. `waitpid` on it returning
    /// `ECHILD` is the proof; a zombie would return the pid instead.
    #[cfg(unix)]
    #[test]
    fn a_timed_out_child_is_reaped_not_left_as_a_zombie() {
        // Spawn by hand so the pid is known, then hand ownership to
        // `wait_with_deadline` — the function that actually holds the
        // kill/reap contract.
        let mut cmd = Command::new("sleep");
        cmd.arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().expect("sleep should spawn");
        let pid = child.id() as libc::pid_t;

        let started = Instant::now();
        let result = wait_with_deadline(child, Instant::now() + Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(TimedFailure::Timeout)));
        assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");

        let mut status: libc::c_int = 0;
        // SAFETY: waitpid on a pid we spawned with a valid status pointer.
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            rc, -1,
            "waitpid returned {rc}: the child was not reaped by the timeout path"
        );
        assert_eq!(errno, Some(libc::ECHILD), "expected ECHILD, got {errno:?}");
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
