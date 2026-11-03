//! The connector's own listener: OAuth + /mcp only, never the paired-browser
//! surface, and never beyond 127.0.0.1. Off-machine clients reach it through
//! an HTTPS reverse proxy on this host (the saved public origin).
use super::McpConnectorState;
use crate::database::DatabaseStore;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpListener as StdListener};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// Unit tests never take a fixed port: 0 binds an ephemeral one.
#[cfg(not(test))]
pub const DEFAULT_PORT: u16 = 4378;
#[cfg(test)]
pub const DEFAULT_PORT: u16 = 0;

/// Right after our own stop, axum may still hold the socket for a moment.
const REBIND_ATTEMPTS: u32 = 10;
const REBIND_PAUSE: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(crate) struct Listener {
    running: Option<Running>,
    pub(crate) bind_error: Option<String>,
}

struct Running {
    configured: u16,
    shutdown: tokio::sync::watch::Sender<bool>,
}

/// Bring the listener to the saved config. The config is read under the
/// listener lock, so concurrent saves converge on the last one saved rather
/// than on whichever caller reconciles last. Binding is synchronous, so the
/// caller's status read-back reports the real outcome. Never falls back to
/// another port: the issuer (and every grant's resource) depends on it.
pub(crate) fn reconcile<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<McpConnectorState>();
    let Ok(mut listener) = state.listener.lock() else {
        return;
    };
    let cfg = match super::config(&app.state::<DatabaseStore>()) {
        Ok(cfg) => cfg,
        Err(e) => {
            log::warn!("[codemux::mcp_connector] listener left as it is: {e}");
            return;
        }
    };
    let want = cfg.enabled.then(|| cfg.configured_port());
    if let (Some(running), Some(port)) = (&listener.running, want) {
        if running.configured == port {
            return;
        }
    }
    let stopped = listener.running.take().map(|running| {
        let _ = running.shutdown.send(true);
        running.configured
    });
    state.bound_port.store(0, Ordering::SeqCst);
    listener.bind_error = None;
    let Some(port) = want else {
        return;
    };
    match bind(port, stopped == Some(port)) {
        Ok(std) => {
            let bound = std.local_addr().map(|a| a.port()).unwrap_or(port);
            let (shutdown, mut rx) = tokio::sync::watch::channel(false);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(std) else {
                    return;
                };
                let router = super::router::<R>().with_state(app);
                let result = axum::serve(listener, router.into_make_service())
                    .with_graceful_shutdown(async move {
                        // Also resolves when the sender drops.
                        let _ = rx.changed().await;
                    })
                    .await;
                if let Err(e) = result {
                    log::warn!("[codemux::mcp_connector] listener exited: {e}");
                }
            });
            state.bound_port.store(bound, Ordering::SeqCst);
            listener.running = Some(Running {
                configured: port,
                shutdown,
            });
            log::info!("[codemux::mcp_connector] listening on 127.0.0.1:{bound}");
        }
        Err(e) => {
            // Not transient (another program holds the port), so no retry
            // loop: Settings and the CLI show it, and enabling again retries.
            listener.bind_error = Some(format!("bind 127.0.0.1:{port}: {e}"));
            log::warn!("[codemux::mcp_connector] could not listen on 127.0.0.1:{port}: {e}");
        }
    }
}

fn bind(port: u16, after_own_stop: bool) -> std::io::Result<StdListener> {
    let mut attempt = 0;
    let listener = loop {
        match StdListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Err(e)
                if after_own_stop
                    && e.kind() == ErrorKind::AddrInUse
                    && attempt + 1 < REBIND_ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(REBIND_PAUSE);
            }
            result => break result?,
        }
    };
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Stop listening (app exit, serve teardown). Graceful: axum stops accepting
/// and drops the socket immediately; in-flight requests finish.
pub(crate) fn stop<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<McpConnectorState>() else {
        return;
    };
    let stopped = {
        let Ok(mut listener) = state.listener.lock() else {
            return;
        };
        listener.bind_error = None;
        let running = listener.running.take();
        if let Some(running) = &running {
            let _ = running.shutdown.send(true);
        }
        state.bound_port.store(0, Ordering::SeqCst);
        running.is_some()
    };
    if stopped {
        super::authority_changed(app);
    }
}
