//! Wire protocol between the Tauri app and the `codemux pty-daemon`
//! subprocess.
//!
//! The protocol is **JSON-lines** over a stream socket: each message is a
//! single JSON value terminated by `\n`. This is intentionally slow and easy
//! to debug — we trade per-byte performance for being able to `nc` the socket
//! and read messages by hand. PTY data payloads are base64-encoded so they
//! survive line-framing without binary-safe escaping.
//!
//! There are two logical channels multiplexed over one TCP-style stream:
//!
//! 1. **Request/response** — the client sends a `ClientRequest`, the daemon
//!    sends back exactly one `ServerResponse` keyed on `request_id`.
//! 2. **Output stream** — after a successful `Attach`, the daemon pushes
//!    `ServerEvent::Output` frames for that session until the client sends
//!    `Detach` or the connection drops.
//!
//! Each Tauri-side `PtyDaemonClient` owns one socket connection; the daemon
//! demuxes by `request_id` and `session_id`.

use serde::{Deserialize, Serialize};

/// Daemon wire-protocol version. Bumped when the message shape changes in a
/// backwards-incompatible way. Adoption on startup compares this against the
/// running daemon's reported version and force-restarts on mismatch — the
/// same pattern superset uses for their `EXPECTED_DAEMON_VERSION`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Request from the Tauri app to the daemon. Every request carries a
/// `request_id` so the client can correlate responses without ordering
/// guarantees.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    /// Handshake. The daemon replies with `Hello` carrying its version.
    /// Stale daemons that don't speak this version are torn down by the
    /// supervisor and respawned.
    Hello { request_id: u64 },

    /// Spawn a new PTY-backed child inside the daemon. The daemon retains
    /// the master fd; the client gets back the `pid` (so resource-monitor
    /// + process-tree views still work) and `session_id` (echoed back).
    Spawn {
        request_id: u64,
        session_id: String,
        workspace_id: String,
        argv: Vec<String>,
        /// Working directory for the child. Must exist on the daemon's
        /// filesystem (daemon and Tauri share `$HOME`).
        cwd: String,
        env: Vec<(String, String)>,
        rows: u16,
        cols: u16,
    },

    /// Attach this connection to the named session's output stream.
    /// The daemon will push `ServerEvent::Output` frames until `Detach`.
    /// If the session has buffered output (collected while no client was
    /// attached) the daemon flushes it as the first frames.
    Attach {
        request_id: u64,
        session_id: String,
    },

    /// Stop receiving output frames for this session. Does NOT kill the
    /// child — the PTY keeps running inside the daemon.
    Detach {
        request_id: u64,
        session_id: String,
    },

    /// Write bytes to the PTY's master end (i.e. forward keystrokes).
    Write {
        request_id: u64,
        session_id: String,
        /// Base64-encoded payload. Decoded by the daemon and written
        /// straight to the master fd.
        data_b64: String,
    },

    /// Resize the PTY window. Mirrors `portable_pty::PtySize`.
    Resize {
        request_id: u64,
        session_id: String,
        rows: u16,
        cols: u16,
    },

    /// Kill the PTY's process group (SIGKILL via killpg, same as the
    /// in-process path uses today). The session entry is removed from
    /// the daemon's session map.
    Close {
        request_id: u64,
        session_id: String,
    },

    /// Enumerate all live sessions in the daemon. Used by the Tauri app
    /// on startup to discover orphaned persistent sessions that survived
    /// a previous run.
    List { request_id: u64 },

    /// Ask the daemon to exit cleanly. All PTYs are killed first. Mostly
    /// used by tests; production code lets the daemon stay alive.
    Shutdown { request_id: u64 },
}

/// One-shot reply to a `ClientRequest`. Always carries the originating
/// `request_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerResponse {
    Hello {
        request_id: u64,
        protocol_version: u32,
        daemon_pid: u32,
        daemon_version: String,
    },
    Spawned {
        request_id: u64,
        session_id: String,
        pid: u32,
    },
    Attached {
        request_id: u64,
        session_id: String,
    },
    Detached {
        request_id: u64,
        session_id: String,
    },
    Written {
        request_id: u64,
    },
    Resized {
        request_id: u64,
    },
    Closed {
        request_id: u64,
    },
    Listed {
        request_id: u64,
        sessions: Vec<DaemonSessionInfo>,
    },
    ShuttingDown {
        request_id: u64,
    },
    /// Generic error reply. Used for any request that fails — unknown
    /// session id, spawn failure, etc.
    Error {
        request_id: u64,
        message: String,
    },
}

/// Push event from daemon to client. Not correlated to a request_id —
/// these are server-initiated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// PTY output frame. The client decodes the base64 and writes it
    /// straight to its xterm channel.
    Output {
        session_id: String,
        data_b64: String,
    },
    /// Child process exited. After this event, the daemon removes the
    /// session from its map and any further `Write`/`Resize`/`Attach`
    /// targeting this id will error.
    Exited {
        session_id: String,
        exit_code: i32,
    },
}

/// One row in the `Listed` response. Carries everything the Tauri app
/// needs to restore a `TerminalSession` entry after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSessionInfo {
    pub session_id: String,
    pub workspace_id: String,
    pub pid: u32,
    pub argv: Vec<String>,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
    /// Unix epoch seconds when the session was spawned.
    pub created_at: i64,
}

/// Top-level frame on the socket. We always send one of these per line.
///
/// The Tauri client demuxes by inspecting the variant: `Response` carries a
/// `request_id` for correlation; `Event` is unsolicited and routed by
/// `session_id` to whichever attach handler owns that id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum Frame {
    Response(ServerResponse),
    Event(ServerEvent),
}
