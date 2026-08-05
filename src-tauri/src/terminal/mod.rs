use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::io::RawFd;

/// Opaque poll handle passed to `batched_reader_loop`. On Unix this is the
/// raw fd of the PTY master (used by `poll()`); on Windows it is unused because
/// the placeholder reader does blocking reads.
#[cfg(unix)]
type PollFd = RawFd;
#[cfg(windows)]
type PollFd = ();
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, Runtime, State};

use crate::project::current_project_root;
use crate::settings_sync;
use crate::state::{self, AppStateStore, TerminalSessionState};

/// Persistent-agent path: routes spawns through `codemux pty-daemon` so
/// they survive the app being closed. Unix-only — Windows builds use the
/// in-process path exclusively.
#[cfg(unix)]
pub mod daemon_backed;

fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;
    let mut first = false; // next char is the byte right after ESC
    let mut in_osc = false; // OSC (ESC ]) ends only on BEL or ST, never a letter
    let mut osc_saw_esc = false; // inside OSC, saw ESC — awaiting '\' for ST

    for c in s.chars() {
        if !in_escape {
            if c == '\x1b' {
                in_escape = true;
                first = true;
                in_osc = false;
                osc_saw_esc = false;
            } else {
                result.push(c);
            }
            continue;
        }

        // The byte right after ESC selects the sequence type.
        if first {
            first = false;
            if c == ']' {
                in_osc = true;
                continue;
            }
            // `[` (CSI) and any other escape (charset selector, ESC-letter,
            // …) fall through to the letter-terminated heuristic below.
        }

        if in_osc {
            // OSC command strings terminate only on BEL (0x07) or ST (ESC \),
            // NOT on the letters in their payload (e.g. a window title).
            if osc_saw_esc {
                osc_saw_esc = false;
                if c == '\\' || c == '\x07' {
                    in_escape = false;
                } else if c == '\x1b' {
                    osc_saw_esc = true;
                }
            } else if c == '\x07' {
                in_escape = false;
            } else if c == '\x1b' {
                osc_saw_esc = true;
            }
            continue;
        }

        // CSI / other escapes end on a final letter (or @/`), or on ST/BEL.
        if c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == '@' || c == '`' {
            in_escape = false;
        } else if c == '\\' || c == '\x07' {
            in_escape = false;
        }
    }
    result
}

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
/// Maximum bytes of PTY output retained in a session's `pending_output` ring
/// buffer between when the frontend channel detaches (e.g. workspace switched
/// away from a parked agent pane — see `terminal-cache.ts::detachPtyChannel`)
/// and when it reattaches. Bounds per-session memory; once exceeded, oldest
/// chunks are evicted FIFO so memory stays bounded but any scrollback prior to
/// the eviction is permanently lost when the user returns to the workspace.
///
/// Sized for sustained verbose-agent runs (Claude Code / Codex / OpenCode)
/// streaming behind a switched-away workspace. 256 MiB holds ~1.5M typical
/// 150-byte token-delta chunks or ~8K max-size 32 KiB tool-output bursts —
/// enough headroom for a long thinking + tool-call sequence while the user
/// works in another workspace, without bounding away real RAM on idle sessions
/// (the cap is an upper bound, not a reservation).
///
/// Prior implementation capped at a fixed 1024 chunks regardless of chunk
/// size, which was a fuzzy proxy for memory: tiny token chunks evicted in a
/// few minutes of agent activity even though they used barely any RAM,
/// silently truncating the scrollback the user expected to see on return.
/// The chunk-count cap was originally chosen under the assumption that the
/// channel would stay attached for the session lifetime (per a now-stale
/// design note); the cache-architecture rework added park-time channel
/// detach for input-latency reasons, which made eviction routinely fire.
///
/// Test builds use a much smaller cap so eviction tests stay fast — tests
/// push 1-byte chunks so chunk-count and byte-count line up.
#[cfg(not(test))]
const OUTPUT_BUFFER_BYTE_LIMIT: usize = 256 * 1024 * 1024;
#[cfg(test)]
const OUTPUT_BUFFER_BYTE_LIMIT: usize = 1024;
/// PTY output batching: flush after this many accumulated bytes.
const PTY_BATCH_SIZE: usize = 32_768;
/// PTY output batching: flush after this much time since last flush (~1 frame at 60 Hz).
const PTY_BATCH_INTERVAL: Duration = Duration::from_millis(16);
/// Flow-control backstop: maximum time the in-process PTY read loop will stay
/// parked (not draining the master fd) before force-resuming itself. Mirrors
/// the daemon's `FLOW_MAX_PARK`. Guards against a renderer that asked to pause
/// (HIGH watermark) but never delivered the matching resume — e.g. a wedged or
/// crashed webview — so a paused PTY can never block the child process forever.
const FLOW_MAX_PARK: Duration = Duration::from_secs(10);
/// Poll cadence while the in-process read loop is flow-paused. Short enough that
/// resume is near-instant, long enough that a parked loop costs ~nothing.
const FLOW_PARK_POLL: Duration = Duration::from_millis(10);
/// Safety cap so we never spawn hundreds of PTYs on startup (e.g. after corrupted or stale persisted state).
const MAX_STARTUP_SESSIONS: usize = 50;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLifecycleState {
    Starting,
    /// Transient overlay shown on each pane while a workspace is being
    /// pushed to / pulled back from a remote host. Lives between the old
    /// PTY being terminated and the replacement session's first output.
    /// Maps to `Starting` in the persisted app-state (`map_status_state`)
    /// — it's just a start with a workspace-specific message — but rides
    /// the `terminal-status` event with its own `migrating` discriminant
    /// so the frontend can give it a distinct "Switching to <host>…" look.
    Migrating,
    Ready,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalStatusPayload {
    pub session_id: String,
    pub state: TerminalLifecycleState,
    pub message: Option<String>,
    pub exit_code: Option<u32>,
}

/// One live consumer of a session's PTY output stream.
///
/// A single PTY reader fans its output out to every subscriber in
/// `SessionRuntime::output_subscribers`. In the common case there is
/// exactly one (the desktop window's xterm), but the stream can be
/// mirrored to additional consumers (e.g. a browser client rendering the
/// same session) — each attaches its own `Channel` and gets an identical
/// byte stream.
pub struct OutputSubscriber {
    /// Monotonic id minted by `attach_pty_output`. A detach (or a failed
    /// send) only removes the subscriber whose generation matches, so a
    /// stale teardown from a consumer that already went away can never
    /// evict a newer subscriber that attached concurrently.
    pub generation: u64,
    pub channel: Channel<Vec<u8>>,
    /// This subscriber's flow-control request. The PTY reader only parks
    /// (applies back-pressure to the child) when EVERY subscriber has set
    /// this — see [`recompute_flow_paused`]. A caught-up subscriber keeps
    /// bytes flowing for all of them.
    pub flow_paused: bool,
}

pub struct SessionRuntime {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
    /// Live consumers of this session's PTY output. Empty when nothing is
    /// attached (output then accumulates in `pending_output` for replay on
    /// the next attach). `queue_or_send_output` fans each chunk out to all
    /// entries and prunes any whose channel send fails.
    pub output_subscribers: Vec<OutputSubscriber>,
    /// Monotonic source of subscriber generations. Bumped on every attach so
    /// each subscriber gets a unique id; a stale detach/failed-send can then
    /// target exactly one subscriber and never clobber a newer one that
    /// attached concurrently.
    pub next_output_generation: u64,
    pub pending_output: VecDeque<Vec<u8>>,
    /// Running total of `chunk.len()` across every entry in `pending_output`.
    /// Maintained alongside the deque so the eviction loop can cap by bytes
    /// (`OUTPUT_BUFFER_BYTE_LIMIT`) without paying an O(n) sum per push. Must
    /// stay in lockstep with the deque: every `push_back` adds the chunk
    /// length, every `pop_front` subtracts it, every `clear()` resets to 0.
    /// `saturating_*` arithmetic prevents arithmetic overflow on the rare
    /// pathological session that exceeds `usize::MAX` cumulative bytes.
    pub pending_output_bytes: usize,
    /// Cumulative count of chunks evicted from `pending_output` because the
    /// buffer hit `OUTPUT_BUFFER_BYTE_LIMIT` while no consumer was attached.
    ///
    /// `terminal-cache.ts` deliberately detaches the output channel when a
    /// workspace is parked (this trade was made to eliminate cross-workspace
    /// typing lag — hidden panes used to push bytes through Tauri IPC and
    /// xterm parsing on the renderer main thread, contending with the
    /// foreground pane). While detached, PTY output accumulates here; if the
    /// user stays away long enough for the byte cap to overflow, the oldest
    /// scrollback is dropped. A non-zero value here is the regression signal
    /// for "scrollback was lost while a workspace was parked." Persisted
    /// across attach/detach cycles so the cumulative loss across a session
    /// is observable, not just the most recent overflow.
    pub dropped_chunks: u64,
    /// Effective PTY producer back-pressure flag for the **in-process** read
    /// loop. When set, `batched_reader_loop` stops draining the PTY master fd;
    /// the kernel PTY buffer then fills and the child's next `write()` blocks —
    /// real back-pressure straight to the producer instead of an unbounded
    /// renderer queue (or, worse, `pending_output` eviction once the 256 MiB
    /// ring caps).
    ///
    /// This is a *derived* flag: `recompute_flow_paused` sets it true only when
    /// there is at least one subscriber and EVERY subscriber has requested a
    /// pause (see [`OutputSubscriber::flow_paused`]). A single caught-up
    /// subscriber keeps bytes flowing for the whole fan-out. The per-subscriber
    /// requests are driven by `set_pty_flow_paused` (the `pause_pty_output` /
    /// `resume_pty_output` commands the renderer calls at its HIGH/LOW
    /// watermarks), and the derived flag is recomputed on every attach, detach,
    /// and pause/resume. Self-heals so a dropped resume can never wedge a PTY:
    /// a fresh attach adds an unpaused subscriber (clearing the derived flag), a
    /// detach drops that subscriber's request with it, and a `FLOW_MAX_PARK`
    /// backstop inside the read loop force-resumes as a last resort.
    ///
    /// Shared (`Arc`) with the reader thread so toggling it needs no lock on
    /// the hot read path. Daemon-backed (persistent) sessions don't run the
    /// in-process loop — their back-pressure routes through the daemon's own
    /// `flow_paused` flag (the recomputed verdict is forwarded there) — so this
    /// stays unread for them (harmless).
    pub flow_paused: Arc<AtomicBool>,
    pub last_status: TerminalStatusPayload,
    pub child_pid: Option<u32>,
    pub skip_preset_launch: bool,
    /// Optional full launch command for restored sessions.
    /// When present, the preset readiness helper injects this command instead
    /// of the normal preset command.
    pub resume_command: Option<String>,
    /// True from the moment a spawn caller has reserved this session id (under
    /// the `PtyState::sessions` lock) to the moment the spawn either succeeds
    /// or fails. While `is_spawning` is true, `is_session_spawn_active` returns
    /// `true` and concurrent spawn attempts skip — closing the TOCTOU race
    /// where two callers both passed the "writer/master is None" check while
    /// the slow ConPTY initialization was in flight on Windows.
    pub is_spawning: bool,
    /// Set when this session is owned by the `codemux pty-daemon` process
    /// instead of the in-process portable-pty path. The PID stored in
    /// `child_pid` belongs to a process the daemon spawned — NOT a direct
    /// child of the Tauri app. Implications:
    ///
    /// - `terminate_pty_session` must NOT call `killpg` for these sessions
    ///   (we don't own the process group; the daemon does).
    /// - On window close, persistent sessions detach from the daemon
    ///   instead of getting torn down.
    /// - Drop is a no-op for persistent sessions; the daemon outlives us.
    pub persistent: bool,
    /// The daemon client this session was spawned through. Set on every
    /// daemon-backed spawn (local or tunneled-remote).
    ///
    /// Resize and close MUST route through this client, not through
    /// `ensure_daemon()` directly — `ensure_daemon` always returns the
    /// LOCAL daemon, which doesn't know about sessions that live on a
    /// remote host's daemon. Pre-fix, every resize/close on a remote
    /// session hit `unknown session` because the command went to the
    /// wrong daemon.
    ///
    /// `#[cfg(unix)]` because the `pty_daemon` module is Unix-only
    /// (the daemon talks Unix sockets, the cloud-push feature is
    /// Unix-only). Keeping the field absent on Windows avoids a
    /// stub type and matches how the rest of the daemon plumbing
    /// gates itself.
    #[cfg(unix)]
    pub daemon_client: Option<Arc<crate::pty_daemon::PtyDaemonClient>>,
}

impl SessionRuntime {
    pub(crate) fn new(session_id: &str) -> Self {
        Self {
            writer: None,
            master: None,
            output_subscribers: Vec::new(),
            next_output_generation: 0,
            pending_output: VecDeque::new(),
            pending_output_bytes: 0,
            dropped_chunks: 0,
            flow_paused: Arc::new(AtomicBool::new(false)),
            last_status: TerminalStatusPayload {
                session_id: session_id.to_string(),
                state: TerminalLifecycleState::Starting,
                message: Some("Starting shell...".into()),
                exit_code: None,
            },
            child_pid: None,
            skip_preset_launch: false,
            resume_command: None,
            is_spawning: false,
            persistent: false,
            #[cfg(unix)]
            daemon_client: None,
        }
    }
}

/// Safety net: if a `SessionRuntime` is dropped with a live `child_pid`, the
/// normal close path was skipped (panic unwind, future refactor forgetting to
/// kill, or a test tearing down fixtures). We kill the process group as a last
/// resort and shout in stderr so the bug is visible.
///
/// The normal close path (`close_terminal_session`) clears `child_pid` to
/// `None` before dropping the runtime, so this impl is silent on the happy
/// path. Any warning printed here is a real bug worth investigating.
impl Drop for SessionRuntime {
    fn drop(&mut self) {
        if let Some(pid) = self.child_pid.take() {
            // Persistent sessions are owned by `codemux pty-daemon`, not by
            // this process. We must NOT kill them on drop — that defeats
            // the whole point of running them detached. The daemon will
            // tear them down via its own `Close` request when the user
            // explicitly closes the pane.
            if self.persistent {
                return;
            }
            eprintln!(
                "[codemux::terminal] SessionRuntime dropped with live child_pid={pid} — \
                 normal close path was skipped. Killing process group as last resort."
            );
            kill_session_tree(pid);
        }
    }
}

#[derive(Default)]
pub struct PtyState {
    pub sessions: Arc<Mutex<HashMap<String, SessionRuntime>>>,
}

impl PtyState {
    /// Returns a snapshot of session_id -> child PID for all active sessions.
    pub fn get_session_pids(&self) -> HashMap<String, u32> {
        let guard = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .filter_map(|(id, runtime)| runtime.child_pid.map(|pid| (id.clone(), pid)))
            .collect()
    }
}

fn remove_session_runtime(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> Option<SessionRuntime> {
    sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id)
}

/// Atomically reserve a session id for a spawn attempt.
///
/// Returns `true` if the caller now owns the spawn for this session and must
/// proceed (and eventually call `clear_spawn_reservation` on every exit path).
/// Returns `false` if another caller is already spawning, the session is
/// already running (`writer`/`master` populated), or this call lost the race.
///
/// This closes the TOCTOU window between the historical
/// "is the session already running?" check and the moment the spawned PTY
/// handles get inserted into the runtime. On Linux the window was tens of
/// microseconds and never observed in practice; on Windows ConPTY init takes
/// hundreds of milliseconds and the window was wide enough that startup
/// session restore would race with user-driven workspace creation and double-
/// spawn the same session id, leaking children.
fn try_reserve_session_spawn(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> bool {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = guard
        .entry(session_id.to_string())
        .or_insert_with(|| SessionRuntime::new(session_id));
    if runtime.writer.is_some() || runtime.master.is_some() || runtime.is_spawning {
        return false;
    }
    runtime.is_spawning = true;
    true
}

/// Check whether a spawn attempt is in flight or already complete for `session_id`.
///
/// Used by tests and by `spawn_missing_ptys_for_workspace` to skip sessions
/// whose runtime is already populated. Acquires the `PtyState::sessions`
/// mutex; do not hold any other lock across this call.
#[allow(dead_code)]
fn is_session_spawn_active(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> bool {
    let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get(session_id)
        .map(|rt| rt.writer.is_some() || rt.master.is_some() || rt.is_spawning)
        .unwrap_or(false)
}

fn map_status_state(state: &TerminalLifecycleState) -> TerminalSessionState {
    match state {
        TerminalLifecycleState::Starting => TerminalSessionState::Starting,
        // Migrating is a transient start with a host-specific message; the
        // persisted session state has no separate variant for it, so collapse
        // it to Starting. The `migrating` discriminant only matters on the
        // `terminal-status` event the frontend overlay reads.
        TerminalLifecycleState::Migrating => TerminalSessionState::Starting,
        TerminalLifecycleState::Ready => TerminalSessionState::Ready,
        TerminalLifecycleState::Exited => TerminalSessionState::Exited,
        TerminalLifecycleState::Failed => TerminalSessionState::Failed,
    }
}

/// Pick the best CWD for a restored PTY session.  Prefers the scrollback
/// metadata's `working_directory` when it is non-empty and still exists on
/// disk, so that CWD-scoped tools (`claude --resume`) find their sessions.
/// Falls back to `fallback` otherwise.
fn resolve_session_cwd(scrollback_working_dir: &str, fallback: &str) -> String {
    if !scrollback_working_dir.is_empty()
        && std::path::Path::new(scrollback_working_dir).is_dir()
    {
        scrollback_working_dir.to_string()
    } else {
        fallback.to_string()
    }
}

fn build_resume_launch_command(original_command: &str, resume_args: &str) -> String {
    let original = original_command.trim();
    let args = resume_args.trim();

    if original.is_empty() {
        return args.to_string();
    }
    if args.is_empty() {
        return original.to_string();
    }

    format!("{original} {args}")
}

fn resolve_resume_command(
    snapshot: &crate::state::AppStateSnapshot,
    meta: &crate::scrollback::ScrollbackMeta,
    adapter_state: &crate::session_adapters::AdapterState,
) -> Option<String> {
    let original_command = meta.original_command.clone().or_else(|| {
        snapshot
            .terminal_sessions
            .iter()
            .find(|session| session.session_id.0 == meta.session_id)
            .and_then(|session| session.original_command.clone())
    })?;

    let adapter_id = meta
        .adapter_id
        .clone()
        .or_else(|| adapter_state.match_adapter_id_for_command(&original_command))?;

    let adapter_match = adapter_state.get_adapter_match(&adapter_id, &meta.adapter_captures)?;
    let resume_args = adapter_match.resume_args?;

    Some(build_resume_launch_command(&original_command, &resume_args))
}

fn with_session_runtime<T>(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    default: impl FnOnce() -> SessionRuntime,
    f: impl FnOnce(&mut SessionRuntime) -> T,
) -> T {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = guard.entry(session_id.to_string()).or_insert_with(default);
    f(runtime)
}

fn with_existing_session_runtime<T>(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    f: impl FnOnce(&mut SessionRuntime) -> T,
) -> Option<T> {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_mut(session_id).map(f)
}

/// Emit `Exited` for `session_id` ONLY if the runtime's `daemon_client`
/// still points at `client` (Arc::ptr_eq). Otherwise we're a stale
/// read task whose session was already replaced by a fresh spawn —
/// emitting Exited here would overwrite the new spawn's Ready and
/// leave the user with a permanent "Shell ended" overlay on a session
/// that's actually alive.
///
/// Called from the daemon-backed read tasks (agent + shell) when
/// their mpsc returns None. The race is real and easy to trigger:
/// push → `terminate_pty_session_keep_channel` tells the daemon to
/// close the old session (background task), spawn_missing_ptys
/// respawns and emits Ready, then the old session's close finally
/// flushes its rx → read task ends → without this check, we'd emit
/// a stale Exited and clobber Ready.
/// Pure-function core of the "is this read task still relevant" check.
/// Extracted from `emit_exited_if_client_owner` so the Arc-pointer
/// comparison logic can be unit-tested without needing a real
/// `tauri::AppHandle` or `PtyDaemonClient`.
///
/// Returns:
/// - `true` if the runtime exists AND its `daemon_client` is the
///   same Arc allocation as `client` (pointer-equal). The caller is
///   the current owner and should emit.
/// - `false` if the runtime is missing, its `daemon_client` is None,
///   or it points to a different Arc (the caller is a stale read
///   task from a previous spawn).
#[cfg(unix)]
pub(crate) fn is_runtime_owned_by_client(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    client: &Arc<crate::pty_daemon::PtyDaemonClient>,
) -> bool {
    with_existing_session_runtime(sessions, session_id, |rt| {
        rt.daemon_client
            .as_ref()
            .map(|c| Arc::ptr_eq(c, client))
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

#[cfg(unix)]
pub(crate) fn emit_exited_if_client_owner<R: Runtime>(
    app: &AppHandle<R>,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    client: &Arc<crate::pty_daemon::PtyDaemonClient>,
    message: &str,
) {
    if !is_runtime_owned_by_client(sessions, session_id, client) {
        eprintln!(
            "[codemux::terminal] skip Exited for {session_id}: stale read task \
             (runtime daemon_client is None or differs — session was respawned)"
        );
        return;
    }
    emit_terminal_status(
        app,
        sessions,
        TerminalStatusPayload {
            session_id: session_id.to_string(),
            state: TerminalLifecycleState::Exited,
            message: Some(message.to_string()),
            exit_code: None,
        },
    );
}

fn emit_terminal_status<R: Runtime>(
    app: &AppHandle<R>,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    payload: TerminalStatusPayload,
) {
    let app_state: State<'_, AppStateStore> = app.state();
    let session_changed = app_state.update_terminal_session_status(
        &payload.session_id,
        map_status_state(&payload.state),
        payload.message.clone(),
        payload.exit_code,
    );

    // For terminal states (Failed/Exited), don't create a new SessionRuntime
    // entry — the session is done. For Starting/Ready, use or_insert to ensure
    // the entry exists.
    match payload.state {
        TerminalLifecycleState::Failed | TerminalLifecycleState::Exited => {
            with_existing_session_runtime(sessions, &payload.session_id, |runtime| {
                runtime.last_status = payload.clone();
            });
        }
        _ => {
            with_session_runtime(
                sessions,
                &payload.session_id,
                || SessionRuntime::new(&payload.session_id),
                |runtime| {
                    runtime.last_status = payload.clone();
                },
            );
        }
    }

    // On terminal exit, clear transient pane status (working/permission → idle).
    // The `terminal-status` event below only drives the live terminal overlay,
    // so the sidebar dot heals from the `PaneStatus` delta instead.
    if matches!(
        payload.state,
        TerminalLifecycleState::Exited | TerminalLifecycleState::Failed
    ) {
        if let Some(delta) =
            app_state.clear_transient_pane_status_by_session_delta(&payload.session_id)
        {
            state::emit_app_state_delta(app, delta);
        }
    }

    // The session's state/last_message/exit_code live in the app state too, and
    // no delta domain covers them — coalesced so a burst of transitions is one
    // snapshot.
    if session_changed {
        state::schedule_emit_app_state(app);
    }

    if let Err(error) = app.emit("terminal-status", payload) {
        eprintln!("[codemux::terminal] Failed to emit terminal status: {error}");
    }
}

/// Recompute a session's derived `flow_paused` back-pressure flag from its
/// current subscriber set and store it into the shared atomic the in-process
/// reader polls. Returns the new effective value.
///
/// Fan-out flow-control policy: a single PTY reader feeds N live subscribers
/// (the desktop window plus any mirror consumers). It must only stop draining
/// the PTY — applying real back-pressure to the child — when EVERY current
/// subscriber has asked to pause; one caught-up subscriber keeps bytes flowing
/// for all of them. With zero subscribers the reader keeps running and its
/// output accumulates in the bounded replay ring. A subscriber's pause request
/// is released automatically when it detaches, because it stops counting toward
/// the "all paused" verdict.
///
/// Pure w.r.t. side channels (no daemon I/O, no task spawn) so it can run under
/// the sessions lock and be unit-tested directly; the daemon forward lives in
/// `apply_flow_control`.
fn recompute_flow_paused(runtime: &SessionRuntime) -> bool {
    let effective = !runtime.output_subscribers.is_empty()
        && runtime.output_subscribers.iter().all(|s| s.flow_paused);
    runtime.flow_paused.store(effective, Ordering::Relaxed);
    effective
}

fn queue_or_send_output(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    chunk: Vec<u8>,
) {
    let subscribers_to_send = with_session_runtime(
        sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            // Maintain `pending_output_bytes` in lockstep with the deque so
            // the eviction loop below can cap by total bytes without an O(n)
            // re-sum. `saturating_add` defends against the pathological case
            // of cumulative bytes wrapping `usize::MAX` — at the production
            // 256 MiB cap this is theoretical, but it's free insurance.
            runtime.pending_output_bytes = runtime
                .pending_output_bytes
                .saturating_add(chunk.len());
            runtime.pending_output.push_back(chunk.clone());
            while runtime.pending_output_bytes > OUTPUT_BUFFER_BYTE_LIMIT {
                let Some(evicted) = runtime.pending_output.pop_front() else {
                    // Defensive: the byte counter and the deque got out of
                    // sync somehow (would indicate a bug in this function or
                    // a future caller mutating the deque directly). Clamp to
                    // zero to break the loop instead of spinning, and let
                    // the next push restart accounting from a clean slate.
                    runtime.pending_output_bytes = 0;
                    break;
                };
                runtime.pending_output_bytes = runtime
                    .pending_output_bytes
                    .saturating_sub(evicted.len());
                if runtime.output_subscribers.is_empty() {
                    let dropped = runtime.dropped_chunks.saturating_add(1);
                    runtime.dropped_chunks = dropped;
                    // Log on the first drop and then on each power-of-two boundary
                    // so a chatty regression doesn't spam stderr but is still
                    // discoverable. A single line is enough — the count is the
                    // signal; the rest is in the bug.
                    if dropped == 1 || dropped.is_power_of_two() {
                        eprintln!(
                            "[codemux::terminal] session={session_id} dropped pending_output \
                             chunk (cumulative={dropped}). Indicates no consumer was \
                             attached when the buffer overflowed."
                        );
                    }
                }
            }

            // Snapshot (generation, channel) for every subscriber so the sends
            // happen OUTSIDE the lock. Cloning a `Channel` is cheap (it's
            // internally ref-counted).
            runtime
                .output_subscribers
                .iter()
                .map(|s| (s.generation, s.channel.clone()))
                .collect::<Vec<_>>()
        },
    );

    if subscribers_to_send.is_empty() {
        return;
    }

    // Tauri IPC delivery can be slower than the in-memory bookkeeping above,
    // especially when the WebView is busy parsing terminal output. Never hold
    // the global sessions mutex while sending, otherwise active keystrokes
    // (`write_to_pty`) block behind unrelated PTY output from other sessions.
    //
    // Fan out to every subscriber; collect the generations whose send failed so
    // they can be pruned. A failure means that consumer's channel is dead (the
    // webview navigated away, a mirror client dropped) — evicting it by
    // generation can never touch a newer subscriber that attached concurrently.
    let mut failed: Vec<u64> = Vec::new();
    for (generation, channel) in subscribers_to_send {
        if let Err(error) = channel.send(chunk.clone()) {
            eprintln!("[codemux::terminal] Failed to send terminal output: {error}");
            failed.push(generation);
        }
    }

    if !failed.is_empty() {
        with_existing_session_runtime(sessions, session_id, |runtime| {
            runtime
                .output_subscribers
                .retain(|s| !failed.contains(&s.generation));
            // Losing a subscriber can flip the "all paused" verdict.
            recompute_flow_paused(runtime);
        });
        // Keep a daemon-backed session's flow gate in step with the pruned set.
        // For daemon sessions this runs on the daemon read task (a tokio
        // context); for in-process sessions there is no daemon client, so it is
        // a no-op that never spawns. Cheap: only reached when a send failed.
        forward_flow_control_to_daemon(sessions, session_id);
    }
}

/// Flush accumulated PTY output as a single batched chunk.
/// Resets `batch` and `last_flush` after flushing.
fn flush_pty_batch(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    batch: &mut Vec<u8>,
    last_flush: &mut Instant,
) {
    if !batch.is_empty() {
        let chunk = std::mem::take(batch);
        queue_or_send_output(sessions, session_id, chunk);
        *last_flush = Instant::now();
    }
}

/// Poll a file descriptor for read-readiness with a timeout.
/// Returns true if data is available (POLLIN) or the fd is dead (POLLHUP/POLLERR),
/// meaning the caller should attempt read() to get data or discover the error.
/// Returns false on timeout (no events within timeout_ms).
/// Retries on EINTR (signal interruption).
#[cfg(unix)]
fn poll_read_ready(fd: RawFd, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // Safety: pfd is a valid pollfd struct on the stack.
        let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if ret < 0 {
            let errno = std::io::Error::last_os_error();
            if errno.raw_os_error() == Some(libc::EINTR) {
                continue; // Interrupted by signal, retry
            }
            // Other poll error (e.g. EBADF) — return true so caller
            // hits read() which will surface the actual error.
            return true;
        }
        return ret > 0 && (pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0;
    }
}

/// Batched PTY reader loop. Uses poll() to guarantee pending data is flushed
/// within PTY_BATCH_INTERVAL even when no more output arrives.
///
/// `poll_fd` is the raw fd for the PTY master (used for poll readiness checks).
/// `reader` is the cloned reader (a dup of the same fd, used for actual reads).
/// `flow_paused` is the session's back-pressure flag: while set, the loop stops
/// draining the master fd so the kernel PTY buffer fills and the child blocks on
/// `write()` (real back-pressure). It is polled (not condvar-waited) so a missed
/// wake can't wedge the loop, with a `FLOW_MAX_PARK` backstop that force-resumes
/// a never-resumed pause.
/// `pre_read_hook` is called with each read's raw data before it's batched,
/// allowing per-read processing (e.g. comm log in the agent loop).
#[cfg(unix)]
fn batched_reader_loop(
    reader: &mut dyn Read,
    poll_fd: PollFd,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    flow_paused: &Arc<AtomicBool>,
    mut pre_read_hook: impl FnMut(&[u8]),
) {
    let mut buf = [0u8; 4096];
    let mut batch: Vec<u8> = Vec::with_capacity(PTY_BATCH_SIZE);
    let mut last_flush = Instant::now();
    let timeout_ms = PTY_BATCH_INTERVAL.as_millis() as i32;
    // How long we've been continuously parked, for the `FLOW_MAX_PARK` backstop.
    let mut paused_since: Option<Instant> = None;

    loop {
        // ── Flow-control gate ──
        // While the attached renderer signals it's behind (its xterm write
        // queue crossed the HIGH watermark), stop reading. The child's PTY
        // writes then block once the kernel buffer fills — back-pressure to
        // the producer instead of an unbounded queue / dropped scrollback.
        // A first-flush keeps the renderer from being starved of bytes it has
        // already accepted before we park. The `FLOW_MAX_PARK` backstop
        // force-resumes if the renderer wedged and never sent the resume.
        if flow_paused.load(Ordering::Relaxed) {
            flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            while flow_paused.load(Ordering::Relaxed) {
                let since = *paused_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= FLOW_MAX_PARK {
                    eprintln!(
                        "[codemux::terminal] session {session_id} flow-paused for >{}s — \
                         force-resuming (renderer wedged or resume frame lost)",
                        FLOW_MAX_PARK.as_secs()
                    );
                    flow_paused.store(false, Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(FLOW_PARK_POLL);
            }
            paused_since = None;
        }

        // If the batch has data, use a timed poll so we flush on timeout.
        // If the batch is empty, block indefinitely until data arrives.
        let poll_timeout = if batch.is_empty() { -1 } else { timeout_ms };

        if !poll_read_ready(poll_fd, poll_timeout) {
            // Timeout with no new data — flush pending batch.
            flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            continue;
        }

        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                pre_read_hook(data);
                batch.extend_from_slice(data);
                if batch.len() >= PTY_BATCH_SIZE || last_flush.elapsed() >= PTY_BATCH_INTERVAL {
                    flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                }
            }
            Ok(_) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                break;
            }
            Err(error) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                eprintln!("[codemux::terminal] PTY read error: {error}");
                break;
            }
        }
    }
}

#[cfg(windows)]
fn batched_reader_loop(
    reader: &mut dyn Read,
    _poll_fd: PollFd,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    flow_paused: &Arc<AtomicBool>,
    mut pre_read_hook: impl FnMut(&[u8]),
) {
    // Windows placeholder: Windows pipes are blocking with no poll() equivalent,
    // so we use a simple blocking read loop that flushes every read. No 16ms
    // batching yet — that needs a Tokio-based rewrite.
    let mut buf = [0u8; 4096];
    let mut batch: Vec<u8> = Vec::with_capacity(PTY_BATCH_SIZE);
    let mut last_flush = Instant::now();
    // How long we've been continuously parked, for the `FLOW_MAX_PARK` backstop.
    let mut paused_since: Option<Instant> = None;
    loop {
        // Flow-control gate (see the Unix variant for the rationale). While
        // paused we stop reading; the ConPTY pipe buffer fills and the child's
        // next write blocks — back-pressure to the producer. `FLOW_MAX_PARK`
        // force-resumes a never-resumed pause.
        if flow_paused.load(Ordering::Relaxed) {
            flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            while flow_paused.load(Ordering::Relaxed) {
                let since = *paused_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= FLOW_MAX_PARK {
                    eprintln!(
                        "[codemux::terminal] session {session_id} flow-paused for >{}s — \
                         force-resuming (renderer wedged or resume frame lost)",
                        FLOW_MAX_PARK.as_secs()
                    );
                    flow_paused.store(false, Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(FLOW_PARK_POLL);
            }
            paused_since = None;
        }

        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                pre_read_hook(data);
                batch.extend_from_slice(data);
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            }
            Ok(_) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                break;
            }
            Err(error) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                eprintln!("[codemux::terminal] PTY read error: {error}");
                break;
            }
        }
    }
}

/// OS-specific PATH entry separator: `;` on Windows, `:` on Unix.
///
/// Exposed as a constant-returning function (not a `const`) so tests can
/// drive the PATH-prepend logic with an explicit separator without caring
/// about the host OS. Production callers get the right value via
/// `path_separator()` which resolves at compile time.
fn path_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

/// Prepend `shim_dir` to an existing `PATH` string using the OS-specific
/// separator. Extracted from the inline logic in `spawn_pty_for_shell`
/// so it's unit-testable without setting up a real PTY + env.
///
/// Semantics:
///   - Empty `current_path` → return `shim_dir` unchanged (no trailing sep).
///   - Non-empty → `{shim_dir}{sep}{current_path}`.
///   - Does NOT deduplicate. If `shim_dir` is already in `current_path`,
///     it will appear twice. That's intentional — the prepend ensures the
///     codemux shim wins even if another invocation of this function
///     already added it once.
fn prepend_shim_to_path(shim_dir: &str, current_path: &str) -> String {
    if current_path.is_empty() {
        shim_dir.to_string()
    } else {
        format!("{shim_dir}{}{current_path}", path_separator())
    }
}

/// Build the final PATH value passed to a PTY child process.
///
/// Always prepends the codemux shim dir so `codemux …` CLI commands work
/// inside the user's terminal. On Windows, **also** prepends
/// `%USERPROFILE%\.local\bin` — the install location used by the Claude
/// Code native installer (`irm https://claude.ai/install.ps1 | iex`) and
/// every other per-user CLI installer that targets Windows with a POSIX
/// layout. Codemux's own environment block may miss a freshly-added
/// `.local\bin` because Windows PATH changes made by installers broadcast
/// via `WM_SETTINGCHANGE`, which a running process may not pick up. Adding
/// the directory here ensures that the shell spawned inside the PTY can
/// find binaries users just installed, regardless of whether Codemux
/// itself saw the PATH update.
///
/// On Unix this is a pass-through to `prepend_shim_to_path` — Unix
/// installers update `~/.bashrc` / `~/.zshrc`, so the user's login shell
/// already picks up `.local/bin` via the `PATH` they export from their
/// rc file. Replicating the prepend on Unix would be redundant at best.
fn build_child_path(shim_dir: &str, current_path: &str) -> String {
    let base = prepend_shim_to_path(shim_dir, current_path);
    #[cfg(windows)]
    {
        if let Some(local_bin) = windows_user_local_bin() {
            return prepend_shim_to_path(&local_bin, &base);
        }
    }
    base
}

/// Returns `%USERPROFILE%\.local\bin` as a string if `USERPROFILE` is set.
/// Does NOT verify that the directory exists — the caller only needs the
/// path string to stuff into PATH, and a non-existent entry in PATH is
/// harmless (Windows PATH parsing silently skips missing dirs).
#[cfg(windows)]
fn windows_user_local_bin() -> Option<String> {
    std::env::var_os("USERPROFILE").map(|p| {
        std::path::Path::new(&p)
            .join(".local")
            .join("bin")
            .display()
            .to_string()
    })
}

#[cfg(unix)]
fn default_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/bash".to_string())
}

#[cfg(windows)]
fn default_shell() -> String {
    resolve_windows_shell(
        |cmd| which::which(cmd).ok().map(|p| p.display().to_string()),
        env::var("COMSPEC").ok(),
    )
}

/// Resolve the default Windows shell from a PATH resolver and a COMSPEC
/// value. Pure function, extracted from `default_shell()` so the fallback
/// chain can be unit-tested on any platform (Linux CI compiles it under
/// `cfg(test)`).
///
/// Fallback order:
///   1. `pwsh.exe` — PowerShell 7+, the modern cross-platform shell.
///      Only present if the user installed it explicitly, but preferred
///      when available because it ships current language features and
///      matches the pwsh most users on macOS/Linux already know.
///   2. `powershell.exe` — Windows PowerShell 5.1, pre-installed on every
///      supported Windows version (Server 2016+ and Windows 10+). This is
///      the safe default that should almost always succeed.
///   3. `COMSPEC` — typically `C:\Windows\System32\cmd.exe`. Honored so
///      corporate images that point COMSPEC at a custom cmd wrapper still
///      work on the cmd.exe path.
///   4. Literal `"cmd.exe"` — last-resort relative name, resolved by
///      `CreateProcessW` against `PATH` at spawn time.
#[cfg(any(windows, test))]
fn resolve_windows_shell<F>(resolve: F, comspec: Option<String>) -> String
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(path) = resolve("pwsh") {
        return path;
    }
    if let Some(path) = resolve("powershell") {
        return path;
    }
    comspec
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

#[cfg(unix)]
fn ensure_cli_shims() -> Option<(String, String)> {
    let current_exe = std::env::current_exe().ok()?;
    let current_exe = current_exe.display().to_string();
    let shim_dir = std::env::temp_dir().join(format!("{}-cli-shims", crate::APP_DIR_NAME));
    std::fs::create_dir_all(&shim_dir).ok()?;

    let shim_path = shim_dir.join("codemux");
    let script = format!(
        "#!/bin/sh\nexec \"{}\" \"$@\"\n",
        current_exe.replace('"', "\\\"")
    );

    // Skip the rewrite (and the chmod) if the shim already matches what we
    // would write. This avoids per-spawn disk churn when a workspace
    // hydrates many sessions at once.
    let needs_write = match std::fs::read_to_string(&shim_path) {
        Ok(existing) => existing != script,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&shim_path, &script).ok()?;
        let mut perms = std::fs::metadata(&shim_path).ok()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).ok()?;
    }

    Some((shim_dir.display().to_string(), current_exe))
}

#[cfg(windows)]
fn ensure_cli_shims() -> Option<(String, String)> {
    let current_exe = std::env::current_exe().ok()?;
    let current_exe = current_exe.display().to_string();
    let shim_dir = std::env::temp_dir().join(format!("{}-cli-shims", crate::APP_DIR_NAME));
    std::fs::create_dir_all(&shim_dir).ok()?;

    let shim_path = shim_dir.join("codemux.bat");
    let script = format!(
        "@echo off\r\n\"{}\" %*\r\n",
        current_exe.replace('"', "\\\"")
    );

    // Skip the rewrite if the shim already matches what we would write.
    // Avoids per-spawn disk churn on Windows where every session hydration
    // would otherwise touch %TEMP%.
    let needs_write = match std::fs::read_to_string(&shim_path) {
        Ok(existing) => existing != script,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&shim_path, &script).ok()?;
    }

    Some((shim_dir.display().to_string(), current_exe))
}

#[cfg(not(any(unix, windows)))]
fn ensure_cli_shims() -> Option<(String, String)> {
    None
}

/// Find the workspace that owns a terminal session by walking each workspace's
/// pane tree.  Returns `None` for orphaned sessions (no workspace references them).
fn find_owning_workspace<'a>(
    snapshot: &'a crate::state::AppStateSnapshot,
    session_id: &str,
) -> Option<&'a crate::state::WorkspaceSnapshot> {
    snapshot.workspaces.iter().find(|ws| {
        ws.surfaces
            .iter()
            .any(|s| crate::state::find_terminal_pane_id(&s.root, session_id).is_some())
    })
}

fn session_working_dir(app_state: &State<'_, AppStateStore>, session_id: &str) -> String {
    app_state
        .snapshot()
        .terminal_sessions
        .into_iter()
        .find(|session| session.session_id.0 == session_id)
        .map(|session| session.cwd)
        .unwrap_or_else(|| current_project_root().display().to_string())
}

/// Kill a PTY child and its entire process group with a single SIGKILL.
///
/// **Why no SIGTERM grace period.** A previous version did SIGTERM → 200ms
/// sleep → SIGKILL. That grace window is exactly the adversarial case for
/// PID recycling: the shell handles SIGTERM and exits in ~50ms, the waiter
/// thread reaps the zombie, the kernel recycles the PID to an unrelated
/// process, then our SIGKILL lands on the wrong process group. Going
/// straight to SIGKILL collapses the window between `remove_session_runtime`
/// and the signal call to microseconds, eliminates the per-session 200ms
/// block on the Tauri worker thread, and is the correct semantic for a
/// "close this pane" user action where no flush is needed.
///
/// Does **not** call `waitpid` — the waiter thread at the bottom of
/// `spawn_pty_for_session` owns the `Box<dyn Child>`
/// and is blocked in `child.wait()`. When SIGKILL lands, that `wait()`
/// returns and the waiter thread reaps the child normally. Reaping from
/// two places would race and produce `ECHILD`.
///
/// Because `portable-pty`'s Unix spawn path calls `libc::setsid()` in its
/// `pre_exec` hook, every PTY child is a session leader whose PGID equals
/// its PID. That means `killpg(pid, ...)` sends the signal to the shell
/// **and** everything the shell spawned (Claude CLI, MCP servers,
/// rust-analyzer, ...) as long as those children have not themselves called
/// `setsid` to detach into a new process group. This handles the common
/// leak pattern — the setsid-detach limitation is documented in
/// `test_killpg_does_not_reach_setsid_detached_grandchild` and tracked as a
/// `/proc`-walking fallback follow-up if we ever observe it in practice.
///
/// `ESRCH` is treated as success: it means the process was already gone
/// (likely reaped by the waiter thread first).
#[cfg(unix)]
fn kill_session_tree(pid: u32) {
    if pid <= 1 {
        // killpg(0, ...) signals our own process group; killpg(1, ...)
        // targets init. Neither is ever correct for a PTY child we
        // spawned. A pid <= 1 stored in SessionRuntime is a bug upstream.
        eprintln!(
            "[codemux::terminal] kill_session_tree refusing pid={pid} (<=1 is never a PTY child)"
        );
        return;
    }
    let pid_i32 = pid as i32;

    // SAFETY: `killpg` is an async-signal-safe POSIX call that takes a
    // plain integer PGID. There is no memory aliasing or lifetime concern.
    //
    // PID recycling is theoretically possible between `remove_session_runtime`
    // (which dropped the `SessionRuntime` in `terminate_pty_session`) and
    // this call, but the window is microseconds (no sleep). On a system
    // with `pid_max` >= 32768, recycling in this window requires ~32k
    // process spawns between our two instructions, which is not realistic.
    // See the doc comment above for the history of why this is not a
    // SIGTERM-then-SIGKILL dance.
    let ret = unsafe { libc::killpg(pid_i32, libc::SIGKILL) };
    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("[codemux::terminal] killpg({pid}, SIGKILL) failed: {errno}");
        }
        // ESRCH means the waiter thread already reaped — success.
    }
}

/// Windows stub. Codemux is Linux-primary; Windows is tracked as a follow-up
/// in the windows-support plan. Until we have a Job Object implementation,
/// this path is a no-op with a warning.
#[cfg(not(unix))]
fn kill_session_tree(pid: u32) {
    eprintln!(
        "[codemux::terminal] kill_session_tree({pid}) is a no-op on non-Unix; \
         PTY children will leak until app exit (TODO: Windows Job Object support)"
    );
}

/// RAII guard that kills and waits on a PTY child process if not explicitly
/// disarmed. Prevents zombie processes when spawn_pty_for_session or
/// terminal spawn encounters an error after the child has been spawned.
struct ChildGuard {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl ChildGuard {
    fn new(child: Box<dyn portable_pty::Child + Send + Sync>) -> Self {
        Self { child: Some(child) }
    }

    /// Take ownership of the child, disarming the guard.
    /// Call this once the child is handed off to the waiter thread.
    fn disarm(mut self) -> Box<dyn portable_pty::Child + Send + Sync> {
        self.child.take().expect("ChildGuard already disarmed")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            eprintln!("[codemux::terminal] Killing orphaned child process");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Remove the WebKitGTK renderer transport vars from a child's environment.
///
/// Those vars configure how *this* process hands composited frames to the
/// compositor; they are meaningless to a shell and actively harmful to any
/// GTK/WebKit app launched from one (including another Codemux, which would
/// read the inherited values as a user override and drop to CPU rendering).
/// `GDK_BACKEND` and the `CODEMUX*` vars are deliberately left in place —
/// children are expected to inherit those.
pub(crate) fn strip_renderer_env(cmd: &mut CommandBuilder) {
    for key in crate::webview_tuning::RENDERER_ENV_VARS {
        cmd.env_remove(key);
    }
}

/// Build worktree environment variables and dynamic agent context for a PTY session.
/// Used by terminal sessions and agent-chat sidecars so workspace-level
/// environment variables stay consistent.
///
/// Exposed `pub(crate)` so the agent-chat command layer can reuse the exact
/// same workspace-level vars when overlaying env onto a chat sidecar
/// (see `crate::commands::agent_chat::workspace_env_overlay`), keeping the
/// terminal and chat surfaces in lockstep.
pub(crate) fn workspace_pty_env(ws: &crate::state::WorkspaceSnapshot) -> Vec<(String, String)> {
    let root = ws.project_root.clone().unwrap_or_else(|| {
        crate::scripts::resolve_root_path(std::path::Path::new(&ws.cwd))
            .to_string_lossy()
            .to_string()
    });
    let port = crate::scripts::allocate_workspace_port(&ws.workspace_id.0);

    let mut vars: Vec<(String, String)> = crate::scripts::script_env(
        std::path::Path::new(&ws.cwd),
        std::path::Path::new(&root),
        ws.git_branch.as_deref(),
        Some(port),
    )
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    vars.push(("CODEMUX_WORKSPACE_NAME".to_string(), ws.title.clone()));
    vars.push((
        "CODEMUX_AGENT_CONTEXT".to_string(),
        crate::agent_context::build_agent_context(
            Some(&ws.title),
            ws.worktree_path.as_deref(),
            ws.git_branch.as_deref(),
            Some(&root),
        ),
    ));

    vars
}

pub fn spawn_pty_for_session<R: Runtime>(app: AppHandle<R>, session_id: String) {
    // Persistent path: every shell goes through the long-lived
    // `codemux pty-daemon` so closing the app doesn't kill it. The
    // agent commands the user later types into the shell inherit the
    // shell's lifetime, so this is what makes "close laptop, agent
    // keeps running" work for the normal preset-driven flow (which
    // spawns a shell first and writes the agent command into it).
    //
    // Fallback is silent and total: any daemon error — circuit breaker
    // open, daemon binary missing, socket race, version mismatch,
    // platform without IPC support — drops to the in-process spawn so
    // the user always gets a working terminal.
    #[cfg(unix)]
    {
        if daemon_path_viable() {
            let app_clone = app.clone();
            let session_id_clone = session_id.clone();
            tauri::async_runtime::spawn(async move {
                match daemon_backed::spawn_pty_for_session_via_daemon(
                    app_clone.clone(),
                    session_id_clone.clone(),
                )
                .await
                {
                    Ok(()) => {}
                    Err(error) => {
                        // Critical: do NOT fall back to in-process
                        // spawn for REMOTE workspaces. A remote
                        // workspace's host_id says "this lives on
                        // pandora," and silently spawning a local
                        // shell would lie about where the user's
                        // sessions are running — leading to the
                        // exact "Cloud icon but local pwd" bug we
                        // shipped a fix for in the prior commit.
                        // Surface the failure as Failed status; the
                        // UI shows the error and the user can pull
                        // back / retry. Local workspaces (host_id
                        // == None) still get the in-process
                        // fallback because for them it's correct.
                        // "Already reserved" is benign — another spawn
                        // task for the same session id is already in
                        // flight (sibling pane spawn race, workspace
                        // re-activation, etc.). Silently no-op instead
                        // of clobbering the in-flight spawn with a
                        // Failed status that the user sees as a
                        // "Reconnecting" / "Couldn't reach the host"
                        // popup over a session that's actually fine.
                        if error.contains("already reserved") {
                            eprintln!(
                                "[codemux::terminal] suppressing benign 'already reserved' \
                                 spawn-retry for session {session_id_clone} \
                                 (sibling spawn in flight; no UI change)"
                            );
                            return;
                        }
                        let app_for_check = app_clone.clone();
                        let is_remote_workspace = is_remote_workspace_for_session(
                            &app_for_check,
                            &session_id_clone,
                        );
                        if is_remote_workspace {
                            eprintln!(
                                "[codemux::terminal] remote-shell spawn failed for session \
                                 {session_id_clone}: {error}; NOT falling back to local"
                            );
                            // Emit Failed so the terminal pane
                            // surfaces a useful message instead of
                            // hanging on "Starting…" forever.
                            let pty_state: State<'_, PtyState> =
                                app_clone.state();
                            emit_terminal_status(
                                &app_clone,
                                &pty_state.sessions,
                                TerminalStatusPayload {
                                    session_id: session_id_clone.clone(),
                                    state: TerminalLifecycleState::Failed,
                                    message: Some(format!(
                                        "Couldn't reach the remote host: {error}. \
                                         Try Test Connection in Settings → Hosts, \
                                         or right-click → Pull back."
                                    )),
                                    exit_code: None,
                                },
                            );
                            remove_session_runtime(&pty_state.sessions, &session_id_clone);
                            return;
                        }
                        eprintln!(
                            "[codemux::terminal] persistent-shell path failed for session \
                             {session_id_clone}: {error}; falling back to in-process spawn"
                        );
                        let sid = session_id_clone.clone();
                        let app_fb = app_clone.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            spawn_pty_for_session_in_process(app_fb, sid);
                        });
                    }
                }
            });
            return;
        }
    }
    spawn_pty_for_session_in_process(app, session_id);
}

/// True if the session belongs to a workspace with `host_id` set.
/// Used to gate the in-process fallback — local workspaces still
/// fall back happily, remote ones must surface the real error.
#[cfg(unix)]
fn is_remote_workspace_for_session<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
) -> bool {
    let app_state: State<'_, AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    find_owning_workspace(&snapshot, session_id)
        .and_then(|w| w.host_id)
        .is_some()
}

fn spawn_pty_for_session_in_process<R: Runtime>(app: AppHandle<R>, session_id: String) {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    // Atomic reservation: insert a placeholder runtime with `is_spawning = true`
    // under the `PtyState::sessions` lock so a concurrent spawn attempt for the
    // same session id sees the marker and bails. The previous "is writer/master
    // populated?" check released the lock before doing slow ConPTY init, which
    // on Windows produced a 100ms+ window where startup session restore and
    // user-driven workspace creation could both pass the check and double-spawn
    // the same session id, eventually leaking ~23 ConPTY children before the
    // app froze. See `try_reserve_session_spawn` for the lock ordering.
    if !try_reserve_session_spawn(&sessions, &session_id) {
        return;
    }

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some("Starting shell...".into()),
            exit_code: None,
        },
    );

    let pty_system = native_pty_system();
    let pty_pair = match pty_system.openpty(PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to open PTY: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation we took out at the top of this function
            // so a future retry / restart_terminal_session can re-try.
            remove_session_runtime(&sessions, &session_id);
            return;
        }
    };

    let shell = default_shell();
    app_state.update_terminal_session_shell(&session_id, shell.clone());

    let cwd = session_working_dir(&app_state, &session_id);
    let mut cmd = CommandBuilder::new(shell.clone());
    cmd.cwd(cwd);

    // Declare terminal capabilities — Codemux is the terminal emulator, so it
    // must advertise what it supports.  Without these, CLI tools launched from a
    // desktop shortcut (no parent terminal) lose ANSI color output.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "codemux");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

    let snapshot = app_state.snapshot();

    // Find the workspace that owns this session by walking each workspace's
    // pane tree.  Orphaned sessions (no workspace references them) get only
    // session-level env vars — no workspace-specific injection, to avoid
    // silently using the wrong workspace's context.
    let owning_ws = find_owning_workspace(&snapshot, &session_id);

    cmd.env("CODEMUX", "1");
    cmd.env("CODEMUX_VERSION", env!("CARGO_PKG_VERSION"));
    cmd.env("CODEMUX_SURFACE_ID", session_id.clone());
    cmd.env("CODEMUX_SESSION_ID", session_id.clone());
    cmd.env("CODEMUX_BROWSER_CMD", "codemux browser");
    cmd.env("BROWSER", "codemux browser open");

    if let Some(ws) = owning_ws {
        cmd.env("CODEMUX_WORKSPACE_ID", &ws.workspace_id.0);
        for (key, val) in workspace_pty_env(ws) {
            cmd.env(&key, val);
        }
    } else {
        eprintln!(
            "[codemux::terminal] No owning workspace found for session {session_id}; \
             skipping workspace env injection"
        );
        cmd.env(
            "CODEMUX_AGENT_CONTEXT",
            crate::agent_context::build_agent_context(None, None, None, None),
        );
    }

    // Inject pane ID and hook server port for agent status notifications.
    // Also detect restored sessions so preset auto-launch can be skipped and
    // the full resume command can be injected later.
    let mut auto_resume_command: Option<String> = None;
    if let Some((_ws_id, pane_id)) = snapshot.workspaces.iter().find_map(|ws| {
        ws.surfaces
            .iter()
            .find_map(|s| crate::state::find_terminal_pane_id(&s.root, &session_id))
            .map(|pane_id| (ws.workspace_id.0.clone(), pane_id.0))
    }) {
        cmd.env("CODEMUX_PANE_ID", &pane_id);
    }

    let session_restore_enabled = settings_sync::load_cache()
        .map(|s| s.session_restore.enabled)
        .unwrap_or(true);
    if session_restore_enabled {
        if let Some(adapter_state) = app.try_state::<crate::session_adapters::AdapterState>() {
            if let Some((ws_id, pane_id, meta)) =
                crate::scrollback::find_scrollback_meta_for_session(&session_id)
            {
                // Override CWD with the original working directory so CWD-scoped
                // tools (e.g. `claude --resume`) find their sessions.
                let effective_cwd = resolve_session_cwd(
                    &meta.working_directory,
                    &session_working_dir(&app_state, &session_id),
                );
                cmd.cwd(&effective_cwd);
                cmd.env("CODEMUX_PANE_ID", &pane_id);
                if let Some(resume_command) =
                    resolve_resume_command(&snapshot, &meta, &adapter_state)
                {
                    eprintln!(
                        "[codemux::terminal] Skipping auto-launch for {session_id}: restored session found at {ws_id}/{pane_id}"
                    );
                    auto_resume_command = Some(resume_command);
                } else {
                    eprintln!(
                        "[codemux::terminal] Restored session metadata found for {session_id} at {ws_id}/{pane_id} but no resume command could be built"
                    );
                }
            } else {
                eprintln!(
                    "[codemux::terminal] No scrollback metadata found for restored session {session_id}"
                );
            }
        }
    }
    if let Some(port) = crate::hooks::hook_port() {
        cmd.env("CODEMUX_HOOK_PORT", port.to_string());
    }

    // Add codemux CLI shim to PATH so `codemux` commands work in user terminals.
    // On Windows, `build_child_path` also prepends `%USERPROFILE%\.local\bin`
    // so Claude Code and similar per-user installs are discoverable even if
    // Codemux's own environment missed the installer's PATH broadcast.
    if let Some((shim_dir, current_exe)) = ensure_cli_shims() {
        let current_path = env::var("PATH").unwrap_or_default();
        let prefixed = build_child_path(&shim_dir, &current_path);
        cmd.env("PATH", prefixed);
        cmd.env("CODEMUX_CLI_SAFE_PATH", current_exe);
    }

    // Renderer transport flags are an app-process concern only.
    strip_renderer_env(&mut cmd);

    let child = match pty_pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to spawn shell {shell}: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation taken at the top of this function;
            // see the same pattern at the openpty failure branch above.
            remove_session_runtime(&sessions, &session_id);
            return;
        }
    };

    // Wrap child in a guard that kills+waits on drop, preventing zombies
    // if a subsequent step fails before the waiter thread is spawned.
    let guard = ChildGuard::new(child);

    // A session without a PID is unkillable — `terminate_pty_session`
    // would later find no `child_pid` and silently skip the kill, leaking
    // the shell and everything it spawned. Fail the spawn loudly instead.
    // The guard will kill the child on drop during the early return.
    let child_pid = match guard.child.as_ref().and_then(|c| c.process_id()) {
        Some(pid) => pid,
        None => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(
                        "portable_pty returned a child with no process_id(); \
                         refusing to start an unkillable session"
                            .into(),
                    ),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    drop(pty_pair.slave);

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to clone PTY reader: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    let writer = match pty_pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to take PTY writer: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    // All resources acquired — disarm the guard and hand child to the waiter thread.
    let mut child = guard.disarm();

    // Extract the raw fd for poll() before moving master into SessionRuntime.
    // The reader fd is a dup of this fd, so polling either detects data on both.
    #[cfg(unix)]
    let poll_fd: PollFd = pty_pair.master.as_raw_fd().unwrap_or(-1);
    #[cfg(windows)]
    let poll_fd: PollFd = ();

    // Clone the back-pressure flag out under the same lock that publishes the
    // runtime, so the reader thread and the `pause_pty_output`/`resume_pty_output`
    // commands share one `Arc<AtomicBool>` with no further locking on the hot
    // read path. A fresh spawn always starts unpaused.
    let flow_paused = with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(writer);
            runtime.master = Some(pty_pair.master);
            runtime.child_pid = Some(child_pid);
            runtime.skip_preset_launch = auto_resume_command.is_some();
            runtime.resume_command = auto_resume_command.clone();
            runtime.flow_paused.store(false, Ordering::Relaxed);
            // Spawn complete — clear the reservation marker so the runtime
            // looks identical to a non-spawning runtime in every downstream
            // check (terminate_pty_session, restart_terminal_session, etc.).
            runtime.is_spawning = false;
            runtime.flow_paused.clone()
        },
    );

    if let Some(command) = auto_resume_command {
        let sessions_for_command = sessions.clone();
        let session_id_for_command = session_id.clone();
        crate::commands::presets::write_command_when_ready(
            sessions_for_command,
            session_id_for_command,
            command,
            120,
        );
    }

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Ready,
            message: Some(format!("Shell ready: {shell}")),
            exit_code: None,
        },
    );

    // Wire up the session adapter output scanner if this session has an original_command.
    let adapter_clone: Option<crate::session_adapters::AdapterState> = app
        .try_state::<crate::session_adapters::AdapterState>()
        .map(|s| s.inner().clone());
    let snapshot = app_state.snapshot();
    let original_cmd = snapshot
        .terminal_sessions
        .iter()
        .find(|s| s.session_id.0 == session_id)
        .and_then(|s| s.original_command.clone());

    // Only create a scanner (and the per-byte hook) when a preset command was used.
    // Plain shell panes get no scanner and a no-op hook — zero overhead in the hot path.
    let has_scanner = if let (Some(ref adapter), Some(ref cmd)) = (&adapter_clone, &original_cmd) {
        adapter.start_scanner(&session_id, cmd).is_some()
    } else {
        false
    };

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    let read_flow_paused = flow_paused;
    let scanner_session_id = session_id.clone();
    let mut line_buf = Vec::<u8>::new();

    std::thread::spawn(move || {
        batched_reader_loop(
            &mut reader,
            poll_fd,
            &read_sessions,
            &read_session_id,
            &read_flow_paused,
            |data: &[u8]| {
                if !has_scanner {
                    return;
                }
                let Some(ref adapter) = adapter_clone else {
                    return;
                };

                // Feed bytes into line buffer, scan complete lines
                for &byte in data {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let clean = strip_ansi_codes(&line);
                        adapter.scan_line(&scanner_session_id, &clean);
                        line_buf.clear();
                    } else if byte != b'\r' {
                        line_buf.push(byte);
                    }
                }
            },
        );
    });

    let wait_app = app.clone();
    let wait_sessions = sessions.clone();
    let wait_session_id = session_id.clone();
    std::thread::spawn(move || {
        let payload = match child.wait() {
            Ok(status) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Exited,
                message: Some(if status.success() {
                    "Shell exited successfully".into()
                } else {
                    format!("Shell exited with code {}", status.exit_code())
                }),
                exit_code: Some(status.exit_code()),
            },
            Err(error) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Failed,
                message: Some(format!("Failed while waiting for shell: {error}")),
                exit_code: None,
            },
        };

        // Send terminal reset sequences to xterm.js via the IPC channel
        // before tearing down the session. This restores xterm.js to a clean
        // state after apps that enable mouse tracking, alt screen, etc.
        const TERMINAL_RESET: &[u8] =
            b"\x1b[?1049l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?2004l\x1b[?25h\x1b[0m";
        queue_or_send_output(&wait_sessions, &wait_session_id, TERMINAL_RESET.to_vec());

        with_session_runtime(
            &wait_sessions,
            &wait_session_id,
            || SessionRuntime::new(&wait_session_id),
            |runtime| {
                runtime.writer = None;
                runtime.master = None;
            },
        );

        emit_terminal_status(&wait_app, &wait_sessions, payload);

        // Clean up the session runtime to prevent memory leak
        remove_session_runtime(&wait_sessions, &wait_session_id);

        state::emit_app_state(&wait_app);
    });
}

/// Walk a workspace's pane tree and return every terminal session id under it.
///
/// Used by `spawn_missing_ptys` (active workspace at startup) and
/// `spawn_missing_ptys_for_workspace` (lazy spawn when the user activates an
/// inactive workspace). Returns an empty vec if no workspace matches `workspace_id`.
fn collect_workspace_session_ids(
    snapshot: &crate::state::AppStateSnapshot,
    workspace_id: &str,
) -> Vec<String> {
    snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .map(|w| crate::state::collect_terminal_sessions(&w.surfaces))
        .unwrap_or_default()
}

/// Spawn PTYs for every terminal session belonging to `workspace_id` that
/// isn't already running. Used by the lazy-activation path so that when a user
/// switches workspaces we materialize that workspace's PTYs on demand.
///
/// Idempotent: each `spawn_pty_for_session` call is gated by
/// `try_reserve_session_spawn`, so re-calling this for an already-active
/// workspace is a cheap no-op.
pub fn spawn_missing_ptys_for_workspace<R: Runtime>(app: AppHandle<R>, workspace_id: &str) {
    let app_state: State<'_, AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let session_ids = collect_workspace_session_ids(&snapshot, workspace_id);

    if session_ids.len() > MAX_STARTUP_SESSIONS {
        eprintln!(
            "[codemux::terminal] Workspace {workspace_id} has {} sessions; \
             spawning only the first {} to avoid blocking the IPC thread",
            session_ids.len(),
            MAX_STARTUP_SESSIONS
        );
    }

    for session_id in session_ids.into_iter().take(MAX_STARTUP_SESSIONS) {
        spawn_pty_for_session(app.clone(), session_id);
    }
}

/// Emit a transient `Migrating` lifecycle status for every terminal session in
/// `workspace_id`. The cloud-push / pull-back flow calls this AFTER the old
/// PTYs are terminated and BEFORE the replacements spawn, so each pane shows a
/// "Switching to <host>…" (push) or "Returning to this device…" (pull-back)
/// overlay during the few seconds the session is being migrated instead of a
/// frozen copy of the old scrollback.
///
/// The overlay auto-dismisses when the respawned session emits `Ready` (or its
/// first output chunk reaches the frontend — the renderer treats live output on
/// a migrating pane as proof the new PTY is alive).
///
/// Rides the same `last_status` plumbing as every other lifecycle emit, so a
/// mid-migration workspace switch + return still shows the overlay
/// (`get_terminal_status` replays `last_status`).
pub fn emit_migrating_for_workspace<R: Runtime>(app: &AppHandle<R>, workspace_id: &str, message: &str) {
    let app_state: State<'_, AppStateStore> = app.state();
    let pty_state: State<'_, PtyState> = app.state();
    let snapshot = app_state.snapshot();
    for session_id in collect_workspace_session_ids(&snapshot, workspace_id) {
        emit_terminal_status(
            app,
            &pty_state.sessions,
            TerminalStatusPayload {
                session_id,
                state: TerminalLifecycleState::Migrating,
                message: Some(message.to_string()),
                exit_code: None,
            },
        );
    }
}

/// Startup PTY hydration. Restores PTY children only for sessions that belong
/// to the **active** workspace, leaving inactive workspaces' sessions to be
/// materialized lazily by `spawn_missing_ptys_for_workspace` when the user
/// switches into them via `activate_workspace`.
///
/// **Why active-only.** The previous implementation walked every persisted
/// `terminal_sessions` entry and called `spawn_pty_for_session` for each, up
/// to `MAX_STARTUP_SESSIONS = 50`. On Linux this is microsecond-cheap and
/// invisible. On Windows ConPTY init takes hundreds of milliseconds per
/// child, so a saved state with N sessions blocked the synchronous IPC
/// thread for N × ~300ms during startup, producing the observed "23 ConPTY
/// children spawning at once → app frozen → force-close" failure mode.
///
/// **What "active" means at startup.** `state::AppStateStore` loads the
/// persisted active workspace id from disk during startup, **before** this
/// function runs (via `lib.rs` setup), so by the time `spawn_missing_ptys`
/// is called the snapshot's `active_workspace_id` is the same workspace
/// id the user was on when they last closed the app. If the persisted
/// active workspace id doesn't resolve to a real workspace (e.g. it was
/// deleted between sessions, or no workspaces exist yet) we spawn nothing
/// — the next user-driven action will spawn its own PTY.
///
/// Other workspaces' sessions are not lost: their persisted scrollback +
/// metadata stays on disk, and `spawn_missing_ptys_for_workspace` rehydrates
/// them on workspace switch.
pub fn spawn_missing_ptys<R: Runtime>(app: AppHandle<R>) {
    let app_state: State<'_, AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let active_workspace_id = snapshot.active_workspace_id.0.clone();

    if active_workspace_id.is_empty() {
        // No workspace persisted yet (fresh install, or all workspaces deleted).
        // The next user-driven workspace creation will spawn its own PTYs via
        // `create_workspace_with_layout` / `create_worktree_workspace`.
        return;
    }

    spawn_missing_ptys_for_workspace(app, &active_workspace_id);
}

#[tauri::command]
pub fn create_terminal_session<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppStateStore>,
) -> Result<String, String> {
    let session_id = app_state.create_terminal_session()?;
    state::emit_app_state(&app);
    spawn_pty_for_session(app, session_id.0.clone());
    Ok(session_id.0)
}

#[tauri::command]
pub fn activate_terminal_session<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppStateStore>,
    session_id: String,
) -> Result<(), String> {
    if app_state.activate_terminal_session(&session_id) {
        state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No terminal session found for {session_id}"))
    }
}

/// Kill the PTY child process for a session and release its runtime buffers.
///
/// This is the **only** code path that actually reaches `kill_session_tree`.
/// Every close entry point (the `close_terminal_session` Tauri command,
/// `close_pane`, `close_tab`, `close_workspace`, `close_workspace_with_worktree`,
/// and `restart_terminal_session`) funnels through here.
///
/// Why it does not touch `AppStateStore`: higher-level close helpers
/// (`state.close_pane`, `state.close_tab`, `state.close_workspace`) already
/// remove sessions from `snapshot.terminal_sessions` as part of their pane-tree
/// bookkeeping. If this function then called `app_state.close_terminal_session`
/// it would fail with "No terminal session found" and short-circuit before
/// reaching the kill — which is exactly the latent bug that let PTY children
/// leak across every close path. Keeping this helper state-store-free means
/// it is idempotent and callable from any point in a close flow.
///
/// Race-safe vs. the waiter thread: both paths call `remove_session_runtime`,
/// which is a `HashMap::remove` under the `PtyState::sessions` mutex. Whoever
/// wins takes the runtime; the loser gets `None` and no-ops. This is also the
/// double-close safety property — calling `terminate_pty_session` twice on the
/// same id is a no-op on the second call.
///
/// We intentionally do **not** reap via `waitpid`: the waiter thread owns the
/// `Box<dyn Child>` and is blocked in `child.wait()`. When SIGKILL lands, that
/// `wait()` returns and the waiter reaps the child normally. Reaping from two
/// places would race into `ECHILD`.
pub(crate) fn terminate_pty_session(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) {
    let Some(mut runtime) = remove_session_runtime(sessions, session_id) else {
        // Either already removed by the waiter thread (child exited on its
        // own) or by a concurrent close. Nothing to do.
        return;
    };

    // Persistent (daemon-backed) sessions: the PID is owned by the
    // `codemux pty-daemon` process, not us. killpg would either signal a
    // process group we don't own (no-op + spurious EPERM in stderr) or, if
    // PIDs got recycled into something we *do* own, send SIGKILL to the
    // wrong process. The correct teardown for a persistent session is to
    // ask the daemon to close it via the socket. We do that here on a
    // detached tokio task so the close path stays sync.
    let was_persistent = runtime.persistent;
    let pid = runtime.child_pid.take();
    // Persistent (daemon-backed) sessions are Unix-only — the
    // pty_daemon module is `#[cfg(unix)]`. On Windows `was_persistent`
    // is always false (the daemon path never runs to set the flag),
    // so this branch is effectively dead on Windows; we cfg-gate it
    // so the compiler doesn't try to resolve `pty_daemon` or the
    // (also cfg-gated) `daemon_client` field there.
    #[cfg(unix)]
    {
        // Capture the daemon client BEFORE dropping runtime — for
        // remote sessions this is the per-workspace SSH-tunneled
        // client; for local sessions it's the singleton local-daemon
        // client.
        let daemon_client = runtime.daemon_client.take();
        if was_persistent {
            runtime.output_subscribers.clear();
            runtime.pending_output.clear();
            runtime.pending_output_bytes = 0;
            // Drop runtime first so any held Arcs (writer, etc.) release before
            // we await the daemon round-trip.
            drop(runtime);
            let session_id = session_id.to_string();
            tauri::async_runtime::spawn(async move {
                // Use the session's captured client. Fall back to the local
                // daemon only if the runtime never recorded one (restored
                // session before reattach completes) — this fallback is
                // harmless because the local daemon will just no-op on an
                // unknown session id rather than affecting the wrong process.
                let client_res = if let Some(c) = daemon_client {
                    Ok(c)
                } else {
                    crate::pty_daemon::ensure_daemon().await
                };
                match client_res {
                    Ok(client) => {
                        if let Err(error) = client.close(session_id.clone()).await {
                            eprintln!(
                                "[codemux::terminal] daemon close failed for persistent session \
                                 {session_id}: {error}"
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "[codemux::terminal] cannot reach daemon to close persistent session \
                             {session_id}: {error}"
                        );
                    }
                }
            });
            return;
        }
    }
    #[cfg(not(unix))]
    let _ = was_persistent;

    // Clear child_pid to None *first* so the `Drop for SessionRuntime`
    // safety-net impl stays silent on the happy path. Any non-None value
    // printed by Drop means something skipped this function.
    runtime.output_subscribers.clear();
    runtime.pending_output.clear();
    runtime.pending_output_bytes = 0;
    if let Some(master) = runtime.master.as_mut() {
        let _ = master.resize(PtySize {
            rows: 1,
            cols: 1,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
    // Drop the runtime *before* signalling: `kill_session_tree` sleeps 200ms
    // between SIGTERM and SIGKILL and there is no point holding PTY handles
    // across that sleep.
    drop(runtime);

    match pid {
        Some(pid) => kill_session_tree(pid),
        None => {
            // Two causes, both benign by the time we get here:
            //   1. The spawn path fails loudly if `process_id()` returns
            //      None (see `spawn_pty_for_session`),
            //      so a live session cannot reach this branch with
            //      child_pid=None.
            //   2. The waiter thread can transiently re-insert a phantom
            //      runtime (via `with_session_runtime`'s entry-or-insert)
            //      after we removed it but before it calls
            //      `remove_session_runtime` itself. That phantom has
            //      `child_pid = None` from `SessionRuntime::new()`, and
            //      a second `terminate_pty_session` racing in during the
            //      phantom window lands here.
            // In both cases there is no live process to kill. Logged only
            // so new close paths that forget to populate child_pid are
            // noticeable in dev.
            eprintln!(
                "[codemux::terminal] terminate_pty_session({session_id}): no child_pid \
                 (spawn returned None or waiter-thread race)"
            );
        }
    }
}

#[tauri::command]
pub fn close_terminal_session<R: Runtime>(
    app: AppHandle<R>,
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: String,
) -> Result<String, String> {
    let fallback_session = app_state.close_terminal_session(&session_id)?;
    terminate_pty_session(&terminal_state.sessions, &session_id);
    state::emit_app_state(&app);
    Ok(fallback_session.0)
}

/// Like `terminate_pty_session` but preserves `output_subscribers` +
/// `pending_output` for daemon-backed (persistent) sessions, so the
/// frontend's xterm stays connected across the kill-and-respawn that
/// happens on workspace push/pull.
///
/// Without this, terminate removes the runtime entirely; the next
/// spawn creates a fresh runtime with no output channel; all of the
/// respawned PTY's output (including the agent's UI) goes into
/// `pending_output` and only becomes visible when the user tab-
/// switches away and back, which triggers `attach_pty_output` to
/// reattach the channel and flush the buffer.
///
/// Falls back to the regular `terminate_pty_session` for non-
/// persistent sessions — the in-process path doesn't have the same
/// "respawn into same session id" pattern and its terminate semantics
/// should stay unchanged.
#[cfg(unix)]
pub(crate) fn terminate_pty_session_keep_channel(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) {
    // Mutate in place if persistent. Returns Some(daemon_client) for
    // a persistent session we handled, None otherwise (we then fall
    // through to the regular terminate).
    let handled = with_existing_session_runtime(sessions, session_id, |rt| {
        if !rt.persistent {
            return None;
        }
        let daemon_client = rt.daemon_client.take();
        rt.child_pid = None;
        rt.writer = None;
        rt.master = None;
        // `persistent` flips to false so try_reserve_session_spawn
        // sees an idle slot and reserves it. The next spawn flips it
        // back to true after attaching.
        rt.persistent = false;
        rt.is_spawning = false;
        rt.skip_preset_launch = false;
        rt.resume_command = None;
        // PRESERVED (the whole point): output_subscribers,
        // pending_output, pending_output_bytes, last_status.
        Some(daemon_client)
    })
    .flatten();

    match handled {
        Some(daemon_client) => {
            // Tell the (old) daemon to close its side of the session.
            // For remote workspaces this is the per-workspace SSH-
            // tunneled client; for local persistent it's the singleton
            // local daemon client. Background tokio task so we stay
            // sync at this call site.
            if let Some(client) = daemon_client {
                let session_id = session_id.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = client.close(session_id.clone()).await {
                        eprintln!(
                            "[codemux::terminal] daemon close (keep-channel) failed for \
                             {session_id}: {error}"
                        );
                    }
                });
            }
        }
        None => {
            // Not persistent (or runtime missing) — defer to the
            // regular terminate which handles the in-process path.
            terminate_pty_session(sessions, session_id);
        }
    }
}

#[tauri::command]
pub fn restart_terminal_session<R: Runtime>(
    app: AppHandle<R>,
    terminal_state: State<'_, PtyState>,
    session_id: String,
) -> Result<(), String> {
    // Kill the old process group before spawning the replacement. Uses the
    // same helper as close so the Drop safety net stays silent.
    terminate_pty_session(&terminal_state.sessions, &session_id);
    spawn_pty_for_session(app, session_id);
    Ok(())
}

#[tauri::command]
pub fn get_terminal_status(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: Option<String>,
) -> Result<TerminalStatusPayload, String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    // Use the existing-only variant. The auto-create variant
    // (`with_session_runtime`) would conjure a fresh `SessionRuntime`
    // here whose `last_status` defaults to `Starting`, which the
    // frontend would dutifully display as a "Terminal starting…"
    // overlay over a session that's actually dead. This was the
    // spurious-Starting-popup bug on tab return for remote
    // workspaces: the push terminated the session, the frontend
    // later called getTerminalStatus, the auto-create gave back a
    // synthetic Starting, and the popup appeared.
    //
    // Returning a synthetic Exited on miss is more honest — the
    // session has no runtime, it's not coming back on its own. The
    // frontend's overlay handler already knows how to display Exited
    // cleanly.
    let status = with_existing_session_runtime(
        &terminal_state.sessions,
        &session_id,
        |runtime| runtime.last_status.clone(),
    )
    .unwrap_or_else(|| TerminalStatusPayload {
        session_id: session_id.clone(),
        state: TerminalLifecycleState::Exited,
        message: Some("Session is no longer running".into()),
        exit_code: None,
    });

    Ok(status)
}

/// Register `channel` as a live subscriber to a session's PTY output and
/// return the subscriber's generation token. The caller passes that token to
/// [`detach_pty_output`] (and `pause_pty_output`/`resume_pty_output`) so a
/// stale teardown from a superseded consumer can never touch a newer one.
///
/// Multiple subscribers can attach to the same session simultaneously (the
/// desktop window plus any mirror clients); each receives an identical byte
/// stream via the fan-out in `queue_or_send_output`. The `pending_output`
/// replay ring is flushed **only to the newly attached channel**, so a late
/// joiner catches up without re-delivering history to the existing
/// subscribers.
#[tauri::command]
pub fn attach_pty_output(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    channel: Channel<Vec<u8>>,
    session_id: Option<String>,
    skip_pending: Option<bool>,
) -> Result<u64, String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    let (generation, pending_chunks) = with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.next_output_generation =
                runtime.next_output_generation.saturating_add(1);
            let generation = runtime.next_output_generation;
            runtime.output_subscribers.push(OutputSubscriber {
                generation,
                channel: channel.clone(),
                // A freshly attached consumer is, by definition, caught up, so
                // it starts unpaused.
                flow_paused: false,
            });
            // Self-heal: adding an unpaused subscriber means "all subscribers
            // paused" can no longer hold, so the in-process read loop resumes.
            // Guards against a prior consumer that paused and was torn down
            // before its resume landed. (The daemon clears its own flag on
            // `Attach` for the same reason; the recomputed verdict is also
            // forwarded to it below.)
            recompute_flow_paused(runtime);
            let pending = if skip_pending.unwrap_or(false) {
                vec![]
            } else {
                runtime.pending_output.iter().cloned().collect::<Vec<_>>()
            };
            (generation, pending)
        },
    );

    // Forward the recomputed back-pressure verdict to the daemon (no-op for
    // in-process sessions). Done outside the sessions lock.
    forward_flow_control_to_daemon(&terminal_state.sessions, &session_id);

    // Replay history to the NEW subscriber only — the existing subscribers
    // already have it. Sent outside the lock like all other channel I/O.
    for chunk in pending_chunks {
        channel
            .send(chunk)
            .map_err(|error| format!("Failed to flush buffered PTY output: {error}"))?;
    }

    Ok(generation)
}

/// Tear down a subscriber installed by [`attach_pty_output`]. A
/// `Some(generation)` removes only that subscriber, so a late detach from a
/// consumer that already lost the session (unmount racing a remount, or a
/// second consumer attaching the same session) can never evict a newer
/// subscriber. `None` removes every subscriber for the session (legacy
/// whole-session detach). Idempotent: an unknown session or a superseded
/// generation is a no-op.
#[tauri::command]
pub fn detach_pty_output(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: Option<String>,
    generation: Option<u64>,
) -> Result<(), String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    with_existing_session_runtime(
        &terminal_state.sessions,
        &session_id,
        |runtime| {
            match generation {
                Some(generation) => runtime
                    .output_subscribers
                    .retain(|s| s.generation != generation),
                None => runtime.output_subscribers.clear(),
            }
            // Self-heal: the removed consumer's pause request dies with it, so
            // recomputing may resume the reader — a backgrounded child (e.g. a
            // daemon-less agent left running on tab switch) is not left blocked
            // on `write()`. Removing the last subscriber also resumes (an empty
            // set never parks).
            recompute_flow_paused(runtime);
        },
    );

    // Push the recomputed verdict to the daemon (no-op for in-process sessions).
    forward_flow_control_to_daemon(&terminal_state.sessions, &session_id);

    Ok(())
}

/// Pause one subscriber's view of a session's PTY output (terminal flow
/// control). The renderer calls this when a fast producer outruns its xterm
/// write queue. Pausing does not stop the reader outright: the child only
/// blocks on `write()` (real back-pressure) once EVERY subscriber has paused —
/// a caught-up mirror consumer keeps the stream flowing. `Some(generation)` is
/// the token returned by [`attach_pty_output`], identifying the calling
/// subscriber; `None` pauses every subscriber for the session (legacy).
///
/// Works on BOTH spawn paths: the recomputed "all paused" verdict drives the
/// in-process `batched_reader_loop`'s shared flag and is forwarded to the
/// daemon's read loop for daemon-backed sessions. Both paths self-heal (resume
/// on attach/detach + a max-park backstop) so a paused PTY can never wedge even
/// if the matching resume is never delivered.
#[tauri::command]
pub fn pause_pty_output(
    terminal_state: State<'_, PtyState>,
    session_id: String,
    generation: Option<u64>,
) -> Result<(), String> {
    set_pty_flow_paused(&terminal_state.sessions, &session_id, generation, true);
    Ok(())
}

/// Resolve the *live* working directory of each requested session's shell.
///
/// This is the fallback half of the terminal-header cwd feature. The
/// preferred source is OSC 7, which the shell pushes on every prompt — but
/// that needs shell integration the user may not have (a bare zsh commonly
/// emits nothing). Reading `/proc/<pid>/cwd` needs no shell cooperation at
/// all, so between the two every local session gets a directory.
///
/// The pid is the session's **shell**, not whatever is running in the
/// foreground, which is exactly what we want: `cd` moves the shell, and a
/// long-running child (a build, an agent) leaves the shell's cwd alone, so
/// the header stays stable instead of following a subprocess around.
///
/// Sessions with no entry in the response are simply unknown to this source
/// and are left to OSC 7:
/// - **remote/SSH panes** — the pid lives on another machine, and reading a
///   local `/proc` entry for it would report an unrelated process's
///   directory. Those sessions have no local `child_pid`, so they drop out
///   here rather than reporting a wrong path.
/// - **non-Linux hosts** — no `/proc`; the whole body compiles out.
/// - **dead or exited sessions** — the readlink fails and is skipped.
///
/// Returns a map rather than erroring per-session so one dead pid in a
/// batch can't fail the poll for every other pane.
///
/// Async + `spawn_blocking` because this is a polled path doing filesystem
/// I/O: a sync command would run the readlink loop on the GTK main thread
/// every tick (see the invariant note in `commands/files.rs`). The pid
/// snapshot is taken before handing off so the mutex guard never crosses
/// into the blocking task.
#[tauri::command]
pub async fn terminal_session_cwds(
    terminal_state: State<'_, PtyState>,
    session_ids: Vec<String>,
) -> Result<HashMap<String, String>, String> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let pids = terminal_state.get_session_pids();
    tokio::task::spawn_blocking(move || read_cwds_for_sessions(&pids, &session_ids))
        .await
        .map_err(|e| format!("terminal_session_cwds task join failed: {e}"))
}

/// Resolve `session_id -> cwd` for the requested sessions given a
/// `session_id -> pid` map. Split out from the command so it can be unit
/// tested without a Tauri `State`.
///
/// Sessions with no pid, or whose pid can't be read, are omitted rather
/// than reported with a placeholder — the caller keeps its previous value,
/// which is strictly better than blanking a header on a transient failure.
fn read_cwds_for_sessions(
    pids: &HashMap<String, u32>,
    session_ids: &[String],
) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(session_ids.len());
    for session_id in session_ids {
        let Some(pid) = pids.get(session_id).copied() else {
            continue;
        };
        if let Some(cwd) = read_process_cwd(pid) {
            out.insert(session_id.clone(), cwd);
        }
    }
    out
}

/// The working directory of a live process, or None when it can't be read
/// (process gone, or a platform without `/proc`).
fn read_process_cwd(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // `/proc/<pid>/cwd` is a symlink to the process's working directory.
        // readlink is a single cheap syscall and needs no ptrace-level
        // privilege for a process we own.
        std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()?
            .to_str()
            .map(str::to_string)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No `/proc`; OSC 7 is the only cwd source on these platforms.
        let _ = pid;
        None
    }
}

/// Resume one subscriber's view of a session's PTY output. `None` resumes every
/// subscriber for the session. See `pause_pty_output`. Idempotent.
#[tauri::command]
pub fn resume_pty_output(
    terminal_state: State<'_, PtyState>,
    session_id: String,
    generation: Option<u64>,
) -> Result<(), String> {
    set_pty_flow_paused(&terminal_state.sessions, &session_id, generation, false);
    Ok(())
}

/// Record a flow-control request (`paused`), recompute the session's derived
/// "all paused" verdict, and apply it to whichever read loop owns the session.
///
/// `Some(generation)` sets just that subscriber's request; a generation that no
/// longer matches any subscriber is a benign no-op (the consumer detached
/// between the renderer's watermark check and this IPC). `None` sets the
/// request on every current subscriber (legacy whole-session pause/resume).
fn set_pty_flow_paused(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    generation: Option<u64>,
    paused: bool,
) {
    with_existing_session_runtime(sessions, session_id, |runtime| {
        match generation {
            Some(generation) => {
                if let Some(sub) = runtime
                    .output_subscribers
                    .iter_mut()
                    .find(|s| s.generation == generation)
                {
                    sub.flow_paused = paused;
                }
            }
            None => {
                for sub in runtime.output_subscribers.iter_mut() {
                    sub.flow_paused = paused;
                }
            }
        }
        recompute_flow_paused(runtime);
    });

    // Push the recomputed verdict to the daemon (no-op for in-process sessions).
    forward_flow_control_to_daemon(sessions, session_id);
}

/// Forward a session's current derived `flow_paused` verdict to the daemon's
/// own flow gate for daemon-backed sessions. Fire-and-forget (mirroring
/// `DaemonWriter`): flow control is advisory and a failure (e.g. the session
/// just exited) is benign. A no-op for in-process sessions — no daemon client
/// exists, and the in-process reader already reads the shared atomic directly.
fn forward_flow_control_to_daemon(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) {
    // Read the already-recomputed verdict and grab the daemon client (if any)
    // in one lock pass.
    let snapshot = with_existing_session_runtime(sessions, session_id, |runtime| {
        let effective = runtime.flow_paused.load(Ordering::Relaxed);
        #[cfg(unix)]
        {
            (effective, runtime.daemon_client.clone())
        }
        #[cfg(not(unix))]
        {
            (effective, Option::<()>::None)
        }
    });

    #[cfg(unix)]
    if let Some((effective, Some(client))) = snapshot {
        let session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = client.set_flow_paused(session_id.clone(), effective).await {
                eprintln!(
                    "[codemux::terminal] flow-control set_flow_paused(paused={effective}) \
                     for {session_id} failed (benign if the session just exited): {error}"
                );
            }
        });
    }
    #[cfg(not(unix))]
    {
        let _ = snapshot;
    }
}

#[tauri::command]
pub fn write_to_pty(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    data: String,
    session_id: Option<String>,
) -> Result<(), String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;

            writer
                .write_all(data.as_bytes())
                .map_err(|error| format!("Failed to write to PTY: {error}"))?;
            writer
                .flush()
                .map_err(|error| format!("Failed to flush PTY writer: {error}"))
        },
    )
}

/// Write data to a PTY by session ID (non-Tauri helper for internal use).
pub fn write_to_pty_by_session(
    pty_state: &State<'_, PtyState>,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    with_session_runtime(
        &pty_state.sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;
            writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write to PTY: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("Failed to flush PTY writer: {e}"))
        },
    )
}

/// Write data to a PTY by session ID using a PtyState directly (for spawned threads).
pub fn write_to_pty_by_session_direct(
    pty_state: &PtyState,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    with_session_runtime(
        &pty_state.sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;
            writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write to PTY: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("Failed to flush PTY writer: {e}"))
        },
    )
}

/// Hard cap on `lines` that `read_terminal_output` will return. Guards against
/// an MCP client asking for the entire 256 MiB pending-output buffer in one
/// shot, which would (a) be useless to the agent and (b) bloat the JSON-RPC
/// response over the stdio transport. Matches `terminal_read` MCP-tool docs.
pub const READ_TERMINAL_MAX_LINES: usize = 5000;

/// Default line count when `terminal_read` is called without an explicit
/// `lines` argument. Sized so a typical "what's on screen right now" snapshot
/// (a few hundred lines of agent output + tool calls) fits without truncation
/// but the response stays compact.
pub const READ_TERMINAL_DEFAULT_LINES: usize = 200;

/// Outcome of a `read_terminal_output` call. Mirrors `write_to_pty`'s
/// request/response shape (session_id resolution semantics, error type), but
/// returns the buffered text plus how many lines were actually included so
/// callers can detect truncation without diffing line counts themselves.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadTerminalOutput {
    pub session_id: String,
    pub data: String,
    pub lines_returned: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

/// Read the most recent `requested_lines` (clamped to `READ_TERMINAL_MAX_LINES`)
/// of PTY output from a session's in-memory `pending_output` buffer.
///
/// This is the read side of the MCP `terminal_read` tool. The buffer is the
/// same rolling chunk deque that `attach_pty_output` replays to the frontend
/// on workspace re-attach — it grows on every PTY chunk and is evicted FIFO at
/// `OUTPUT_BUFFER_BYTE_LIMIT` (256 MiB in release builds), so this helper sees
/// the same scrollback the user would see in the terminal pane minus anything
/// the eviction loop has already discarded.
///
/// Output bytes are decoded with `from_utf8_lossy` and split on `\n`. Trailing
/// `\r` from CRLF endings is stripped per line so agents reading the response
/// don't have to handle Windows-style line endings themselves. Empty lines are
/// preserved.
pub fn read_terminal_output(
    terminal_state: &PtyState,
    app_state: &AppStateStore,
    requested_lines: Option<usize>,
    session_id: Option<String>,
) -> Result<ReadTerminalOutput, String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    let line_cap = requested_lines
        .unwrap_or(READ_TERMINAL_DEFAULT_LINES)
        .min(READ_TERMINAL_MAX_LINES)
        .max(1);

    let chunks = {
        let guard = terminal_state
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let runtime = guard
            .get(&session_id)
            .ok_or_else(|| format!("Terminal session {session_id} not found"))?;
        runtime
            .pending_output
            .iter()
            .cloned()
            .collect::<Vec<Vec<u8>>>()
    };

    let total_bytes: usize = chunks.iter().map(|c| c.len()).sum();
    let mut combined = Vec::with_capacity(total_bytes);
    for chunk in &chunks {
        combined.extend_from_slice(chunk);
    }
    let (data, total_lines, lines_returned, truncated) = tail_pty_output(&combined, line_cap);

    Ok(ReadTerminalOutput {
        session_id,
        data,
        lines_returned,
        total_lines,
        truncated,
    })
}

/// Pure helper used by `read_terminal_output` (and exercised directly in unit
/// tests). Decodes raw PTY bytes with `from_utf8_lossy`, strips trailing
/// `\r` from CRLF-terminated lines, and returns the last `line_cap` lines
/// joined with `\n`.
///
/// Returns `(joined_text, total_lines, lines_returned, truncated)`. `truncated`
/// is true iff we dropped any leading lines to fit under the cap.
///
/// Split out from `read_terminal_output` so the line-tailing contract can be
/// tested without standing up a `PtyState` + `AppStateStore` + `tauri::State`
/// — the rest of the read path is just lookup + state plumbing.
pub fn tail_pty_output(raw: &[u8], line_cap: usize) -> (String, usize, usize, bool) {
    let text = String::from_utf8_lossy(raw);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let total_lines = lines.len();
    let cap = line_cap.max(1);
    let start = total_lines.saturating_sub(cap);
    let returned = &lines[start..];
    let data = returned.join("\n");
    let lines_returned = returned.len();
    (data, total_lines, lines_returned, start > 0)
}

#[tauri::command]
pub fn resize_pty<R: Runtime>(
    _app: AppHandle<R>,
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    rows: u16,
    cols: u16,
    session_id: Option<String>,
) -> Result<(), String> {
    if rows == 0 || cols == 0 {
        return Ok(());
    }

    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    // Persistent (daemon-backed) sessions have `master: None` because the
    // daemon owns the PTY. Route the resize over the socket instead. We
    // do this on a tokio task so the sync command handler returns
    // immediately; resize is fire-and-forget at the terminal level
    // anyway (xterm doesn't wait for an ack). Unix-only because the
    // daemon doesn't exist on Windows.
    #[cfg(unix)]
    {
        // Snapshot persistent + the daemon client that owns this session.
        // The client is captured at spawn time and may be either the local
        // singleton or a per-workspace SSH-tunneled client; either way it's
        // the one that knows about this session id.
        let (persistent, daemon_client) = with_session_runtime(
            &terminal_state.sessions,
            &session_id,
            || SessionRuntime::new(&session_id),
            |runtime| {
                Ok::<_, String>((runtime.persistent, runtime.daemon_client.clone()))
            },
        )?;
        if persistent {
            let session_id_clone = session_id.clone();
            // Fall back to ensure_daemon ONLY if the runtime is missing the
            // client — which happens on restored sessions before the
            // spawn-or-reattach path has run. For remote sessions on first
            // reattach this would route to the wrong daemon, but the
            // reattach path also re-populates daemon_client, so this gap
            // closes within the same tick.
            tauri::async_runtime::spawn(async move {
                let client_res = if let Some(c) = daemon_client {
                    Ok(c)
                } else {
                    crate::pty_daemon::ensure_daemon().await
                };
                match client_res {
                    Ok(client) => {
                        if let Err(error) =
                            client.resize(session_id_clone.clone(), rows, cols).await
                        {
                            eprintln!(
                                "[codemux::terminal] daemon resize failed for \
                                 {session_id_clone}: {error}"
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "[codemux::terminal] cannot reach daemon to resize \
                             {session_id_clone}: {error}"
                        );
                    }
                }
            });
            app_state.update_terminal_session_size(&session_id, cols, rows);
            return Ok(());
        }
    }

    with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            let master = runtime
                .master
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently resizable"))?;

            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|error| format!("Failed to resize PTY: {error}"))
        },
    )?;

    app_state.update_terminal_session_size(&session_id, cols, rows);

    Ok(())
}

/// Clear stuck Working/Permission status for a terminal session.
/// Called from the frontend when the user presses Escape in a terminal
/// where an agent was processing — the agent stops but stays alive,
/// so the PTY exit cleanup never fires.
#[tauri::command]
pub fn clear_agent_status<R: Runtime>(session_id: String, app_state: State<'_, AppStateStore>, app: AppHandle<R>) {
    // Nothing else moves on this path, so the clear ships as a `PaneStatus`
    // delta — the sidebar drops one dot without re-parsing the app state.
    if let Some(delta) = app_state.clear_transient_pane_status_by_session_delta(&session_id) {
        state::emit_app_state_delta(&app, delta);
    }
}

/// Decide whether to try the persistent-PTY-daemon path for this spawn.
///
/// Default app behavior: **always try the daemon first**. The only reasons
/// to skip it are:
///
/// - The platform isn't wired yet (Windows IPC TBD — falls back cleanly to
///   the in-process path so Windows users get the old behavior with zero
///   regression).
/// - The crash circuit breaker is open (daemon has been failing in a tight
///   loop; we stop trying for the rest of this app run).
/// - An env-var kill switch is set (`CODEMUX_DISABLE_PTY_DAEMON=1`), so we
///   have a panic button if a release ships and something goes badly wrong
///   in the field. Users never need to touch this in normal operation.
///
/// There is **no user-facing setting**. Persistent agents are the default
/// because the future cloud-push feature builds on the same mechanism, and
/// "your agent didn't die when the app closed" is a strict UX upgrade.
fn daemon_path_viable() -> bool {
    if std::env::var_os("CODEMUX_DISABLE_PTY_DAEMON").is_some() {
        return false;
    }
    #[cfg(not(unix))]
    {
        // Windows path: scaffolded but unvalidated. Until the named-pipe
        // server is wired and tested on a real Windows box, fall back to
        // in-process so Windows users keep the existing behavior. This
        // returns `false` unconditionally; flip when Windows support
        // lands.
        return false;
    }
    #[cfg(unix)]
    {
        !crate::pty_daemon::supervisor::circuit_is_open()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sessions() -> Arc<Mutex<HashMap<String, SessionRuntime>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// Install a fresh output subscriber exactly the way `attach_pty_output`
    /// does (mint a generation, push an unpaused entry, recompute flow) and
    /// return its generation. Test-only mirror of the command body so the
    /// unit tests don't need a real Tauri `State`/`AppHandle`.
    fn attach_subscriber(
        sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
        session_id: &str,
        channel: Channel<Vec<u8>>,
    ) -> u64 {
        with_session_runtime(
            sessions,
            session_id,
            || SessionRuntime::new(session_id),
            |runtime| {
                runtime.next_output_generation =
                    runtime.next_output_generation.saturating_add(1);
                let generation = runtime.next_output_generation;
                runtime.output_subscribers.push(OutputSubscriber {
                    generation,
                    channel,
                    flow_paused: false,
                });
                recompute_flow_paused(runtime);
                generation
            },
        )
    }

    /// Remove a subscriber by generation the way `detach_pty_output` does.
    fn detach_subscriber(
        sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
        session_id: &str,
        generation: u64,
    ) {
        with_existing_session_runtime(sessions, session_id, |runtime| {
            runtime
                .output_subscribers
                .retain(|s| s.generation != generation);
            recompute_flow_paused(runtime);
        });
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn read_cwds_for_sessions_resolves_live_pids_and_skips_the_rest() {
        // The test process is a live pid we know the cwd of, so it stands in
        // for a session's shell without spawning one.
        let self_pid = std::process::id();
        let expected = std::env::current_dir()
            .expect("cwd")
            .to_str()
            .expect("utf8 cwd")
            .to_string();

        let mut pids = HashMap::new();
        pids.insert("live".to_string(), self_pid);
        // A pid that cannot exist — `/proc/<pid>/cwd` readlink fails.
        pids.insert("dead".to_string(), u32::MAX);

        let requested = vec![
            "live".to_string(),
            "dead".to_string(),
            // No pid at all: the remote/SSH case, where the shell runs on
            // another machine. Must be omitted, never guessed at from a
            // local /proc entry.
            "remote".to_string(),
        ];
        let out = read_cwds_for_sessions(&pids, &requested);

        assert_eq!(out.get("live"), Some(&expected));
        assert!(!out.contains_key("dead"), "unreadable pid must be omitted");
        assert!(
            !out.contains_key("remote"),
            "session with no local pid must be omitted"
        );
    }

    #[test]
    fn read_cwds_for_sessions_only_answers_what_was_asked() {
        let mut pids = HashMap::new();
        pids.insert("asked".to_string(), std::process::id());
        pids.insert("not_asked".to_string(), std::process::id());

        let out = read_cwds_for_sessions(&pids, &["asked".to_string()]);
        assert!(!out.contains_key("not_asked"));
    }

    #[test]
    fn migrating_state_serializes_to_migrating_discriminant() {
        // The frontend overlay (TerminalStatusPayload union in types.ts) keys
        // off this exact string. If the snake_case rename ever drifts, the
        // "Switching to <host>…" overlay silently stops rendering — pin the
        // wire value here so that regression fails a test instead of shipping.
        let json = serde_json::to_string(&TerminalLifecycleState::Migrating).unwrap();
        assert_eq!(json, "\"migrating\"");
    }

    #[test]
    fn migrating_maps_to_starting_session_state() {
        // Migrating has no separate persisted variant — it collapses to
        // Starting so existing state consumers (overview, sidebar dots,
        // session persistence) are untouched. Only the transient
        // `terminal-status` event carries the `migrating` discriminant.
        assert!(matches!(
            map_status_state(&TerminalLifecycleState::Migrating),
            TerminalSessionState::Starting
        ));
    }

    #[test]
    fn strip_ansi_codes_handles_osc_sequences() {
        // OSC window-title (ESC ] 0 ; <title> BEL) is terminated by BEL, not by
        // the letters in its payload — the whole sequence must vanish.
        assert_eq!(strip_ansi_codes("\x1b]0;bash\x07"), "");
        assert_eq!(strip_ansi_codes("\x1b]0;bash\x07$ ls"), "$ ls");
        // OSC terminated by ST (ESC \) — e.g. a hyperlink.
        assert_eq!(strip_ansi_codes("\x1b]8;;http://x\x1b\\done"), "done");
        // CSI and charset/ESC-letter escapes still strip correctly.
        assert_eq!(strip_ansi_codes("\x1b[2J\x1b[Hhello"), "hello");
        assert_eq!(strip_ansi_codes("\x1b(Bplain"), "plain");
        assert_eq!(strip_ansi_codes("\x1bcreset"), "reset");
    }

    // ── Regression tests for the cross-machine push spawn bugs ────────
    //
    // Each of these pins one of the four bugs from the marathon
    // debugging session that landed in commit 6bb557e. If anyone
    // simplifies the affected logic later, these tests will catch
    // re-regressions before the user does.
    //
    // Unix-only — the helpers being tested (is_runtime_owned_by_client,
    // terminate_pty_session_keep_channel) and the PtyDaemonClient
    // they exercise are `#[cfg(unix)]` because the daemon model is
    // Unix-only. On Windows there's nothing to test here.
    #[cfg(unix)]
    mod cross_machine_push {
        use super::*;

    /// `is_runtime_owned_by_client` returns true when the runtime's
    /// `daemon_client` is the SAME Arc allocation as the caller's
    /// — that's a current read task and Exited should fire.
    #[tokio::test]
    async fn is_runtime_owned_by_client_matching_arc_returns_true() {
        let sessions = make_sessions();
        let client = crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        {
            let mut guard = sessions.lock().unwrap();
            let mut rt = SessionRuntime::new("session-X");
            rt.daemon_client = Some(client.clone());
            guard.insert("session-X".into(), rt);
        }
        assert!(
            is_runtime_owned_by_client(&sessions, "session-X", &client),
            "same Arc allocation must be detected as owner"
        );
    }

    /// `is_runtime_owned_by_client` returns false when the runtime's
    /// `daemon_client` is a DIFFERENT Arc allocation (the session was
    /// respawned with a fresh client). The caller is a stale read
    /// task whose Exited would clobber the new spawn's Ready.
    #[tokio::test]
    async fn is_runtime_owned_by_client_different_arc_returns_false() {
        let sessions = make_sessions();
        let old_client =
            crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        let new_client =
            crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        {
            let mut guard = sessions.lock().unwrap();
            let mut rt = SessionRuntime::new("session-X");
            rt.daemon_client = Some(new_client.clone());
            guard.insert("session-X".into(), rt);
        }
        assert!(
            !is_runtime_owned_by_client(&sessions, "session-X", &old_client),
            "old read task's stale Arc must be detected as no-longer-owner — \
             without this check, the stale Exited overrides the new spawn's Ready"
        );
    }

    /// `is_runtime_owned_by_client` returns false when the runtime
    /// has no daemon_client yet — covers the window between
    /// `terminate_pty_session_keep_channel` clearing the client and
    /// the new spawn populating it. A stale read task whose mpsc
    /// returns None during this window must NOT emit Exited.
    #[tokio::test]
    async fn is_runtime_owned_by_client_none_client_returns_false() {
        let sessions = make_sessions();
        let client =
            crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        {
            let mut guard = sessions.lock().unwrap();
            let mut rt = SessionRuntime::new("session-X");
            rt.daemon_client = None;
            guard.insert("session-X".into(), rt);
        }
        assert!(
            !is_runtime_owned_by_client(&sessions, "session-X", &client),
            "runtime with no daemon_client (between terminate and respawn) \
             must not be claimed by a stale read task"
        );
    }

    /// `is_runtime_owned_by_client` returns false when no runtime
    /// exists for the session id — covers the "session was fully
    /// removed" case. No Exited should fire for nonexistent sessions.
    #[tokio::test]
    async fn is_runtime_owned_by_client_missing_runtime_returns_false() {
        let sessions = make_sessions();
        let client =
            crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        assert!(
            !is_runtime_owned_by_client(&sessions, "session-missing", &client),
            "no runtime → no owner → must return false"
        );
    }

    /// `terminate_pty_session_keep_channel` for a daemon-backed
    /// (persistent) session must PRESERVE `output_subscribers` and
    /// `pending_output` so the frontend's xterm stays attached
    /// across the kill-and-respawn that happens on workspace push.
    /// Without this, the respawned PTY's output buffers in
    /// `pending_output` until a tab-switch forces re-attach.
    #[tokio::test]
    async fn terminate_keep_channel_preserves_channel_for_persistent_session() {
        let sessions = make_sessions();
        let client =
            crate::pty_daemon::PtyDaemonClient::new_for_test_arc_identity().await;
        let starting_payload = TerminalStatusPayload {
            session_id: "session-X".into(),
            state: TerminalLifecycleState::Ready,
            message: Some("ready".into()),
            exit_code: None,
        };
        {
            let mut guard = sessions.lock().unwrap();
            let mut rt = SessionRuntime::new("session-X");
            rt.persistent = true;
            rt.daemon_client = Some(client.clone());
            rt.child_pid = Some(12345);
            rt.last_status = starting_payload.clone();
            // Stash some pending output to verify it survives.
            rt.pending_output.push_back(b"prior\n".to_vec());
            rt.pending_output_bytes = 6;
            guard.insert("session-X".into(), rt);
        }

        terminate_pty_session_keep_channel(&sessions, "session-X");
        // Give the spawned tokio task a tick to run, even though
        // we're not asserting on its side-effects.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let guard = sessions.lock().unwrap();
        let rt = guard
            .get("session-X")
            .expect("runtime must still exist (the whole point of keep_channel)");
        assert!(
            rt.daemon_client.is_none(),
            "daemon_client must be taken (old client is dead)"
        );
        assert!(rt.writer.is_none(), "writer must be cleared");
        assert!(rt.child_pid.is_none(), "child_pid must be cleared");
        assert!(!rt.persistent, "persistent flag must flip false so try_reserve sees idle");
        assert!(!rt.is_spawning, "is_spawning must be false");
        // The critical preservation property:
        assert_eq!(
            rt.pending_output.len(),
            1,
            "pending_output must be preserved — clearing it loses any output \
             that arrived between terminate and the frontend's next attach"
        );
        assert!(
            matches!(rt.last_status.state, TerminalLifecycleState::Ready),
            "last_status must be preserved (don't overwrite the existing \
             lifecycle state with a synthetic Exited; the respawn will emit \
             its own Starting → Ready)"
        );
    }
    } // mod cross_machine_push

    // ── Shell + PATH tests ───────────────────────────────────────────
    //
    // `path_separator` and `prepend_shim_to_path` are cross-platform
    // helpers, so their tests run on every platform. `default_shell` on
    // Windows returns a Windows-specific value, so that test is
    // `#[cfg(windows)]`-gated and only executes on Windows CI.

    #[test]
    fn test_path_separator_matches_host_os() {
        let sep = path_separator();
        #[cfg(windows)]
        assert_eq!(sep, ";");
        #[cfg(unix)]
        assert_eq!(sep, ":");
    }

    #[test]
    fn test_prepend_shim_to_path_with_existing_path() {
        // Typical case: PATH has entries, shim gets prepended with
        // the OS-correct separator.
        let result = prepend_shim_to_path("/opt/codemux/shims", "/usr/bin:/bin");
        #[cfg(unix)]
        assert_eq!(result, "/opt/codemux/shims:/usr/bin:/bin");
        #[cfg(windows)]
        assert_eq!(result, "/opt/codemux/shims;/usr/bin:/bin");
    }

    #[test]
    fn test_prepend_shim_to_path_with_empty_current() {
        // Edge case: PATH is empty or unset. Result must be the
        // shim_dir with NO trailing separator — an empty trailing
        // component would make the child process try to resolve
        // binaries from the empty path (which is CWD on some shells,
        // a security hazard).
        let result = prepend_shim_to_path("/opt/codemux/shims", "");
        assert_eq!(result, "/opt/codemux/shims");
        assert!(
            !result.ends_with(':') && !result.ends_with(';'),
            "empty current_path must not produce a trailing separator"
        );
    }

    #[test]
    fn test_prepend_shim_to_path_windows_style_paths() {
        // Verify the Windows-style paths pass through unmodified on
        // Windows. The function is oblivious to path content — it
        // only joins with the separator.
        let result = prepend_shim_to_path(
            r"C:\Users\zeus\AppData\Local\Codemux\shims",
            r"C:\Windows\System32;C:\Program Files\Git\bin",
        );
        #[cfg(windows)]
        assert_eq!(
            result,
            r"C:\Users\zeus\AppData\Local\Codemux\shims;C:\Windows\System32;C:\Program Files\Git\bin"
        );
        #[cfg(unix)]
        {
            // On Unix the separator is `:`, so the Windows paths get
            // joined with a colon. That's fine — this is just exercising
            // the function's behavior under test, not a production path.
            assert_eq!(
                result,
                r"C:\Users\zeus\AppData\Local\Codemux\shims:C:\Windows\System32;C:\Program Files\Git\bin"
            );
        }
    }

    // ── build_child_path + Windows user-local-bin tests ──────────────
    //
    // `build_child_path` is the cross-platform wrapper used by both
    // `spawn_pty_for_session`. On Unix it's
    // a pass-through to `prepend_shim_to_path`. On Windows it *also*
    // prepends `%USERPROFILE%\.local\bin` so Claude Code (installed
    // via `irm claude.ai/install.ps1 | iex`) and similar per-user
    // CLIs are discoverable even if Codemux's own PATH missed the
    // installer's `WM_SETTINGCHANGE` broadcast.

    #[cfg(unix)]
    #[test]
    fn test_build_child_path_unix_is_passthrough() {
        // On Unix, build_child_path must match prepend_shim_to_path
        // exactly — there's no additional prepend. Unix installers
        // update shell rc files, so PATH propagation is handled by
        // the user's login shell, not by Codemux.
        let a = build_child_path("/opt/codemux/shims", "/usr/bin:/bin");
        let b = prepend_shim_to_path("/opt/codemux/shims", "/usr/bin:/bin");
        assert_eq!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn test_build_child_path_unix_empty_current() {
        // Edge case mirror of the prepend test — no trailing separator
        // even when the pass-through chain is exercised.
        let result = build_child_path("/opt/codemux/shims", "");
        assert_eq!(result, "/opt/codemux/shims");
    }

    #[cfg(windows)]
    #[test]
    fn test_build_child_path_windows_prepends_user_local_bin() {
        // On Windows, the result must START with
        // `%USERPROFILE%\.local\bin` so that executables installed there
        // are preferred over anything else on PATH. The next component
        // is the codemux shim dir (from prepend_shim_to_path). The
        // remainder is the caller-supplied current_path.
        let result = build_child_path(r"C:\codemux\shims", r"C:\Windows\System32");
        let user_profile = std::env::var("USERPROFILE").expect("USERPROFILE must be set on Windows");
        let expected_local_bin = format!(r"{}\.local\bin", user_profile);
        assert!(
            result.starts_with(&expected_local_bin),
            "expected build_child_path to start with {expected_local_bin:?}, got {result:?}",
        );
        assert!(
            result.contains(r"C:\codemux\shims"),
            "shim dir must be preserved somewhere in the result, got {result:?}",
        );
        assert!(
            result.contains(r"C:\Windows\System32"),
            "original current_path must be preserved, got {result:?}",
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_user_local_bin_uses_userprofile() {
        // windows_user_local_bin() must resolve USERPROFILE +
        // `.local\bin` verbatim — no normalization, no extra logic,
        // just a composed path. We reconstruct what we expect and
        // verify the helper matches.
        let user_profile = std::env::var("USERPROFILE").expect("USERPROFILE must be set on Windows");
        let expected = format!(r"{}\.local\bin", user_profile);
        let actual = windows_user_local_bin().expect("USERPROFILE is set so this must return Some");
        assert_eq!(actual, expected);
    }

    #[cfg(windows)]
    #[test]
    fn test_default_shell_windows_returns_powershell_or_cmd() {
        // default_shell() on Windows probes PATH for pwsh → powershell
        // (always pre-installed on Windows 10+ / Server 2016+) and only
        // falls back to COMSPEC / "cmd.exe" if neither PowerShell binary
        // is present. On a stock Windows runner the result will almost
        // always be powershell.exe; on a runner with PowerShell 7 it
        // will be pwsh.exe. It should never return a Unix shell path.
        let shell = default_shell();
        let lower = shell.to_lowercase();
        assert!(
            lower.ends_with("pwsh.exe")
                || lower.ends_with("powershell.exe")
                || lower.ends_with("cmd.exe"),
            "Windows default_shell should end with pwsh.exe, powershell.exe, or cmd.exe, got: {shell}"
        );
        assert!(
            !shell.contains("/bin/"),
            "Windows default_shell must not return a Unix shell path, got: {shell}"
        );
    }

    // ── resolve_windows_shell fallback chain ─────────────────────────
    //
    // These tests exercise the pure-function version of the Windows
    // shell resolver directly, so they run on every platform (the
    // function is compiled under `cfg(any(windows, test))`). Each test
    // injects a fake PATH resolver closure so the fallback order can be
    // verified without relying on what's actually installed on the
    // runner.

    #[test]
    fn test_resolve_windows_shell_prefers_pwsh_when_available() {
        // When both pwsh and powershell are on PATH, pwsh wins — it's
        // the newer cross-platform PowerShell.
        let result = resolve_windows_shell(
            |cmd| match cmd {
                "pwsh" => Some(r"C:\Program Files\PowerShell\7\pwsh.exe".to_string()),
                "powershell" => Some(
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_string(),
                ),
                _ => None,
            },
            Some(r"C:\Windows\System32\cmd.exe".to_string()),
        );
        assert_eq!(result, r"C:\Program Files\PowerShell\7\pwsh.exe");
    }

    #[test]
    fn test_resolve_windows_shell_falls_back_to_powershell_when_pwsh_missing() {
        // pwsh absent (PowerShell 7 not installed) → pick powershell.exe
        // (Windows PowerShell 5.1 is always present on modern Windows).
        let result = resolve_windows_shell(
            |cmd| match cmd {
                "powershell" => Some(
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_string(),
                ),
                _ => None,
            },
            Some(r"C:\Windows\System32\cmd.exe".to_string()),
        );
        assert_eq!(
            result,
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        );
    }

    #[test]
    fn test_resolve_windows_shell_falls_back_to_comspec_when_no_powershell() {
        // Hypothetical: neither PowerShell nor pwsh on PATH (e.g. a
        // stripped-down Windows container). Fall back to COMSPEC.
        let result = resolve_windows_shell(
            |_| None,
            Some(r"C:\Windows\System32\cmd.exe".to_string()),
        );
        assert_eq!(result, r"C:\Windows\System32\cmd.exe");
    }

    #[test]
    fn test_resolve_windows_shell_final_fallback_to_literal_cmd_exe() {
        // Last-resort: no PowerShell, no COMSPEC → literal "cmd.exe"
        // relative name so CreateProcessW can still resolve it.
        let result = resolve_windows_shell(|_| None, None);
        assert_eq!(result, "cmd.exe");
    }

    #[test]
    fn test_resolve_windows_shell_treats_blank_comspec_as_unset() {
        // COMSPEC set to whitespace is treated as unset — matches the
        // historic guard in the old default_shell() implementation.
        let result = resolve_windows_shell(|_| None, Some("   ".to_string()));
        assert_eq!(result, "cmd.exe");
    }

    #[test]
    fn test_resolve_windows_shell_pwsh_beats_comspec() {
        // Even if COMSPEC is set to something weird, pwsh still wins —
        // the whole point of the refactor is that we prefer modern
        // shells over cmd.exe whenever available.
        let result = resolve_windows_shell(
            |cmd| match cmd {
                "pwsh" => Some(r"C:\Program Files\PowerShell\7\pwsh.exe".to_string()),
                _ => None,
            },
            Some(r"C:\some\custom\cmd.exe".to_string()),
        );
        assert_eq!(result, r"C:\Program Files\PowerShell\7\pwsh.exe");
    }

    #[cfg(unix)]
    #[test]
    fn test_default_shell_unix_returns_bash_or_shell_env() {
        // Paired guard for the Unix path: default_shell() on Unix reads
        // $SHELL and falls back to `/bin/bash`. It should never return
        // a .exe suffix or a Windows-style path.
        let shell = default_shell();
        assert!(
            shell.starts_with('/') || shell.is_empty() == false,
            "Unix default_shell should be an absolute path, got: {shell}"
        );
        assert!(
            !shell.to_lowercase().ends_with(".exe"),
            "Unix default_shell must not return a .exe binary, got: {shell}"
        );
    }

    #[test]
    fn test_failed_status_does_not_create_ghost_entry() {
        let sessions = make_sessions();

        // with_existing_session_runtime should not create an entry
        let result = with_existing_session_runtime(&sessions, "ghost-session", |runtime| {
            runtime.last_status.state = TerminalLifecycleState::Failed;
        });

        assert!(
            result.is_none(),
            "should not create entry for non-existent session"
        );
        let guard = sessions.lock().unwrap();
        assert!(
            !guard.contains_key("ghost-session"),
            "no ghost entry should exist"
        );
    }

    #[test]
    fn test_starting_status_creates_entry() {
        let sessions = make_sessions();

        with_session_runtime(
            &sessions,
            "new-session",
            || SessionRuntime::new("new-session"),
            |runtime| {
                runtime.last_status = TerminalStatusPayload {
                    session_id: "new-session".to_string(),
                    state: TerminalLifecycleState::Starting,
                    message: Some("Starting shell...".into()),
                    exit_code: None,
                };
            },
        );

        let guard = sessions.lock().unwrap();
        assert!(
            guard.contains_key("new-session"),
            "Starting should create entry"
        );
    }

    #[test]
    fn test_get_session_pids_no_stale_after_cleanup() {
        let pty_state = PtyState::default();
        let sessions = pty_state.sessions.clone();

        with_session_runtime(
            &sessions,
            "sess-1",
            || SessionRuntime::new("sess-1"),
            |runtime| {
                runtime.child_pid = Some(12345);
            },
        );
        assert_eq!(pty_state.get_session_pids().len(), 1);

        // Simulate cleanup: update status then remove
        with_existing_session_runtime(&sessions, "sess-1", |runtime| {
            runtime.last_status.state = TerminalLifecycleState::Failed;
        });
        remove_session_runtime(&sessions, "sess-1");

        assert!(
            pty_state.get_session_pids().is_empty(),
            "no stale pids after cleanup"
        );
    }

    #[test]
    fn test_queue_or_send_output_buffers_chunks() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        queue_or_send_output(&sessions, "sess", vec![1, 2, 3]);
        queue_or_send_output(&sessions, "sess", vec![4, 5, 6]);

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), 2);
        assert_eq!(runtime.pending_output[0], vec![1, 2, 3]);
        assert_eq!(runtime.pending_output[1], vec![4, 5, 6]);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        for i in 0..OUTPUT_BUFFER_BYTE_LIMIT + 10 {
            queue_or_send_output(&sessions, "sess", vec![i as u8]);
        }

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), OUTPUT_BUFFER_BYTE_LIMIT);
        // Oldest 10 evicted; first remaining is chunk #10
        assert_eq!(runtime.pending_output[0], vec![10]);
    }

    #[test]
    fn test_flush_pty_batch_sends_and_resets() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        let mut batch = vec![1u8, 2, 3, 4, 5];
        let mut last_flush = Instant::now() - Duration::from_secs(1);

        flush_pty_batch(&sessions, "sess", &mut batch, &mut last_flush);

        assert!(batch.is_empty(), "batch should be cleared after flush");
        assert!(
            last_flush.elapsed() < Duration::from_millis(100),
            "last_flush should be recent"
        );

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), 1);
        assert_eq!(runtime.pending_output[0], vec![1, 2, 3, 4, 5]);
    }

    /// dropped_chunks is per-session, not global — eviction in session A
    /// must not increment session B's counter.
    #[test]
    fn test_dropped_chunks_isolated_per_session() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "a", || SessionRuntime::new("a"), |_| {});
        with_session_runtime(&sessions, "b", || SessionRuntime::new("b"), |_| {});

        for i in 0..OUTPUT_BUFFER_BYTE_LIMIT + 3 {
            queue_or_send_output(&sessions, "a", vec![i as u8]);
        }
        for i in 0..5 {
            queue_or_send_output(&sessions, "b", vec![i]);
        }

        let guard = sessions.lock().unwrap();
        assert_eq!(guard.get("a").unwrap().dropped_chunks, 3);
        assert_eq!(guard.get("b").unwrap().dropped_chunks, 0);
    }

    /// dropped_chunks does not reset across attach/detach cycles. A counter
    /// reset would obscure the cumulative regression signal.
    #[test]
    fn test_dropped_chunks_persists_across_attach_detach() {
        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "persist",
            || SessionRuntime::new("persist"),
            |_| {},
        );

        for i in 0..OUTPUT_BUFFER_BYTE_LIMIT + 7 {
            queue_or_send_output(&sessions, "persist", vec![i as u8]);
        }
        let after_first_overflow = {
            let guard = sessions.lock().unwrap();
            guard.get("persist").unwrap().dropped_chunks
        };
        assert_eq!(after_first_overflow, 7);

        // Simulate attach (mirrors attach_pty_output body): install a
        // subscriber, drain pending_output, then detach again. Reset the
        // byte counter alongside the deque to keep the invariant —
        // production code does this in `close_terminal_session`; tests
        // touching the deque directly must do the same.
        let channel: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        let generation = attach_subscriber(&sessions, "persist", channel);
        with_existing_session_runtime(&sessions, "persist", |runtime| {
            runtime.pending_output.clear();
            runtime.pending_output_bytes = 0;
        });
        detach_subscriber(&sessions, "persist", generation);

        // Counter should still reflect the prior overflow.
        let guard = sessions.lock().unwrap();
        assert_eq!(
            guard.get("persist").unwrap().dropped_chunks,
            7,
            "dropped_chunks must NOT reset on attach/detach"
        );
    }

    /// `dropped_chunks` increments exactly once per evicted chunk when the
    /// buffer overflows with no channel attached. This is the observability
    /// hook the cache architecture uses to detect a regression that would
    /// silently lose PTY data.
    #[test]
    fn test_dropped_chunks_counter_increments_on_overflow() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "drop", || SessionRuntime::new("drop"), |_| {});

        let extra = 5usize;
        for i in 0..OUTPUT_BUFFER_BYTE_LIMIT + extra {
            queue_or_send_output(&sessions, "drop", vec![i as u8]);
        }

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("drop").unwrap();
        assert_eq!(runtime.pending_output.len(), OUTPUT_BUFFER_BYTE_LIMIT);
        assert_eq!(
            runtime.dropped_chunks, extra as u64,
            "dropped_chunks should record every eviction, not just the first"
        );
    }

    /// The flush-on-attach mechanism is the load-bearing claim from the
    /// terminal-rendering analysis §6 step 2. With the cache architecture the
    /// channel stays attached for the session lifetime so this path is
    /// effectively cold-start-only — but the invariant must still hold,
    /// because cold start IS the only time the channel attaches and the
    /// pending_output between PTY spawn and first attach must reach the
    /// frontend in order, with no loss.
    ///
    /// Constructs a real `tauri::ipc::Channel` whose handler captures the
    /// payload bytes, mirrors the body of `attach_pty_output` directly
    /// (collecting `pending_output` then forwarding through the channel),
    /// and asserts the consumer received the full stream in order.
    #[test]
    fn test_pending_output_replays_on_attach() {
        use std::sync::Mutex as StdMutex;

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "replay",
            || SessionRuntime::new("replay"),
            |_| {},
        );

        // Queue bytes BEFORE any consumer exists — this is the "PTY ran
        // ahead of the frontend attach" window.
        for i in 0..10u8 {
            queue_or_send_output(&sessions, "replay", vec![i, i + 100]);
        }

        let captured: Arc<StdMutex<Vec<Vec<u8>>>> = Arc::new(StdMutex::new(Vec::new()));
        let captured_handler = captured.clone();
        let channel: Channel<Vec<u8>> = Channel::new(move |body| {
            let bytes = body.deserialize::<Vec<u8>>().expect("decode body");
            captured_handler.lock().unwrap().push(bytes);
            Ok(())
        });

        // Mirror the attach_pty_output Tauri command body directly: install
        // the subscriber, snapshot pending_output, then forward each chunk.
        let pending_chunks = with_session_runtime(
            &sessions,
            "replay",
            || SessionRuntime::new("replay"),
            |runtime| {
                runtime.next_output_generation =
                    runtime.next_output_generation.saturating_add(1);
                let generation = runtime.next_output_generation;
                runtime.output_subscribers.push(OutputSubscriber {
                    generation,
                    channel: channel.clone(),
                    flow_paused: false,
                });
                runtime.pending_output.iter().cloned().collect::<Vec<_>>()
            },
        );
        for chunk in pending_chunks {
            channel.send(chunk).expect("channel send");
        }

        let received = captured.lock().unwrap().clone();
        assert_eq!(received.len(), 10, "consumer should see every chunk");
        for (i, chunk) in received.iter().enumerate() {
            assert_eq!(
                chunk,
                &vec![i as u8, i as u8 + 100],
                "chunk {i} payload preserved end-to-end"
            );
        }
    }

    /// With the cache architecture, the channel stays attached for the full
    /// session lifetime. Once attached, every subsequent chunk produced by
    /// the reader thread (modeled here as direct `queue_or_send_output`
    /// calls) is forwarded to the consumer immediately. The pending_output
    /// buffer remains a bounded replay window; live delivery is separate and
    /// must not count normal ring-buffer eviction as dropped frontend output.
    #[test]
    fn test_attached_channel_receives_live_writes() {
        use std::sync::Mutex as StdMutex;

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "live",
            || SessionRuntime::new("live"),
            |_| {},
        );

        let captured: Arc<StdMutex<Vec<Vec<u8>>>> = Arc::new(StdMutex::new(Vec::new()));
        let captured_handler = captured.clone();
        let channel: Channel<Vec<u8>> = Channel::new(move |body| {
            let bytes = body.deserialize::<Vec<u8>>().expect("decode body");
            captured_handler.lock().unwrap().push(bytes);
            Ok(())
        });
        attach_subscriber(&sessions, "live", channel);

        // Drive the same path the reader thread uses for each batched flush.
        for i in 0..OUTPUT_BUFFER_BYTE_LIMIT + 5 {
            queue_or_send_output(&sessions, "live", vec![i as u8]);
        }

        let received = captured.lock().unwrap().clone();
        assert_eq!(
            received.len(),
            OUTPUT_BUFFER_BYTE_LIMIT + 5,
            "channel should receive every chunk synchronously when attached"
        );
        for (i, chunk) in received.iter().enumerate() {
            assert_eq!(chunk, &vec![i as u8]);
        }

        // pending_output is still appended to (the buffer is the source for
        // cold-start replay if the consumer ever reattaches), but the
        // channel-side delivery is the live path and never lossy. Verify
        // chunks are bounded by OUTPUT_BUFFER_BYTE_LIMIT and that no drops have
        // occurred — drops are the regression signal added in 2.6.
        let guard = sessions.lock().unwrap();
        let runtime = guard.get("live").unwrap();
        assert!(runtime.pending_output.len() <= OUTPUT_BUFFER_BYTE_LIMIT);
        assert_eq!(
            runtime.dropped_chunks, 0,
            "bounded replay eviction is not dropped live output while channel is attached"
        );
    }

    #[test]
    fn test_attached_channel_send_happens_outside_sessions_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "live-lock",
            || SessionRuntime::new("live-lock"),
            |_| {},
        );

        let send_observed_unlocked_mutex = Arc::new(AtomicBool::new(false));
        let observed = send_observed_unlocked_mutex.clone();
        let sessions_for_handler = sessions.clone();
        let channel: Channel<Vec<u8>> = Channel::new(move |_| {
            if sessions_for_handler.try_lock().is_ok() {
                observed.store(true, Ordering::SeqCst);
            }
            Ok(())
        });

        attach_subscriber(&sessions, "live-lock", channel);

        queue_or_send_output(&sessions, "live-lock", vec![42]);

        assert!(
            send_observed_unlocked_mutex.load(Ordering::SeqCst),
            "channel.send must not run while the global sessions mutex is held"
        );
    }

    /// Real `tauri::ipc::Channel` whose handler records every decoded chunk, in
    /// order. Shared shape used by the fan-out tests below.
    fn recording_channel() -> (Channel<Vec<u8>>, Arc<Mutex<Vec<Vec<u8>>>>) {
        let captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = captured.clone();
        let channel: Channel<Vec<u8>> = Channel::new(move |body| {
            let bytes = body.deserialize::<Vec<u8>>().expect("decode body");
            handler.lock().unwrap().push(bytes);
            Ok(())
        });
        (channel, captured)
    }

    /// Multi-client fan-out: two subscribers attached to the same session must
    /// BOTH receive every chunk, in the same interleaved order the reader
    /// produced them. This is the core mirror-mode delivery guarantee.
    #[test]
    fn test_fan_out_delivers_to_all_subscribers() {
        let sessions = make_sessions();
        let (ch_a, cap_a) = recording_channel();
        let (ch_b, cap_b) = recording_channel();
        attach_subscriber(&sessions, "fan", ch_a);
        attach_subscriber(&sessions, "fan", ch_b);

        for i in 0..8u8 {
            queue_or_send_output(&sessions, "fan", vec![i, i + 200]);
        }

        let a = cap_a.lock().unwrap().clone();
        let b = cap_b.lock().unwrap().clone();
        assert_eq!(a.len(), 8, "subscriber A must see every chunk");
        assert_eq!(b.len(), 8, "subscriber B must see every chunk");
        for i in 0..8u8 {
            assert_eq!(a[i as usize], vec![i, i + 200], "A order preserved");
            assert_eq!(b[i as usize], vec![i, i + 200], "B order preserved");
        }
    }

    /// Detaching one subscriber must leave the other streaming uninterrupted —
    /// removing A never touches B's delivery.
    #[test]
    fn test_detach_one_leaves_other_streaming() {
        let sessions = make_sessions();
        let (ch_a, cap_a) = recording_channel();
        let (ch_b, cap_b) = recording_channel();
        let gen_a = attach_subscriber(&sessions, "detach", ch_a);
        attach_subscriber(&sessions, "detach", ch_b);

        queue_or_send_output(&sessions, "detach", vec![1]);
        // Both saw the first chunk.
        assert_eq!(cap_a.lock().unwrap().len(), 1);
        assert_eq!(cap_b.lock().unwrap().len(), 1);

        detach_subscriber(&sessions, "detach", gen_a);
        // Only B remains a subscriber.
        with_existing_session_runtime(&sessions, "detach", |runtime| {
            assert_eq!(runtime.output_subscribers.len(), 1, "A removed");
        });

        queue_or_send_output(&sessions, "detach", vec![2]);
        queue_or_send_output(&sessions, "detach", vec![3]);

        assert_eq!(
            cap_a.lock().unwrap().clone(),
            vec![vec![1u8]],
            "detached A must receive nothing further"
        );
        assert_eq!(
            cap_b.lock().unwrap().clone(),
            vec![vec![1u8], vec![2u8], vec![3u8]],
            "B keeps streaming across A's detach"
        );
    }

    /// Replay-on-attach targets the NEW subscriber only: a late joiner catches
    /// up from the `pending_output` ring while the existing subscriber does not
    /// re-receive history it already streamed live.
    #[test]
    fn test_replay_on_attach_reaches_only_the_attacher() {
        let sessions = make_sessions();
        let (ch_a, cap_a) = recording_channel();
        attach_subscriber(&sessions, "join", ch_a);

        // Produce output while only A is attached — A gets it live, and it also
        // lands in the replay ring.
        for i in 0..5u8 {
            queue_or_send_output(&sessions, "join", vec![i]);
        }
        assert_eq!(cap_a.lock().unwrap().len(), 5, "A streamed 5 chunks live");

        // B attaches late. Mirror attach_pty_output: install B, snapshot
        // pending_output, replay to B's channel only.
        let (ch_b, cap_b) = recording_channel();
        let pending = with_session_runtime(
            &sessions,
            "join",
            || SessionRuntime::new("join"),
            |runtime| {
                runtime.next_output_generation =
                    runtime.next_output_generation.saturating_add(1);
                let generation = runtime.next_output_generation;
                runtime.output_subscribers.push(OutputSubscriber {
                    generation,
                    channel: ch_b.clone(),
                    flow_paused: false,
                });
                runtime.pending_output.iter().cloned().collect::<Vec<_>>()
            },
        );
        for chunk in pending {
            ch_b.send(chunk).expect("replay send");
        }

        // A must NOT have received the replay again (still 5 live chunks); B
        // catches up with exactly the ring's history.
        assert_eq!(
            cap_a.lock().unwrap().len(),
            5,
            "existing subscriber must not re-receive replayed history"
        );
        assert_eq!(
            cap_b.lock().unwrap().clone(),
            (0..5u8).map(|i| vec![i]).collect::<Vec<_>>(),
            "late joiner replays the full ring in order"
        );

        // A subsequent live chunk now reaches BOTH.
        queue_or_send_output(&sessions, "join", vec![99]);
        assert_eq!(*cap_a.lock().unwrap().last().unwrap(), vec![99u8]);
        assert_eq!(*cap_b.lock().unwrap().last().unwrap(), vec![99u8]);
    }

    /// A detach carrying a stale/superseded generation must not remove the
    /// current subscriber — the guard that keeps an unmount race from tearing
    /// down a newer attach.
    #[test]
    fn test_detach_with_stale_generation_is_noop() {
        let sessions = make_sessions();
        let (channel, captured) = recording_channel();
        let generation = attach_subscriber(&sessions, "stale-detach", channel);

        // Detach a generation that was never issued → nothing removed.
        detach_subscriber(&sessions, "stale-detach", generation + 1);
        with_existing_session_runtime(&sessions, "stale-detach", |runtime| {
            assert_eq!(
                runtime.output_subscribers.len(),
                1,
                "stale detach must not remove the live subscriber"
            );
        });

        // The live subscriber still streams.
        queue_or_send_output(&sessions, "stale-detach", vec![7]);
        assert_eq!(captured.lock().unwrap().clone(), vec![vec![7u8]]);

        // The matching generation does remove it.
        detach_subscriber(&sessions, "stale-detach", generation);
        with_existing_session_runtime(&sessions, "stale-detach", |runtime| {
            assert!(runtime.output_subscribers.is_empty(), "matching detach removes");
        });
    }

    #[test]
    fn test_flush_pty_batch_noop_on_empty() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        let mut batch: Vec<u8> = Vec::new();
        let original_time = Instant::now() - Duration::from_secs(10);
        let mut last_flush = original_time;

        flush_pty_batch(&sessions, "sess", &mut batch, &mut last_flush);

        assert_eq!(
            last_flush, original_time,
            "last_flush should not change on empty batch"
        );
        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(
            runtime.pending_output.len(),
            0,
            "no output should be queued"
        );
    }

    /// Verify that data written to a PTY appears in pending_output within
    /// PTY_BATCH_INTERVAL even when no further writes occur. This tests the
    /// poll()-based flush timeout in batched_reader_loop.
    #[cfg(unix)]
    #[test]
    fn test_batch_flushes_on_timeout_without_further_writes() {
        use portable_pty::{native_pty_system, PtySize};

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "test-pty",
            || SessionRuntime::new("test-pty"),
            |_| {},
        );

        // Open a real PTY pair.
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 2,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let poll_fd = pair.master.as_raw_fd().expect("no raw fd");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut writer = pair.master.take_writer().expect("take writer");

        // Start the batched reader loop in a background thread.
        let read_sessions = sessions.clone();
        let flow_paused = Arc::new(AtomicBool::new(false));
        let reader_handle = std::thread::spawn(move || {
            batched_reader_loop(
                &mut reader,
                poll_fd,
                &read_sessions,
                "test-pty",
                &flow_paused,
                |_| {},
            );
        });

        // Write a small payload — well below PTY_BATCH_SIZE.
        //
        // IMPORTANT: do NOT drop `pair.slave` anywhere in this test. Keep
        // it alive for the full duration and let it drop naturally at
        // end-of-scope once `reader_handle.join()` has returned.
        //
        // Why: the reader thread reads from `pair.master`'s read side,
        // which on Linux receives data via the PTY's line discipline ECHO
        // behavior — bytes written to `master.writer` → slave's input →
        // line discipline echoes them back → `master.reader` returns them.
        // The line discipline is attached to the master/slave pair; if
        // we close the slave end while the reader is still running, the
        // discipline can transition into a "no slave" state where echo
        // stops working and `master.reader` either returns stale EOF or
        // never delivers the echoed bytes. GitHub Actions' ubuntu-latest
        // kernel hits this case; local dev machines don't, which is why
        // three prior fix attempts (widened sleep → polling loop → drop
        // after write) all passed locally and still failed on CI.
        //
        // The ONLY correct sequence is: keep slave open → write → let
        // echo deliver → read in background thread → assert pending_output
        // has the bytes → drop writer → join reader (reader sees EOF on
        // writer drop because the line discipline is still alive and
        // propagates it properly) → function returns → slave drops last.
        let payload = b"hello from pty test\r\n";
        writer.write_all(payload).expect("write failed");
        writer.flush().expect("flush failed");

        // Poll for up to `deadline` waiting for the batched reader thread to
        // flush. The key assertion is that data arrives WITHOUT another write
        // — not that it arrives in exactly 16ms. Previous iterations used a
        // single `sleep(PTY_BATCH_INTERVAL + 200ms)` which kept tripping on
        // loaded CI runners where thread scheduling jitter blew past the
        // margin. Polling gives fast machines a typical finish time of
        // ~16-50ms while still tolerating up to 3s of CI jitter — strictly
        // more headroom than a fixed sleep at no cost to local dev runs.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            {
                let guard = sessions.lock().unwrap();
                let runtime = guard.get("test-pty").unwrap();
                if !runtime.pending_output.is_empty() {
                    found = true;
                    break;
                }
            }
        }
        assert!(
            found,
            "data should appear in pending_output within 3s (PTY_BATCH_INTERVAL = {PTY_BATCH_INTERVAL:?})"
        );

        // Verify the content includes our payload.
        let content = {
            let guard = sessions.lock().unwrap();
            let runtime = guard.get("test-pty").unwrap();
            runtime
                .pending_output
                .iter()
                .flat_map(|c| c.iter().copied())
                .collect::<Vec<u8>>()
        };
        let content_str = String::from_utf8_lossy(&content);
        assert!(
            content_str.contains("hello from pty test"),
            "pending_output should contain the written payload, got: {content_str:?}"
        );

        // Clean up: the reader thread is blocked in poll()/read() on the
        // master's read side and will stay blocked until SOMETHING signals
        // EOF. Dropping `writer` alone is not enough — on Linux PTYs,
        // master.writer and master.reader are independent halves, so
        // closing the write side doesn't propagate EOF to the read side.
        // The slave is what we need to close: dropping `pair.slave` here
        // tears down the slave end of the PTY, which causes the kernel to
        // mark the master's read side with POLLHUP. The reader thread's
        // `poll_read_ready` returns true, `reader.read()` returns Ok(0),
        // the batched loop hits its `Ok(_) => break` branch, and the
        // thread exits cleanly — which is what `reader_handle.join()`
        // needs to return.
        //
        // Sequence matters: slave MUST stay alive until AFTER the payload
        // has been observed in pending_output (see the long comment above
        // the write block), but MUST be dropped before we try to join the
        // reader. Do both in the right order here.
        drop(pair.slave);
        drop(writer);
        reader_handle.join().expect("reader thread panicked");
    }

    /// In-process flow control: while `flow_paused` is set the batched reader
    /// loop must NOT drain the PTY master fd (so the kernel buffer fills and the
    /// child blocks on write — real back-pressure), and once cleared it must
    /// resume and deliver the buffered output. This is the in-process analogue
    /// of the daemon's flow gate and the core of issue #73's backend change.
    ///
    /// Shares the careful "keep slave alive until after the assert" line-
    /// discipline sequencing documented at length in
    /// `test_batch_flushes_on_timeout_without_further_writes`.
    #[cfg(unix)]
    #[test]
    fn test_in_process_flow_control_pauses_and_resumes_reader() {
        use portable_pty::{native_pty_system, PtySize};

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "flow-pty",
            || SessionRuntime::new("flow-pty"),
            |_| {},
        );

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 2,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let poll_fd = pair.master.as_raw_fd().expect("no raw fd");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut writer = pair.master.take_writer().expect("take writer");

        // Start PAUSED so the loop parks on its very first iteration.
        let flow_paused = Arc::new(AtomicBool::new(true));
        let read_sessions = sessions.clone();
        let read_flow_paused = flow_paused.clone();
        let reader_handle = std::thread::spawn(move || {
            batched_reader_loop(
                &mut reader,
                poll_fd,
                &read_sessions,
                "flow-pty",
                &read_flow_paused,
                |_| {},
            );
        });

        // Write a tiny payload (well under the kernel PTY buffer, so the write
        // itself never blocks). The line discipline echoes it into the master's
        // read side, where it sits because the loop is parked.
        let payload = b"paused output should not drain\r\n";
        writer.write_all(payload).expect("write failed");
        writer.flush().expect("flush failed");

        // While paused, the bytes must NOT reach pending_output. Wait well past
        // PTY_BATCH_INTERVAL so a non-gated loop would certainly have flushed.
        std::thread::sleep(Duration::from_millis(250));
        {
            let guard = sessions.lock().unwrap();
            let runtime = guard.get("flow-pty").unwrap();
            assert!(
                runtime.pending_output.is_empty(),
                "paused reader must not drain the master fd, but pending_output \
                 had {} chunk(s)",
                runtime.pending_output.len()
            );
        }

        // Resume — the buffered echo should now drain into pending_output.
        flow_paused.store(false, Ordering::Relaxed);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            {
                let guard = sessions.lock().unwrap();
                let runtime = guard.get("flow-pty").unwrap();
                if !runtime.pending_output.is_empty() {
                    found = true;
                    break;
                }
            }
        }
        assert!(
            found,
            "after resume, buffered output should drain into pending_output within 3s"
        );

        let content = {
            let guard = sessions.lock().unwrap();
            let runtime = guard.get("flow-pty").unwrap();
            runtime
                .pending_output
                .iter()
                .flat_map(|c| c.iter().copied())
                .collect::<Vec<u8>>()
        };
        let content_str = String::from_utf8_lossy(&content);
        assert!(
            content_str.contains("paused output should not drain"),
            "resumed output should contain the written payload, got: {content_str:?}"
        );

        // Tear down (see the sibling test for why slave must drop before join).
        drop(pair.slave);
        drop(writer);
        reader_handle.join().expect("reader thread panicked");
    }

    /// End-to-end firehose: a real `yes` producer in a real PTY, the real
    /// `batched_reader_loop`, and a real `tauri::ipc::Channel` consumer — the
    /// full in-process path issue #73 targets. Exercises every acceptance
    /// criterion at the backend level:
    ///   1. with a consumer attached, `pending_output` stays bounded and
    ///      `dropped_chunks` stays 0 throughout;
    ///   2. pausing stops bytes reaching the consumer (the producer blocks on
    ///      write — real back-pressure) and resuming delivers again, with no
    ///      permanent stall;
    ///   3. all of this on an in-process (non-daemon) session.
    #[cfg(unix)]
    #[test]
    fn test_firehose_backpressure_bounds_backend_with_consumer_attached() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::sync::atomic::AtomicUsize;

        let sessions = make_sessions();
        with_session_runtime(&sessions, "fire", || SessionRuntime::new("fire"), |_| {});

        // Real PTY + a real `yes` firehose writing into the slave.
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut child = pair
            .slave
            .spawn_command(CommandBuilder::new("yes"))
            .expect("spawn yes");

        let poll_fd = pair.master.as_raw_fd().expect("raw fd");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");

        // Real Channel consumer: count the bytes it receives. Installing it on
        // the runtime means `queue_or_send_output` forwards every chunk to it
        // (and never counts a drop while a channel is attached).
        let received = Arc::new(AtomicUsize::new(0));
        let received_handler = received.clone();
        let channel: Channel<Vec<u8>> = Channel::new(move |body| {
            let bytes = body.deserialize::<Vec<u8>>().expect("decode body");
            received_handler.fetch_add(bytes.len(), Ordering::SeqCst);
            Ok(())
        });

        // Start unpaused; install the subscriber and grab the shared flow flag
        // (mirrors the spawn + attach paths).
        attach_subscriber(&sessions, "fire", channel);
        let flow_paused = with_existing_session_runtime(&sessions, "fire", |runtime| {
            runtime.flow_paused.clone()
        })
        .expect("runtime exists");

        let read_sessions = sessions.clone();
        let read_flow_paused = flow_paused.clone();
        let reader_handle = std::thread::spawn(move || {
            batched_reader_loop(
                &mut reader,
                poll_fd,
                &read_sessions,
                "fire",
                &read_flow_paused,
                |_| {},
            );
        });

        // Helper: assert the backend never lets the ring overflow into drops
        // and never exceeds the byte cap, with a consumer attached.
        let assert_bounded = |sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>| {
            let guard = sessions.lock().unwrap();
            let rt = guard.get("fire").unwrap();
            assert_eq!(
                rt.dropped_chunks, 0,
                "with a consumer attached the dropped-chunk counter must stay 0"
            );
            assert!(
                rt.pending_output_bytes <= OUTPUT_BUFFER_BYTE_LIMIT,
                "pending_output must stay bounded by the ring cap, got {} > {}",
                rt.pending_output_bytes,
                OUTPUT_BUFFER_BYTE_LIMIT
            );
        };

        // ── Phase A: running — the firehose reaches the consumer. ──
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while received.load(Ordering::SeqCst) < 64 * 1024
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            received.load(Ordering::SeqCst) >= 64 * 1024,
            "consumer should receive the firehose while running (got {} bytes)",
            received.load(Ordering::SeqCst)
        );
        assert_bounded(&sessions);

        // ── Phase B: paused — back-pressure stops delivery entirely. ──
        flow_paused.store(true, Ordering::Relaxed);
        // Let any in-flight read/flush settle, then snapshot.
        std::thread::sleep(Duration::from_millis(200));
        let at_pause = received.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        let after_pause = received.load(Ordering::SeqCst);
        assert_eq!(
            after_pause, at_pause,
            "while paused the consumer must receive NOTHING further (producer is \
             blocked on write) — got {at_pause} then {after_pause}"
        );
        assert_bounded(&sessions);

        // ── Phase C: resumed — delivery continues, no permanent stall. ──
        flow_paused.store(false, Ordering::Relaxed);
        let resume_deadline = std::time::Instant::now() + Duration::from_secs(5);
        while received.load(Ordering::SeqCst) <= after_pause
            && std::time::Instant::now() < resume_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            received.load(Ordering::SeqCst) > after_pause,
            "consumer must receive again after resume (no permanent stall): \
             {after_pause} then {}",
            received.load(Ordering::SeqCst)
        );
        assert_bounded(&sessions);

        // ── Teardown ── kill the firehose, ensure the loop isn't parked, then
        // let it observe EOF and exit so the join returns.
        flow_paused.store(false, Ordering::Relaxed);
        let _ = child.kill();
        let _ = child.wait();
        drop(pair.slave);
        reader_handle.join().expect("reader thread panicked");
    }

    /// `set_pty_flow_paused` must flip the in-process `flow_paused` flag for a
    /// single-subscriber session with no daemon client — i.e. pause/resume is
    /// not a no-op on the in-process path (issue #73 acceptance criterion). A
    /// fresh reattach then clears it (self-heal: a new unpaused subscriber can't
    /// leave "all subscribers paused" true).
    #[test]
    fn test_set_pty_flow_paused_toggles_in_process_flag() {
        let sessions = make_sessions();
        // No daemon_client set → this is an in-process session.
        let channel: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        let generation = attach_subscriber(&sessions, "inproc", channel);
        let flag = with_existing_session_runtime(&sessions, "inproc", |runtime| {
            runtime.flow_paused.clone()
        })
        .expect("runtime exists");
        assert!(
            !flag.load(Ordering::Relaxed),
            "a fresh subscriber starts unpaused"
        );

        set_pty_flow_paused(&sessions, "inproc", Some(generation), true);
        assert!(
            flag.load(Ordering::Relaxed),
            "pausing the only subscriber must set the in-process flow_paused flag"
        );

        set_pty_flow_paused(&sessions, "inproc", Some(generation), false);
        assert!(
            !flag.load(Ordering::Relaxed),
            "resume must clear the in-process flow_paused flag"
        );

        // Re-pause, then simulate a reattach: a fresh unpaused subscriber makes
        // "all subscribers paused" false, so the derived flag clears.
        set_pty_flow_paused(&sessions, "inproc", Some(generation), true);
        assert!(flag.load(Ordering::Relaxed));
        let channel2: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        attach_subscriber(&sessions, "inproc", channel2);
        assert!(
            !flag.load(Ordering::Relaxed),
            "a fresh attach must clear a stale pause (new subscriber is unpaused)"
        );
    }

    /// A subscriber pausing alone must NOT park the reader while another
    /// subscriber is still caught up — the fan-out only parks when EVERY
    /// subscriber has paused. This is the core multi-client flow-control rule.
    #[test]
    fn test_flow_parks_only_when_all_subscribers_paused() {
        let sessions = make_sessions();
        let ch_a: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        let ch_b: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        let gen_a = attach_subscriber(&sessions, "multi", ch_a);
        let gen_b = attach_subscriber(&sessions, "multi", ch_b);
        let flag = with_existing_session_runtime(&sessions, "multi", |runtime| {
            runtime.flow_paused.clone()
        })
        .expect("runtime exists");

        // A pauses alone → B still unpaused → reader must NOT park.
        set_pty_flow_paused(&sessions, "multi", Some(gen_a), true);
        assert!(
            !flag.load(Ordering::Relaxed),
            "one paused subscriber must not park the reader while another is live"
        );

        // B also pauses → all paused → reader parks.
        set_pty_flow_paused(&sessions, "multi", Some(gen_b), true);
        assert!(
            flag.load(Ordering::Relaxed),
            "reader must park once every subscriber has paused"
        );

        // A resumes → no longer all paused → reader resumes.
        set_pty_flow_paused(&sessions, "multi", Some(gen_a), false);
        assert!(
            !flag.load(Ordering::Relaxed),
            "reader must resume as soon as any subscriber resumes"
        );

        // Detaching the still-paused B (with A unpaused) leaves the reader
        // running; detaching A too (leaving only paused... none) → empty set
        // never parks.
        set_pty_flow_paused(&sessions, "multi", Some(gen_a), true);
        assert!(flag.load(Ordering::Relaxed), "both paused → parked");
        detach_subscriber(&sessions, "multi", gen_b);
        assert!(
            flag.load(Ordering::Relaxed),
            "only the paused subscriber remains → still parked"
        );
        detach_subscriber(&sessions, "multi", gen_a);
        assert!(
            !flag.load(Ordering::Relaxed),
            "no subscribers left → reader must run (empty set never parks)"
        );
    }

    /// `set_pty_flow_paused` on a missing session is a benign no-op (the session
    /// may have just exited between the renderer's watermark check and the IPC).
    #[test]
    fn test_set_pty_flow_paused_missing_session_is_noop() {
        let sessions = make_sessions();
        // Must not panic / must not create a phantom runtime.
        set_pty_flow_paused(&sessions, "ghost", Some(1), true);
        let guard = sessions.lock().unwrap();
        assert!(
            guard.get("ghost").is_none(),
            "flow control on a missing session must not materialize a runtime"
        );
    }

    /// A stale `generation` (a subscriber that already detached) must not toggle
    /// the flag for the surviving subscribers.
    #[test]
    fn test_set_pty_flow_paused_stale_generation_is_noop() {
        let sessions = make_sessions();
        let channel: Channel<Vec<u8>> = Channel::new(|_| Ok(()));
        let generation = attach_subscriber(&sessions, "stale", channel);
        let flag = with_existing_session_runtime(&sessions, "stale", |runtime| {
            runtime.flow_paused.clone()
        })
        .expect("runtime exists");

        // Pause a generation that was never installed → no subscriber matches →
        // recompute leaves the live (unpaused) subscriber flowing.
        set_pty_flow_paused(&sessions, "stale", Some(generation + 999), true);
        assert!(
            !flag.load(Ordering::Relaxed),
            "a pause aimed at a non-existent generation must not park the reader"
        );
    }

    /// The `FLOW_MAX_PARK` backstop must force-resume a reader that was paused
    /// and never resumed, so a wedged/crashed renderer can't block the child
    /// forever. We can't wait the full 10s in a unit test, so this asserts the
    /// invariant structurally: the constant is positive and the poll cadence is
    /// shorter than the cap (a parked loop re-checks the flag and the deadline
    /// many times before force-resuming).
    #[test]
    fn test_flow_max_park_backstop_constants_are_sane() {
        assert!(
            FLOW_MAX_PARK > Duration::ZERO,
            "max-park backstop must be a positive duration"
        );
        assert!(
            FLOW_PARK_POLL > Duration::ZERO && FLOW_PARK_POLL < FLOW_MAX_PARK,
            "park poll cadence must be positive and well under the max-park cap"
        );
    }

    // -- workspace_pty_env tests -------------------------------------------------

    fn test_workspace(
        id: &str,
        title: &str,
        cwd: &str,
        git_branch: Option<&str>,
        worktree_path: Option<&str>,
        project_root: Option<&str>,
    ) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        WorkspaceSnapshot {
            workspace_id: WorkspaceId(id.to_string()),
            is_git: true,
            title: title.to_string(),
            workspace_type: WorkspaceType::Standard,
            cwd: cwd.to_string(),
            git_branch: git_branch.map(String::from),
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            latest_agent_state: None,
            worktree_path: worktree_path.map(String::from),
            project_root: project_root.map(String::from),
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: None,
            last_visited_at: None,
        }
    }

    fn env_map(ws: &crate::state::WorkspaceSnapshot) -> std::collections::HashMap<String, String> {
        workspace_pty_env(ws).into_iter().collect()
    }

    #[test]
    fn pty_env_sets_worktree_vars() {
        let ws = test_workspace(
            "ws-123",
            "my-feature",
            "/home/user/.codemux/worktrees/repo/my-feature",
            Some("feat/my-feature"),
            Some("/home/user/.codemux/worktrees/repo/my-feature"),
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);

        assert_eq!(m["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(
            m["CODEMUX_WORKSPACE_PATH"],
            "/home/user/.codemux/worktrees/repo/my-feature"
        );
        assert_eq!(m["CODEMUX_BRANCH"], "feat/my-feature");
        assert!(m.contains_key("CODEMUX_PORT"));
        assert_eq!(m["CODEMUX_WORKSPACE_NAME"], "my-feature");
    }

    #[test]
    fn pty_env_omits_branch_when_none() {
        let ws = test_workspace(
            "ws-456",
            "main",
            "/home/user/projects/repo",
            None,
            None,
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);

        assert!(!m.contains_key("CODEMUX_BRANCH"));
        assert_eq!(m["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(m["CODEMUX_WORKSPACE_PATH"], "/home/user/projects/repo");
    }

    #[test]
    fn pty_env_resolves_root_from_cwd_when_project_root_missing() {
        // When project_root is None, resolve_root_path falls back to cwd
        // if no .git directory is found.
        let ws = test_workspace("ws-789", "test-ws", "/tmp/no-git-here", None, None, None);
        let m = env_map(&ws);

        assert_eq!(m["CODEMUX_ROOT_PATH"], "/tmp/no-git-here");
    }

    #[test]
    fn pty_env_port_is_deterministic() {
        let ws1 = test_workspace("ws-abc", "ws", "/tmp", None, None, None);
        let ws2 = test_workspace("ws-abc", "ws", "/tmp", None, None, None);
        let m1 = env_map(&ws1);
        let m2 = env_map(&ws2);

        assert_eq!(m1["CODEMUX_PORT"], m2["CODEMUX_PORT"]);
    }

    #[test]
    fn pty_env_agent_context_contains_workspace_info() {
        let ws = test_workspace(
            "ws-123",
            "analyze-db",
            "/home/zeus/.codemux/worktrees/proj/analyze-db",
            Some("analyze-db"),
            Some("/home/zeus/.codemux/worktrees/proj/analyze-db"),
            Some("/home/zeus/projects/proj"),
        );
        let m = env_map(&ws);
        let ctx = &m["CODEMUX_AGENT_CONTEXT"];

        assert!(ctx.contains("Your workspace: analyze-db"));
        assert!(ctx.contains("Your working directory: /home/zeus/.codemux/worktrees/proj/analyze-db"));
        assert!(ctx.contains("Your branch: analyze-db"));
        assert!(ctx.contains("Original repo (reference only): /home/zeus/projects/proj"));
        assert!(ctx.contains("Do NOT create additional git worktrees"));
        assert!(ctx.contains("Do NOT cd to the original repo path"));
    }

    #[test]
    fn pty_env_agent_context_omits_worktree_when_not_worktree() {
        let ws = test_workspace(
            "ws-456",
            "main",
            "/home/user/projects/repo",
            Some("main"),
            None,
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);
        let ctx = &m["CODEMUX_AGENT_CONTEXT"];

        assert!(ctx.contains("Your workspace: main"));
        assert!(!ctx.contains("Your worktree:"));
        assert!(ctx.contains("Your branch: main"));
        assert!(ctx.contains("codemux browser"));
    }

    #[test]
    fn pty_env_always_includes_agent_context() {
        // Even with minimal workspace info, CODEMUX_AGENT_CONTEXT is present
        let ws = test_workspace("ws-min", "ws", "/tmp", None, None, None);
        let m = env_map(&ws);

        assert!(m.contains_key("CODEMUX_AGENT_CONTEXT"));
        assert!(m["CODEMUX_AGENT_CONTEXT"].contains("Codemux"));
        assert!(m["CODEMUX_AGENT_CONTEXT"].contains("codemux browser"));
    }

    // ── Bug 1: find_owning_workspace tests ────────────────────────

    /// Build a workspace with a single terminal pane in its surface tree.
    fn test_workspace_with_pane(
        id: &str,
        title: &str,
        cwd: &str,
        git_branch: Option<&str>,
        worktree_path: Option<&str>,
        project_root: Option<&str>,
        session_id: &str,
    ) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        let mut ws = test_workspace(id, title, cwd, git_branch, worktree_path, project_root);
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId(format!("surf-{id}")),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Terminal {
                pane_id: PaneId(format!("pane-{session_id}")),
                session_id: SessionId(session_id.into()),
                title: "shell".into(),
            },
            active_pane_id: PaneId(format!("pane-{session_id}")),
        }];
        ws
    }

    fn test_snapshot(
        active_id: &str,
        workspaces: Vec<crate::state::WorkspaceSnapshot>,
    ) -> crate::state::AppStateSnapshot {
        use crate::state::*;
        AppStateSnapshot {
            schema_version: 1,
            snapshot_revision: 0,
            snapshot_instance: String::new(),
            active_workspace_id: WorkspaceId(active_id.into()),
            workspaces,
            terminal_sessions: Vec::new(),
            browser_sessions: Vec::new(),
            persistence: PersistenceSchema {
                schema_version: 1,
                stores_layout_metadata: true,
                stores_terminal_metadata: true,
                stores_live_process_state: false,
            },
            config: CodemuxConfigSnapshot {
                config_version: 1,
                default_shell: None,
                theme_source: "system".into(),
                linux_first: false,
                notification_sound_enabled: false,
                ai_commit_message_enabled: false,
                ai_commit_message_cli: None,
                ai_commit_message_model: None,
                ai_resolver_enabled: false,
                ai_resolver_cli: None,
                ai_resolver_model: None,
                ai_resolver_strategy: "smart_merge".into(),
            },
            detected_ports: Vec::new(),
            notifications: Vec::new(),
            pane_statuses: std::collections::HashMap::new(),
            agent_browser_sessions: Vec::new(),
            archived_workspaces: Vec::new(),
        }
    }

    #[test]
    fn find_owning_workspace_returns_correct_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "ws-a", "/a", None, None, None, "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b", "ws-b", "/b", Some("feat"), Some("/b"), Some("/repo"), "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        // Session in workspace B should resolve to workspace B, not active workspace A
        let owner = find_owning_workspace(&snapshot, "sess-b");
        assert!(owner.is_some());
        assert_eq!(owner.unwrap().workspace_id.0, "ws-b");

        // Session in workspace A
        let owner_a = find_owning_workspace(&snapshot, "sess-a");
        assert!(owner_a.is_some());
        assert_eq!(owner_a.unwrap().workspace_id.0, "ws-a");
    }

    #[test]
    fn find_owning_workspace_returns_none_for_orphan() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "ws-a", "/a", None, None, None, "sess-a",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a]);

        assert!(find_owning_workspace(&snapshot, "sess-orphan").is_none());
    }

    #[test]
    fn find_owning_workspace_handles_empty_surfaces() {
        let ws = test_workspace("ws-empty", "empty", "/e", None, None, None);
        let snapshot = test_snapshot("ws-empty", vec![ws]);

        // Workspace has no surfaces at all
        assert!(find_owning_workspace(&snapshot, "sess-any").is_none());
    }

    #[test]
    fn find_owning_workspace_searches_split_panes() {
        use crate::state::*;
        let mut ws = test_workspace("ws-split", "split", "/s", None, None, None);
        // Nested: split > split > terminal (3 levels deep)
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId("surf-split".into()),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Split {
                pane_id: PaneId("split-root".into()),
                direction: SplitDirection::Horizontal,
                child_sizes: vec![0.5, 0.5],
                children: vec![
                    PaneNodeSnapshot::Split {
                        pane_id: PaneId("split-inner".into()),
                        direction: SplitDirection::Vertical,
                        child_sizes: vec![0.5, 0.5],
                        children: vec![
                            PaneNodeSnapshot::Terminal {
                                pane_id: PaneId("pane-deep".into()),
                                session_id: SessionId("sess-deep".into()),
                                title: "deep".into(),
                            },
                            PaneNodeSnapshot::Terminal {
                                pane_id: PaneId("pane-1".into()),
                                session_id: SessionId("sess-1".into()),
                                title: "left-bottom".into(),
                            },
                        ],
                    },
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("pane-2".into()),
                        session_id: SessionId("sess-2".into()),
                        title: "right".into(),
                    },
                ],
            },
            active_pane_id: PaneId("pane-1".into()),
        }];
        let snapshot = test_snapshot("ws-split", vec![ws]);

        // Deeply nested terminal (split > split > terminal) is reachable
        assert!(find_owning_workspace(&snapshot, "sess-deep").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-1").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-2").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-3").is_none());
    }

    #[test]
    fn owning_workspace_env_vars_use_correct_workspace() {
        // The actual bug scenario: workspace A is active, session belongs to workspace B.
        // Verify env vars come from workspace B, not workspace A.
        let ws_a = test_workspace_with_pane(
            "ws-a",
            "main-ws",
            "/home/user/projects/repo",
            Some("main"),
            None,
            Some("/home/user/projects/repo"),
            "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b",
            "feature-ws",
            "/home/user/.codemux/worktrees/repo/feature",
            Some("feat/feature"),
            Some("/home/user/.codemux/worktrees/repo/feature"),
            Some("/home/user/projects/repo"),
            "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        // Look up workspace B's session
        let owner = find_owning_workspace(&snapshot, "sess-b").unwrap();
        assert_eq!(owner.workspace_id.0, "ws-b");

        // Verify env vars come from workspace B
        let env = env_map(owner);
        assert_eq!(
            env["CODEMUX_WORKSPACE_PATH"],
            "/home/user/.codemux/worktrees/repo/feature"
        );
        assert_eq!(env["CODEMUX_BRANCH"], "feat/feature");
        assert_eq!(env["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(env["CODEMUX_WORKSPACE_NAME"], "feature-ws");

        // Verify workspace A would produce different values
        let owner_a = find_owning_workspace(&snapshot, "sess-a").unwrap();
        let env_a = env_map(owner_a);
        assert_eq!(env_a["CODEMUX_WORKSPACE_PATH"], "/home/user/projects/repo");
        assert_eq!(env_a["CODEMUX_BRANCH"], "main");
    }

    // ── resolve_session_cwd tests ─────────────────────────────────

    #[test]
    fn resolve_session_cwd_uses_scrollback_dir_when_exists() {
        // Use the platform temp dir as the "known-to-exist" path — `/tmp`
        // exists on Unix but not on Windows, and `std::env::temp_dir()`
        // resolves correctly on both (e.g. `/tmp` on Linux,
        // `C:\Users\<user>\AppData\Local\Temp` on Windows).
        let temp = std::env::temp_dir();
        let temp_str = temp.to_string_lossy().into_owned();
        let result = resolve_session_cwd(&temp_str, "/fallback");
        assert_eq!(result, temp_str);
    }

    #[test]
    fn resolve_session_cwd_falls_back_when_dir_missing() {
        let result = resolve_session_cwd(
            "/nonexistent/worktree/that/was/deleted",
            "/home/fallback",
        );
        assert_eq!(result, "/home/fallback");
    }

    #[test]
    fn resolve_session_cwd_falls_back_on_empty_string() {
        let result = resolve_session_cwd("", "/home/fallback");
        assert_eq!(result, "/home/fallback");
    }

    // ── Spawn-reservation TOCTOU tests (Bug 1b) ───────────────────

    fn make_spawn_sessions() -> Arc<Mutex<HashMap<String, SessionRuntime>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn try_reserve_session_spawn_first_caller_wins() {
        let sessions = make_spawn_sessions();
        assert!(
            try_reserve_session_spawn(&sessions, "sess-1"),
            "first reservation must succeed"
        );
        // The reservation must be visible as is_spawning under the lock.
        assert!(is_session_spawn_active(&sessions, "sess-1"));
    }

    #[test]
    fn try_reserve_session_spawn_second_caller_loses_while_in_flight() {
        let sessions = make_spawn_sessions();
        assert!(try_reserve_session_spawn(&sessions, "race-id"));
        // Second caller for the same session must observe the placeholder
        // and bail. Without this guarantee, two concurrent
        // `spawn_pty_for_session` callers would both pass the historic
        // "writer/master is None" check and both spawn ConPTY children for
        // the same session id — the exact bug we're closing.
        assert!(
            !try_reserve_session_spawn(&sessions, "race-id"),
            "second reservation while in flight must lose"
        );
    }

    #[test]
    fn try_reserve_session_spawn_succeeds_again_after_runtime_removed() {
        // The error early-return paths in spawn_pty_for_session call
        // remove_session_runtime to drop the placeholder. After that, the
        // next caller must be able to reserve again so a real retry / restart
        // path works.
        let sessions = make_spawn_sessions();
        assert!(try_reserve_session_spawn(&sessions, "retry-id"));
        // Simulate the error early-return cleanup.
        remove_session_runtime(&sessions, "retry-id");
        assert!(
            try_reserve_session_spawn(&sessions, "retry-id"),
            "post-cleanup retry must succeed"
        );
    }

    #[test]
    fn try_reserve_session_spawn_blocks_when_writer_already_set() {
        // Once a session is fully running (writer/master populated, is_spawning
        // cleared) a fresh `spawn_pty_for_session` call MUST also lose the
        // reservation race so we never double-spawn.
        let sessions = make_spawn_sessions();
        with_session_runtime(
            &sessions,
            "running-id",
            || SessionRuntime::new("running-id"),
            |rt| {
                // Use a no-op Write impl as a stand-in for a real PTY writer.
                struct NoopWriter;
                impl std::io::Write for NoopWriter {
                    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                        Ok(buf.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                rt.writer = Some(Box::new(NoopWriter));
                rt.is_spawning = false;
            },
        );
        assert!(
            !try_reserve_session_spawn(&sessions, "running-id"),
            "reservation against a fully-running session must lose"
        );
    }

    #[test]
    fn try_reserve_session_spawn_only_one_winner_under_thread_race() {
        // Smoke-test the lock semantics: 16 threads all race to reserve the
        // same session id. Exactly one must win.
        let sessions = make_spawn_sessions();
        let session_id = "thread-race".to_string();
        let mut handles = Vec::new();
        let winners = Arc::new(Mutex::new(0u32));
        for _ in 0..16 {
            let s = sessions.clone();
            let id = session_id.clone();
            let w = winners.clone();
            handles.push(std::thread::spawn(move || {
                if try_reserve_session_spawn(&s, &id) {
                    *w.lock().unwrap() += 1;
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let winner_count = *winners.lock().unwrap();
        assert_eq!(
            winner_count, 1,
            "exactly one of 16 racing threads must win the reservation; got {winner_count}"
        );
    }

    #[test]
    fn is_session_spawn_active_reflects_lifecycle_states() {
        let sessions = make_spawn_sessions();
        // No entry → not active.
        assert!(!is_session_spawn_active(&sessions, "lifecycle"));
        // Reserved → active.
        assert!(try_reserve_session_spawn(&sessions, "lifecycle"));
        assert!(is_session_spawn_active(&sessions, "lifecycle"));
        // Cleared (success path simulation) → still active because writer/
        // master would be populated by the real spawn flow. Here we just
        // clear is_spawning and check the helper still says "active" iff
        // writer or master is populated. We're not setting them here, so
        // is_session_spawn_active should now be false.
        with_session_runtime(
            &sessions,
            "lifecycle",
            || SessionRuntime::new("lifecycle"),
            |rt| {
                rt.is_spawning = false;
            },
        );
        assert!(
            !is_session_spawn_active(&sessions, "lifecycle"),
            "after clearing is_spawning with writer/master still None, the session is not active"
        );
    }

    // ── Active-workspace gating tests (Bug 1a) ───────────────────

    #[test]
    fn collect_workspace_session_ids_returns_only_target_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "a", "/a", None, None, None, "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b", "b", "/b", None, None, None, "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        let a_ids = collect_workspace_session_ids(&snapshot, "ws-a");
        let b_ids = collect_workspace_session_ids(&snapshot, "ws-b");

        assert_eq!(a_ids, vec!["sess-a".to_string()]);
        assert_eq!(b_ids, vec!["sess-b".to_string()]);
    }

    #[test]
    fn collect_workspace_session_ids_returns_empty_for_unknown_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "a", "/a", None, None, None, "sess-a",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a]);

        // Misspelled / nonexistent workspace id is the realistic stale-state
        // case during startup hydration; must return empty rather than
        // accidentally hydrating a different workspace.
        let nope = collect_workspace_session_ids(&snapshot, "ws-does-not-exist");
        assert!(nope.is_empty());
    }

    #[test]
    fn collect_workspace_session_ids_walks_split_panes() {
        use crate::state::*;
        let mut ws = test_workspace("ws-tree", "t", "/t", None, None, None);
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId("surf".into()),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Split {
                pane_id: PaneId("split-root".into()),
                direction: SplitDirection::Horizontal,
                child_sizes: vec![0.5, 0.5],
                children: vec![
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("p1".into()),
                        session_id: SessionId("s1".into()),
                        title: "left".into(),
                    },
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("p2".into()),
                        session_id: SessionId("s2".into()),
                        title: "right".into(),
                    },
                ],
            },
            active_pane_id: PaneId("p1".into()),
        }];
        let snapshot = test_snapshot("ws-tree", vec![ws]);

        let ids = collect_workspace_session_ids(&snapshot, "ws-tree");
        // Order matches the depth-first walk in collect_terminal_sessions;
        // we just care that BOTH session ids are present so the lazy hydrate
        // path covers every pane in the workspace.
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"s1".to_string()));
        assert!(ids.contains(&"s2".to_string()));
    }

    // -- process-group kill tests -----------------------------------------------
    //
    // These tests spawn real processes through a real PTY and verify that
    // `kill_session_tree` / `terminate_pty_session` / `SessionRuntime::drop`
    // actually terminate the child (and its children). They are `#[cfg(unix)]`
    // because `kill_session_tree` is only meaningful on Unix, and `#[serial]`
    // so they do not interact with any other test that might spawn processes.

    #[cfg(unix)]
    mod process_kill {
        use super::*;
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use serial_test::serial;
        use std::time::Instant;

        /// Liveness probe for a pid we do **not** own (no `Child` handle).
        /// Uses `libc::kill(pid, 0)` — sends no signal, only checks whether
        /// the pid is reachable. Returns `false` (== not alive) both when
        /// the process has fully exited AND been reaped, and when we have
        /// no permission to signal it (which should not happen in tests).
        ///
        /// Caveat: a zombie (exited but not yet reaped by its parent) still
        /// reports "alive" via this probe because the process-table entry
        /// exists. For children we own, use `try_wait_child` instead.
        fn is_alive(pid: i32) -> bool {
            // Safety: kill with signal 0 does not deliver anything; it only
            // checks whether the caller could signal the target.
            let ret = unsafe { libc::kill(pid, 0) };
            if ret == 0 {
                return true;
            }
            let err = std::io::Error::last_os_error().raw_os_error();
            err != Some(libc::ESRCH)
        }

        /// Poll `is_alive` for up to `timeout` and return whether the pid
        /// became unreachable within that window. Use this ONLY for pids we
        /// do not own (orphaned grandchildren reaped by init).
        fn wait_until_gone(pid: i32, timeout: Duration) -> bool {
            let start = Instant::now();
            while start.elapsed() < timeout {
                if !is_alive(pid) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            !is_alive(pid)
        }

        /// Wait for a `Child` that we own (via portable-pty) to be reaped.
        /// This is the correct probe for children we spawned ourselves —
        /// `libc::kill(pid, 0)` would return success on the zombie because
        /// its process-table entry still exists until we call `wait()`.
        /// `try_wait` reaps the zombie as soon as it sees one, giving us a
        /// definitive "this process has exited" signal.
        fn wait_child_gone(
            child: &mut Box<dyn portable_pty::Child + Send + Sync>,
            timeout: Duration,
        ) -> bool {
            let start = Instant::now();
            while start.elapsed() < timeout {
                match child.try_wait() {
                    Ok(Some(_)) => return true, // exited + reaped
                    Ok(None) => {}              // still running
                    Err(_) => return true,      // reaping error ≈ already gone
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            matches!(child.try_wait(), Ok(Some(_)) | Err(_))
        }

        /// Spawn a command on a fresh PTY pair and return `(child, pid)`.
        /// Dropping the returned `Child` does not kill the process — we
        /// keep it alive so the tests can call `wait()` themselves to avoid
        /// leaving zombies behind.
        fn spawn_on_pty(
            cmd: CommandBuilder,
        ) -> (Box<dyn portable_pty::Child + Send + Sync>, u32) {
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let child = pty.slave.spawn_command(cmd).expect("spawn");
            // Drop the slave side — we don't need it; the spawned process
            // already inherited its fds.
            drop(pty.slave);
            let pid = child.process_id().expect("child pid");
            // Leak the master so the PTY fds stay open for the lifetime of
            // the test. Otherwise closing the master can deliver SIGHUP to
            // the child before we run our kill logic.
            std::mem::forget(pty.master);
            (child, pid)
        }

        /// SIGTERM→200ms→SIGKILL should clear a simple long-sleeping child.
        #[test]
        #[serial]
        fn test_kill_session_tree_removes_shell() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);

            // Sanity: child is alive before we kill it.
            assert!(is_alive(pid as i32), "child should be alive pre-kill");

            kill_session_tree(pid);

            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "pid {pid} should be exited + reaped within 2s after kill_session_tree"
            );
        }

        /// The critical test: killpg must reach the *grandchild* of the PTY
        /// shell. This is the exact property we rely on — portable-pty's
        /// `setsid()` means the shell is a process-group leader, and any
        /// children it spawns inherit that PGID unless they detach themselves.
        /// A plain `sh` doing `sleep 3600 &` is the canonical case.
        #[test]
        #[serial]
        fn test_kill_session_tree_kills_grandchild() {
            // We need to keep the master side of the PTY around so we can
            // read the grandchild PID the shell prints, so we spawn inline
            // instead of using `spawn_on_pty`.
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.arg("-c");
            // Shell backgrounds sleep, prints its PID on a marker line,
            // then waits forever. We parse the marker to learn the PID.
            cmd.arg("sleep 3600 & echo GRANDCHILD_PID=$! && wait");
            let mut child = pty.slave.spawn_command(cmd).expect("spawn");
            drop(pty.slave);
            let shell_pid = child.process_id().expect("shell pid");
            let mut reader = pty.master.try_clone_reader().expect("reader");

            // Read PTY output until we see the marker line.
            let mut buf = [0u8; 1024];
            let mut accumulated = Vec::new();
            let start = Instant::now();
            let grandchild_pid: u32 = loop {
                if start.elapsed() > Duration::from_secs(5) {
                    panic!(
                        "timed out waiting for GRANDCHILD_PID in: {:?}",
                        String::from_utf8_lossy(&accumulated)
                    );
                }
                match reader.read(&mut buf) {
                    Ok(0) => panic!("PTY closed before GRANDCHILD_PID arrived"),
                    Ok(n) => {
                        accumulated.extend_from_slice(&buf[..n]);
                        if let Ok(text) = std::str::from_utf8(&accumulated) {
                            if let Some(line) =
                                text.lines().find(|l| l.contains("GRANDCHILD_PID="))
                            {
                                let pid_str = line
                                    .split("GRANDCHILD_PID=")
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .trim_end_matches(|c: char| !c.is_ascii_digit());
                                break pid_str.parse().expect("parse pid");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => panic!("read error: {e}"),
                }
            };

            assert!(is_alive(shell_pid as i32), "shell should be alive");
            assert!(
                is_alive(grandchild_pid as i32),
                "grandchild should be alive"
            );

            kill_session_tree(shell_pid);

            // Shell is ours → use try_wait to reap the zombie.
            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "shell pid {shell_pid} should be exited + reaped \
                 after kill_session_tree"
            );
            // Grandchild is orphaned by the shell's death → init (pid 1)
            // adopts and reaps it. `libc::kill(pid, 0)` is the correct
            // probe here because we are not its parent. Init reaps
            // quickly but give it up to 3s to absorb CI jitter.
            assert!(
                wait_until_gone(grandchild_pid as i32, Duration::from_secs(3)),
                "grandchild pid {grandchild_pid} (the `sleep 3600 &`) should be \
                 gone after kill_session_tree — this is the whole point of killpg"
            );
            // Leak the master so closing it cannot deliver SIGHUP to any
            // still-exiting process.
            std::mem::forget(pty.master);
        }

        /// Calling kill_session_tree on a PID that has already been reaped
        /// (ESRCH path) must not panic or print spurious errors.
        #[test]
        #[serial]
        fn test_kill_session_tree_esrch_is_ok() {
            let cmd = CommandBuilder::new("true");
            let (mut child, pid) = spawn_on_pty(cmd);
            // Wait for `true` to exit naturally.
            let _ = child.wait();
            // Give the kernel a moment to fully tear down the process entry.
            std::thread::sleep(Duration::from_millis(50));
            assert!(!is_alive(pid as i32), "true should be gone after wait");

            // Should complete without panicking and without extra stderr.
            kill_session_tree(pid);
        }

        /// `Drop for SessionRuntime` is the safety net for code paths that
        /// forget to clear `child_pid`. Construct a runtime with a live PID,
        /// drop it, and assert the child is exited + reaped.
        #[test]
        #[serial]
        fn test_session_runtime_drop_kills_live_child() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);
            assert!(is_alive(pid as i32));

            let mut runtime = SessionRuntime::new("drop-test");
            runtime.child_pid = Some(pid);
            // Drop runs here → warning printed to stderr, process killed.
            drop(runtime);

            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "Drop impl should have killed and allowed reap of pid {pid}"
            );
        }

        /// `terminate_pty_session` must be idempotent — calling it twice on
        /// the same id removes the runtime the first time and no-ops the
        /// second, matching the `HashMap::remove` semantics that protect
        /// us from races with the waiter thread.
        #[test]
        #[serial]
        fn test_terminate_pty_session_double_call_is_noop() {
            let sessions = make_sessions();
            // Insert a runtime with no child_pid so the helper exits
            // cleanly without trying to signal anything.
            with_session_runtime(
                &sessions,
                "double",
                || SessionRuntime::new("double"),
                |runtime| {
                    runtime.child_pid = None;
                },
            );

            terminate_pty_session(&sessions, "double");
            terminate_pty_session(&sessions, "double");

            assert!(
                sessions.lock().unwrap().get("double").is_none(),
                "runtime should be gone after terminate_pty_session"
            );
        }

        /// Asserts the property we rely on for the whole fix: portable-pty
        /// puts every spawned child into its own session via `setsid()` in
        /// its Unix `pre_exec` hook, so the child's PGID equals its PID.
        /// If portable-pty ever regresses on this, `killpg(pid, ...)` would
        /// signal Codemux's own process group instead of the child — which
        /// this test would catch by asserting `getpgid(pid) == pid`.
        #[test]
        #[serial]
        fn test_portable_pty_child_pgid_equals_pid() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);

            // SAFETY: `getpgid` is a simple POSIX call that reads the
            // process group id of the target pid. No memory concerns. A
            // return of -1 would indicate ESRCH/EPERM; we assert it's > 0.
            let pgid = unsafe { libc::getpgid(pid as i32) };
            assert!(pgid > 0, "getpgid({pid}) failed with {pgid}");
            assert_eq!(
                pgid as u32, pid,
                "portable-pty child PGID must equal its PID (setsid contract); \
                 got pgid={pgid} pid={pid}. If this fails, portable-pty regressed \
                 and `killpg` now targets Codemux's own process group."
            );

            // Clean up.
            kill_session_tree(pid);
            let _ = wait_child_gone(&mut child, Duration::from_secs(2));
        }

        /// Soak test: spawn 10 independent PTY sessions with long-running
        /// sleeps, tear each one down through `terminate_pty_session`, and
        /// assert all 10 PIDs are gone. Exercises the HashMap mutex + kill
        /// path under burst load and catches any per-session leak that
        /// single-session tests would miss.
        #[test]
        #[serial]
        fn test_soak_ten_sessions_no_leaks() {
            const N: usize = 10;
            let sessions = make_sessions();
            let mut children: Vec<(Box<dyn portable_pty::Child + Send + Sync>, u32)> =
                Vec::with_capacity(N);

            for i in 0..N {
                let mut cmd = CommandBuilder::new("sleep");
                cmd.arg("3600");
                let (child, pid) = spawn_on_pty(cmd);
                // Insert a runtime with the real pid so `terminate_pty_session`
                // has something to find and kill. Master/writer are set to
                // None — this test exercises the kill path, not I/O.
                let session_id = format!("soak-{i}");
                with_session_runtime(
                    &sessions,
                    &session_id,
                    || SessionRuntime::new(&session_id),
                    |runtime| {
                        runtime.child_pid = Some(pid);
                    },
                );
                children.push((child, pid));
            }

            // Confirm every child is alive before we kill them.
            for (_, pid) in &children {
                assert!(is_alive(*pid as i32), "soak child {pid} should be alive");
            }

            // Tear them all down through the public helper.
            for i in 0..N {
                terminate_pty_session(&sessions, &format!("soak-{i}"));
            }

            // Every runtime should be gone from the map, and every child
            // should be reaped within 2s.
            {
                let guard = sessions.lock().unwrap();
                for i in 0..N {
                    assert!(
                        guard.get(&format!("soak-{i}")).is_none(),
                        "runtime soak-{i} should have been removed"
                    );
                }
            }
            for (mut child, pid) in children {
                assert!(
                    wait_child_gone(&mut child, Duration::from_secs(2)),
                    "soak pid {pid} should be exited + reaped"
                );
            }
        }

        /// Documents a known v1 limitation: if a grandchild calls `setsid`
        /// itself, it escapes the shell's process group and `killpg(shell_pgid, ...)`
        /// does NOT reach it. This test pins that behavior so anyone reading
        /// the test file understands the failure mode; if this test ever
        /// fails (i.e. the grandchild IS killed), it means we gained a
        /// tree-walking fallback and this limitation is no longer accurate.
        ///
        /// Track the `/proc`-walking tree-kill fallback as the fix if this
        /// becomes a real problem — e.g. if Claude CLI, an MCP server, or
        /// rust-analyzer is observed to call `setsid` in practice.
        #[test]
        #[serial]
        fn test_killpg_does_not_reach_setsid_detached_grandchild() {
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.arg("-c");
            // `setsid --fork` always double-forks and exec's the target in
            // a new session. The inner `sh -c` prints its own `$$` (the
            // real session leader) so we can probe it. We cannot use
            // `$!` here because setsid's internal fork makes `$!` point
            // at the short-lived wrapper, not the real session leader.
            cmd.arg(
                "setsid --fork sh -c 'echo DETACHED_PID=$$; sleep 3600' & \
                 echo SHELL_READY; wait",
            );
            let mut child = pty.slave.spawn_command(cmd).expect("spawn");
            drop(pty.slave);
            let shell_pid = child.process_id().expect("shell pid");
            let mut reader = pty.master.try_clone_reader().expect("reader");

            let mut buf = [0u8; 1024];
            let mut accumulated = Vec::new();
            let start = Instant::now();
            let detached_pid: u32 = loop {
                if start.elapsed() > Duration::from_secs(5) {
                    panic!(
                        "timed out waiting for DETACHED_PID in: {:?}",
                        String::from_utf8_lossy(&accumulated)
                    );
                }
                match reader.read(&mut buf) {
                    Ok(0) => panic!("PTY closed before DETACHED_PID arrived"),
                    Ok(n) => {
                        accumulated.extend_from_slice(&buf[..n]);
                        if let Ok(text) = std::str::from_utf8(&accumulated) {
                            if let Some(line) =
                                text.lines().find(|l| l.contains("DETACHED_PID="))
                            {
                                let pid_str = line
                                    .split("DETACHED_PID=")
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .trim_end_matches(|c: char| !c.is_ascii_digit());
                                break pid_str.parse().expect("parse pid");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => panic!("read error: {e}"),
                }
            };

            // Confirm the detached child is in its own PGID.
            // SAFETY: `getpgid` reads the pgid of a pid we just read from
            // the PTY; no aliasing concerns.
            let detached_pgid = unsafe { libc::getpgid(detached_pid as i32) };
            assert!(
                detached_pgid > 0 && detached_pgid as u32 == detached_pid,
                "detached child should be its own pgid leader (pgid={detached_pgid} pid={detached_pid})"
            );
            assert_ne!(
                detached_pgid as u32, shell_pid,
                "detached child must not share the shell's pgid — that's the point of setsid"
            );

            // Kill the shell's process group. The detached grandchild
            // should survive because it's in a different pgid.
            kill_session_tree(shell_pid);
            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "shell pid {shell_pid} should still be killed by killpg"
            );

            // The detached grandchild is STILL alive — this is the limitation.
            // Give the scheduler a moment, then assert.
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                is_alive(detached_pid as i32),
                "KNOWN LIMITATION: setsid-detached grandchild pid {detached_pid} \
                 should still be alive after killpg(shell_pgid). If this assertion \
                 fails, we gained a tree-kill fallback — update this test to reflect \
                 the new behavior."
            );

            // Clean up the detached child manually so the test leaves no
            // zombies behind. We're not its parent (init is, since the
            // shell died), so we use plain kill.
            // SAFETY: plain POSIX kill; detached_pid is a pid we own
            // transitively and are now cleaning up after the test.
            unsafe { libc::kill(detached_pid as i32, libc::SIGKILL) };
            // Let init reap it.
            let _ = wait_until_gone(detached_pid as i32, Duration::from_secs(2));

            std::mem::forget(pty.master);
        }

        // -- guard-path tests (pid <= 1) ---------------------------------
        //
        // These tests verify that `kill_session_tree` refuses dangerous
        // PIDs without invoking `killpg`. The only way to *prove* killpg
        // wasn't called is to show that a known innocent process still
        // exists after the call — if `kill_session_tree(0)` reached
        // `killpg(0, SIGKILL)` it would signal our own process group and
        // everything we spawned (the witness sleep, plus the test runner
        // itself) would die.

        /// `kill_session_tree(0)` must be refused: `killpg(0, ...)` signals
        /// the caller's own process group.
        #[test]
        #[serial]
        fn test_kill_session_tree_rejects_pid_zero() {
            // Spawn a witness child that MUST still be alive after the
            // guarded call. If kill_session_tree(0) reached killpg(0, ...)
            // it would signal this test's process group and kill the
            // witness (and probably the test process itself).
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut witness, witness_pid) = spawn_on_pty(cmd);
            assert!(is_alive(witness_pid as i32));

            // The guard path: early-return with an eprintln, no killpg.
            kill_session_tree(0);

            // Witness must still be alive.
            assert!(
                is_alive(witness_pid as i32),
                "witness pid {witness_pid} should still be alive — \
                 kill_session_tree(0) must not reach killpg()"
            );

            // Clean up.
            kill_session_tree(witness_pid);
            let _ = wait_child_gone(&mut witness, Duration::from_secs(2));
        }

        /// `kill_session_tree(1)` must be refused: pid 1 is init / the
        /// container entrypoint and is never a PTY child we spawned.
        /// Even though the kernel usually refuses signals to init, a
        /// defensive guard keeps a bad pid from ever reaching libc.
        #[test]
        #[serial]
        fn test_kill_session_tree_rejects_pid_one() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut witness, witness_pid) = spawn_on_pty(cmd);
            assert!(is_alive(witness_pid as i32));

            kill_session_tree(1);

            assert!(
                is_alive(witness_pid as i32),
                "witness pid {witness_pid} should still be alive — \
                 kill_session_tree(1) must not reach killpg()"
            );

            kill_session_tree(witness_pid);
            let _ = wait_child_gone(&mut witness, Duration::from_secs(2));
        }

        // -- terminate_pty_session edge paths ----------------------------

        /// The `let Some(...) else { return }` path: asking to terminate
        /// a session that is not in the map must be a silent no-op.
        #[test]
        fn test_terminate_pty_session_missing_session_is_noop() {
            let sessions = make_sessions();
            assert!(sessions.lock().unwrap().is_empty());

            // No panic, no insertion.
            terminate_pty_session(&sessions, "not-there");

            assert!(
                sessions.lock().unwrap().is_empty(),
                "terminate on missing session must not create an entry"
            );
        }

        /// The `None => eprintln!(...)` path: a runtime is in the map but
        /// `child_pid` is `None`. After Fix 2 (spawn fails loud if
        /// `process_id()` returns None), a live session cannot reach this
        /// branch — the only remaining cause is waiter-thread phantom
        /// insertion between `terminate_pty_session`'s remove and its
        /// kill. Must still remove the runtime and not panic.
        #[test]
        fn test_terminate_pty_session_none_child_pid() {
            let sessions = make_sessions();
            // `SessionRuntime::new` initializes child_pid to None.
            with_session_runtime(
                &sessions,
                "phantom",
                || SessionRuntime::new("phantom"),
                |_| {},
            );
            assert!(sessions.lock().unwrap().contains_key("phantom"));

            terminate_pty_session(&sessions, "phantom");

            assert!(
                sessions.lock().unwrap().get("phantom").is_none(),
                "runtime should be removed even when child_pid is None"
            );
            // Also: the eprintln warning text must not imply a spawn bug
            // unambiguously — see the `None` arm of `terminate_pty_session`
            // and its updated wording "spawn returned None or waiter-thread
            // race". We don't capture stderr here, but the wording test is
            // the source-level comment.
        }

        // -- SessionRuntime Drop ------------------------------------------

        /// Dropping a runtime with `child_pid = None` must be silent and
        /// must not panic. Guards against a future refactor that adds
        /// logic to `Drop` assuming `child_pid` is always `Some`.
        #[test]
        fn test_session_runtime_drop_with_none_pid_is_silent() {
            let runtime = SessionRuntime::new("drop-none");
            assert!(runtime.child_pid.is_none());
            // No panic on drop. Runs the Drop impl's `if let Some(..)`
            // with a None and falls through silently.
            drop(runtime);
        }

        // -- documented gaps (ignored stubs) -----------------------------

        #[test]
        #[ignore]
        fn test_concurrent_terminate_and_waiter_race() {
            // Requires controlled thread interleaving to test properly.
            // The race is benign (no leak, possible spurious warning).
            // Documented as untested.
        }

        #[test]
        #[ignore]
        fn test_spawn_with_none_process_id() {
            // Cannot simulate portable_pty::Child::process_id() returning
            // None without mocking the crate. The spawn path now fails
            // loudly (Fix 2). Documented as untested.
        }
    }

    // ── tail_pty_output (terminal_read core) ───────────────────────────
    //
    // `tail_pty_output` is the pure line-tailing helper inside
    // `read_terminal_output`, which is the body of the `read_terminal`
    // socket command and the `terminal_read` MCP tool. Anything we
    // assert here is also the wire-format contract those callers see.

    #[test]
    fn tail_pty_output_empty_input() {
        let (data, total, returned, truncated) = tail_pty_output(b"", 200);
        // `"".split('\n')` yields a single empty string, so we report
        // exactly one line. This is intentional — it gives the caller a
        // stable invariant: total_lines >= 1 always.
        assert_eq!(data, "");
        assert_eq!(total, 1);
        assert_eq!(returned, 1);
        assert!(!truncated);
    }

    #[test]
    fn tail_pty_output_under_cap_returns_all() {
        let raw = b"alpha\nbeta\ngamma";
        let (data, total, returned, truncated) = tail_pty_output(raw, 200);
        assert_eq!(data, "alpha\nbeta\ngamma");
        assert_eq!(total, 3);
        assert_eq!(returned, 3);
        assert!(!truncated);
    }

    #[test]
    fn tail_pty_output_strips_crlf() {
        // PTYs running on Windows or programs that explicitly emit CRLF
        // (some agents, some shells) shouldn't leave \r at the end of
        // every line in the MCP response. tail_pty_output normalizes.
        let raw = b"first\r\nsecond\r\nthird";
        let (data, total, _returned, _truncated) = tail_pty_output(raw, 200);
        assert_eq!(data, "first\nsecond\nthird");
        assert_eq!(total, 3);
    }

    #[test]
    fn tail_pty_output_caps_to_last_n_lines() {
        // Build 10 lines, ask for last 3 — must return the trailing
        // window and flag `truncated=true` so the caller can detect
        // it without diffing counts.
        let raw = (1..=10)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (data, total, returned, truncated) = tail_pty_output(raw.as_bytes(), 3);
        assert_eq!(data, "line-8\nline-9\nline-10");
        assert_eq!(total, 10);
        assert_eq!(returned, 3);
        assert!(truncated);
    }

    #[test]
    fn tail_pty_output_zero_cap_is_treated_as_one() {
        // The MCP schema constrains `lines` to >= 1, but defensively the
        // helper clamps 0 to 1 so a buggy caller can't trip an index
        // panic in `lines[start..]`.
        let raw = b"alpha\nbeta\ngamma";
        let (_data, _total, returned, _truncated) = tail_pty_output(raw, 0);
        assert_eq!(returned, 1);
    }

    #[test]
    fn tail_pty_output_invalid_utf8_is_lossy() {
        // PTY bytes can be raw and partial — a half-decoded UTF-8 sequence
        // must not panic. `from_utf8_lossy` replaces invalid sequences
        // with U+FFFD, which is fine for an agent reading the response.
        let raw = b"alpha\n\xFF\xFE\xFD\nbeta";
        let (data, total, returned, _) = tail_pty_output(raw, 200);
        assert!(data.starts_with("alpha\n"));
        assert!(data.ends_with("\nbeta"));
        assert_eq!(total, 3);
        assert_eq!(returned, 3);
    }

    // ── read_terminal_output integration ───────────────────────────────
    //
    // Exercises the full PtyState lookup → chunk-collect → tail path,
    // bypassing the Tauri State wrapper since `read_terminal_output`
    // takes plain `&PtyState`/`&AppStateStore` refs. Guards the
    // `read_terminal` socket command against regressions in the
    // session-resolution and pending-output-aggregation logic.

    #[test]
    fn read_terminal_output_missing_session_errors() {
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();
        let err = read_terminal_output(&pty, &app, None, Some("does-not-exist".to_string()))
            .expect_err("missing session must error");
        assert!(err.contains("does-not-exist"));
    }

    #[test]
    fn read_terminal_output_no_active_session_errors() {
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();
        // `default_app_state()` seeds a CWD workspace with an active
        // surface, so `active_terminal_session_id()` returns Some.
        // Wipe that so the no-active-session fallback fires — vexis-agent
        // calls `terminal_read` without a session_id when it wants the
        // focused pane, and we need a stable error if nothing is focused.
        app.clear_workspaces();
        let err = read_terminal_output(&pty, &app, None, None)
            .expect_err("no active session must error");
        assert!(
            err.to_lowercase().contains("no active"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn read_terminal_output_concatenates_chunks() {
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();

        with_session_runtime(
            &pty.sessions,
            "sess-read",
            || SessionRuntime::new("sess-read"),
            |runtime| {
                // Simulate two PTY chunks: the agent's prompt and a tool
                // output line. The helper should glue them together in
                // order, not interleave or sort.
                runtime.pending_output.push_back(b"hello\n".to_vec());
                runtime.pending_output.push_back(b"world".to_vec());
            },
        );

        let out =
            read_terminal_output(&pty, &app, Some(10), Some("sess-read".to_string())).unwrap();
        assert_eq!(out.session_id, "sess-read");
        assert_eq!(out.data, "hello\nworld");
        assert_eq!(out.total_lines, 2);
        assert_eq!(out.lines_returned, 2);
        assert!(!out.truncated);
    }

    #[test]
    fn read_terminal_output_default_when_lines_none() {
        // The `read_terminal` socket-command arm passes `lines` straight
        // through as `Option<usize>`. When the MCP caller omits the
        // argument entirely, this must use `READ_TERMINAL_DEFAULT_LINES`
        // (200) — not 0, not the cap. Builds 250 lines of buffered
        // output and asks for the default; expect 200 returned with
        // `truncated=true` and 250 total.
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();

        with_session_runtime(
            &pty.sessions,
            "sess-default",
            || SessionRuntime::new("sess-default"),
            |runtime| {
                let bulk = (1..=250)
                    .map(|i| format!("ln-{i}\n"))
                    .collect::<Vec<_>>()
                    .join("");
                runtime.pending_output.push_back(bulk.into_bytes());
            },
        );

        let out =
            read_terminal_output(&pty, &app, None, Some("sess-default".to_string())).unwrap();
        assert_eq!(
            out.lines_returned, READ_TERMINAL_DEFAULT_LINES,
            "default line count must be {READ_TERMINAL_DEFAULT_LINES}"
        );
        assert!(out.truncated, "older lines beyond the default must be dropped");
        // 250 lines of "ln-N\n" + the trailing empty split = 251 total.
        assert_eq!(out.total_lines, 251);
    }

    #[test]
    fn read_terminal_output_clamps_to_max() {
        // Even if the dispatcher passes `Some(usize::MAX)` (a brain
        // ignoring the schema cap), the helper must clamp to
        // `READ_TERMINAL_MAX_LINES` (5000). Asks for 1_000_000 lines
        // against a tiny buffer; expect the full buffer back (since
        // it's under the cap) but also assert the cap path was taken
        // by checking the helper accepted a value larger than the cap
        // without panicking and without returning more than the cap.
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();

        with_session_runtime(
            &pty.sessions,
            "sess-clamp",
            || SessionRuntime::new("sess-clamp"),
            |runtime| {
                // 6000 lines so we can prove the cap fires.
                let bulk = (1..=6000)
                    .map(|i| format!("ln-{i}\n"))
                    .collect::<Vec<_>>()
                    .join("");
                runtime.pending_output.push_back(bulk.into_bytes());
            },
        );

        let out =
            read_terminal_output(&pty, &app, Some(1_000_000), Some("sess-clamp".to_string()))
                .unwrap();
        assert_eq!(
            out.lines_returned, READ_TERMINAL_MAX_LINES,
            "must clamp to READ_TERMINAL_MAX_LINES ({READ_TERMINAL_MAX_LINES})"
        );
        assert!(out.truncated);
    }

    #[test]
    fn read_terminal_output_applies_line_cap() {
        let pty = PtyState::default();
        let app = crate::state::AppStateStore::default();

        with_session_runtime(
            &pty.sessions,
            "sess-cap",
            || SessionRuntime::new("sess-cap"),
            |runtime| {
                let bulk = (1..=10)
                    .map(|i| format!("ln-{i}\n"))
                    .collect::<Vec<_>>()
                    .join("");
                runtime.pending_output.push_back(bulk.into_bytes());
            },
        );

        let out =
            read_terminal_output(&pty, &app, Some(3), Some("sess-cap".to_string())).unwrap();
        // Last 3 of "ln-1..ln-10\n" — the trailing empty line counts, so
        // the cap-3 window is "ln-9", "ln-10", "" → joined "ln-9\nln-10\n".
        // Truncated must be true so the caller knows older lines exist.
        assert!(out.truncated);
        assert_eq!(out.lines_returned, 3);
        assert!(out.data.contains("ln-9"));
        assert!(out.data.contains("ln-10"));
        assert!(!out.data.contains("ln-7"));
    }
}
