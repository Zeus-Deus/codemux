//! Minimal PTY manager for the headless daemon.
//!
//! Tools that an agent calls (`terminal_write`, `terminal_read`)
//! map to "write to this workspace's shell PTY," "drain the last N
//! bytes the shell produced." This is intentionally narrower than
//! the desktop's terminal subsystem (which manages tabs, focus,
//! resize, scrollback, agent-process supervision, scroll bookmarks,
//! etc.). The headless daemon only needs: one PTY per terminal id,
//! interactive bytes in/out, lifecycle.
//!
//! Implementation uses `portable-pty` directly — already a project
//! dependency — and keeps the reader running on a background OS
//! thread that appends to a per-terminal ring buffer protected by
//! a mutex. The agent polls `terminal_read` to get accumulated
//! output; there is no streaming push path in v1 (it's not needed
//! for stdio MCP tools and would just complicate the surface area).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use uuid::Uuid;

/// One spawned PTY shell.
struct PtySlot {
    /// Master fd, kept alive so writes succeed and child doesn't
    /// get SIGHUP'd when we drop our handle. Unused after being
    /// stored but the Drop side-effect (closing the master fd) is
    /// what triggers PTY teardown when the slot is removed, so the
    /// field is load-bearing despite being read-flagged dead.
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    /// Writer half, separated because portable-pty's reader and
    /// writer come off the master via separate calls.
    writer: Box<dyn Write + Send>,
    /// Accumulated bytes from the child. Ring-buffered to a cap so
    /// long-running shells don't blow memory.
    buffer: Arc<Mutex<RingBuffer>>,
    /// Working directory the shell was launched in.
    cwd: String,
    /// Command line used to spawn (for diagnostics).
    command: String,
    /// Child process handle. We hold it so the OS doesn't reap the
    /// process behind our back; dropping it kills the shell.
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TerminalInfo {
    pub id: String,
    pub cwd: String,
    pub command: String,
}

#[derive(Debug)]
pub enum PtyError {
    NotFound(String),
    Io(String),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "terminal not found: {s}"),
            Self::Io(s) => write!(f, "pty io error: {s}"),
        }
    }
}

impl std::error::Error for PtyError {}

const BUFFER_CAP: usize = 1 * 1024 * 1024; // 1 MiB per terminal — plenty for control plane

struct RingBuffer {
    bytes: Vec<u8>,
    cap: usize,
}

impl RingBuffer {
    fn new(cap: usize) -> Self {
        Self { bytes: Vec::new(), cap }
    }
    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.cap {
            // Single chunk bigger than the cap — keep only the tail.
            let start = chunk.len() - self.cap;
            self.bytes.clear();
            self.bytes.extend_from_slice(&chunk[start..]);
            return;
        }
        let total = self.bytes.len() + chunk.len();
        if total > self.cap {
            let drop_count = total - self.cap;
            self.bytes.drain(0..drop_count);
        }
        self.bytes.extend_from_slice(chunk);
    }
    fn snapshot(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    fn snapshot_tail(&self, max: usize) -> Vec<u8> {
        if self.bytes.len() <= max {
            return self.bytes.clone();
        }
        let start = self.bytes.len() - max;
        self.bytes[start..].to_vec()
    }
}

/// Top-level manager. Holds all PTYs by id, hands them out to
/// callers, owns the read threads.
pub struct PtyManager {
    slots: Mutex<HashMap<String, PtySlot>>,
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self { slots: Mutex::new(HashMap::new()) }
    }

    /// Spawn a new PTY shell at `cwd`. If `command` is `None`, runs
    /// `$SHELL` (or `/bin/sh` if unset). Returns the new terminal id.
    pub fn spawn(&self, cwd: PathBuf, command: Option<String>) -> Result<TerminalInfo, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Io(format!("openpty: {e}")))?;

        let resolved_command = command.clone().unwrap_or_else(|| {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
        });
        let mut builder = if resolved_command.contains(' ') {
            // Naive but adequate for v1: shell-out via sh -c if there
            // are spaces (likely arguments). Real shells will quote
            // properly. Anyone needing fancier control can pass an
            // explicit binary.
            let mut b = CommandBuilder::new("/bin/sh");
            b.arg("-c");
            b.arg(&resolved_command);
            b
        } else {
            CommandBuilder::new(&resolved_command)
        };
        builder.cwd(&cwd);
        // PTYs without a sensible TERM make ncurses-based tools sad.
        builder.env("TERM", "xterm-256color");
        // Strip AppRun's loader/toolkit rewrites so commands run here resolve
        // host libraries rather than the AppImage's bundled ones. No-op
        // outside an AppImage.
        crate::execution::sanitize_appimage_env_pty(&mut builder);

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| PtyError::Io(format!("spawn: {e}")))?;
        // Slave fd is no longer needed in this process now that the
        // child holds it. Dropping it explicitly avoids fd leaks.
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master
            .try_clone_reader()
            .map_err(|e| PtyError::Io(format!("clone reader: {e}")))?;
        let writer = master
            .take_writer()
            .map_err(|e| PtyError::Io(format!("take writer: {e}")))?;

        let buffer = Arc::new(Mutex::new(RingBuffer::new(BUFFER_CAP)));
        let buffer_clone = Arc::clone(&buffer);
        std::thread::Builder::new()
            .name("codemux-remote-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF — child exited or pty closed
                        Ok(n) => {
                            if let Ok(mut rb) = buffer_clone.lock() {
                                rb.push(&buf[..n]);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break, // pty closed, EIO on Linux when peer dies
                    }
                }
            })
            .map_err(|e| PtyError::Io(format!("spawn reader thread: {e}")))?;

        let id = Uuid::new_v4().to_string();
        let info = TerminalInfo {
            id: id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            command: resolved_command.clone(),
        };
        let slot = PtySlot {
            master,
            writer,
            buffer,
            cwd: info.cwd.clone(),
            command: info.command.clone(),
            _child: child,
        };
        self.slots.lock().unwrap().insert(id, slot);
        Ok(info)
    }

    /// Write bytes to the given PTY's stdin. The bytes are sent as-is;
    /// callers wanting a newline-terminated command must include `\n`
    /// (or `\r`) themselves.
    pub fn write(&self, terminal_id: &str, data: &[u8]) -> Result<(), PtyError> {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots
            .get_mut(terminal_id)
            .ok_or_else(|| PtyError::NotFound(terminal_id.to_string()))?;
        slot.writer
            .write_all(data)
            .map_err(|e| PtyError::Io(format!("pty write: {e}")))?;
        slot.writer
            .flush()
            .map_err(|e| PtyError::Io(format!("pty flush: {e}")))?;
        Ok(())
    }

    /// Read everything in the PTY's ring buffer (capped at 1 MiB).
    /// `max_bytes`, if `Some`, returns only the tail.
    pub fn read(&self, terminal_id: &str, max_bytes: Option<usize>) -> Result<Vec<u8>, PtyError> {
        let slots = self.slots.lock().unwrap();
        let slot = slots
            .get(terminal_id)
            .ok_or_else(|| PtyError::NotFound(terminal_id.to_string()))?;
        let buffer = slot.buffer.lock().unwrap();
        Ok(match max_bytes {
            Some(n) => buffer.snapshot_tail(n),
            None => buffer.snapshot(),
        })
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        self.slots
            .lock()
            .unwrap()
            .iter()
            .map(|(id, slot)| TerminalInfo {
                id: id.clone(),
                cwd: slot.cwd.clone(),
                command: slot.command.clone(),
            })
            .collect()
    }

    /// Kill the terminal (drops the child + master fd, which sends
    /// SIGHUP to the shell).
    pub fn close(&self, terminal_id: &str) -> Result<(), PtyError> {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots
            .remove(terminal_id)
            .ok_or_else(|| PtyError::NotFound(terminal_id.to_string()))?;
        drop(slot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_output<F: Fn(&[u8]) -> bool>(mgr: &PtyManager, tid: &str, pred: F) -> Vec<u8> {
        for _ in 0..50 {
            let bytes = mgr.read(tid, None).unwrap();
            if pred(&bytes) {
                return bytes;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        mgr.read(tid, None).unwrap()
    }

    #[test]
    fn spawn_write_read_roundtrip() {
        let mgr = PtyManager::new();
        let info = mgr
            .spawn(std::env::temp_dir(), Some("/bin/sh".into()))
            .expect("spawn");
        // Write a marker the shell will echo back via printf.
        mgr.write(&info.id, b"printf 'HELLO-FROM-PTY\\n'\n").unwrap();

        let out = wait_for_output(&mgr, &info.id, |bytes| {
            String::from_utf8_lossy(bytes).contains("HELLO-FROM-PTY")
        });
        assert!(
            String::from_utf8_lossy(&out).contains("HELLO-FROM-PTY"),
            "expected echo back, got {}",
            String::from_utf8_lossy(&out)
        );
        mgr.close(&info.id).unwrap();
    }

    #[test]
    fn read_unknown_returns_not_found() {
        let mgr = PtyManager::new();
        match mgr.read("does-not-exist", None) {
            Err(PtyError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_shows_spawned_terminals() {
        let mgr = PtyManager::new();
        let info = mgr
            .spawn(std::env::temp_dir(), Some("/bin/sh".into()))
            .unwrap();
        let listed = mgr.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, info.id);
        mgr.close(&info.id).unwrap();
    }

    #[test]
    fn ring_buffer_caps_at_one_mib() {
        let mut rb = RingBuffer::new(8);
        rb.push(b"abcd");
        rb.push(b"efgh");
        rb.push(b"ijkl"); // total exceeds cap; oldest 4 drop
        assert_eq!(&rb.snapshot(), b"efghijkl");
    }

    #[test]
    fn ring_buffer_oversized_single_chunk_keeps_tail() {
        let mut rb = RingBuffer::new(4);
        rb.push(b"abcdefghij");
        assert_eq!(&rb.snapshot(), b"ghij");
    }
}
