//! Headless `codemux serve` entrypoint (issue #176, Phase 3).
//!
//! Boots the FULL Codemux backend headless on `tauri::test::MockRuntime` (via
//! [`crate::build_headless_app`], which runs the app's `setup` hook — control
//! server, state restore, background loops, and the web-remote config
//! hydration — because nothing else will: Tauri only runs `setup` from an
//! event loop, and serve never enters one) with no display attached, binds the
//! web-remote server through the **same** shared enable path the GUI and the
//! `codemux remote enable` CLI use, prints a scannable pairing QR + link, and
//! then blocks until SIGINT / SIGTERM. This is the away-from-home SSH flow:
//! expose the desktop's UI to a phone or laptop without a desktop session.
//!
//! Not a control-socket round-trip: `serve` is a long-lived foreground
//! process, so `main` dispatches it on the main thread (see
//! [`crate::run_serve`]) rather than through `maybe_run_cli`'s `block_on`.
//!
//! Mutual exclusion with the GUI is symmetric: `serve` refuses to start when
//! another instance already holds the control endpoint, and the GUI's boot
//! path (in `main.rs`) refuses to start when a `serve` instance does.

use crate::web_remote::{self, BIND_SCOPE_ALL};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

/// Tracks admitted headless requests until they finish changing state.
#[derive(Default)]
pub(crate) struct ShutdownAdmission {
    closed: AtomicBool,
    in_flight: Arc<RwLock<()>>,
}

pub(crate) type ShutdownLease = OwnedRwLockReadGuard<()>;
const SHUTTING_DOWN: &str = "Codemux is shutting down";

impl ShutdownAdmission {
    pub(crate) fn admit<R: Runtime>(
        app: &AppHandle<R>,
    ) -> Result<Option<ShutdownLease>, &'static str> {
        app.try_state::<Self>().map(|admission| admission.try_admit()).transpose()
    }

    pub(super) fn check_open<R: Runtime>(app: &AppHandle<R>) -> Result<(), &'static str> {
        app.try_state::<Self>().map_or(Ok(()), |admission| admission.ensure_open())
    }

    fn ensure_open(&self) -> Result<(), &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SHUTTING_DOWN);
        }
        Ok(())
    }

    fn try_admit(&self) -> Result<ShutdownLease, &'static str> {
        let permit = Arc::clone(&self.in_flight)
            .try_read_owned()
            .map_err(|_| SHUTTING_DOWN)?;
        self.ensure_open()?;
        Ok(permit)
    }

    pub(super) async fn close_and_drain(&self) -> Result<(), String> {
        self.closed.store(true, Ordering::Release);
        // Repository setup or teardown can wait indefinitely on an external
        // command. Keep admission closed if the drain is cancelled.
        tokio::time::timeout(Duration::from_secs(20), self.in_flight.write())
            .await
            .map(|_| ())
            .map_err(|_| "Timed out waiting for headless requests to finish during shutdown".into())
    }
}

/// Options parsed from `codemux serve [--scope …] [--port N] [--relay]`.
/// Flag semantics mirror `codemux remote enable` exactly.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Bind scope: `all` | `tailscale` | `loopback`. `None` = fall back to the
    /// persisted scope (or `all` on first run).
    pub scope: Option<String>,
    /// Port to bind. `None` = the persisted port (default 4377).
    pub port: Option<u16>,
    /// Also enable the from-anywhere iroh relay transport.
    pub relay: bool,
}

/// Entry point for `codemux serve`. Runs on the main thread; blocks until a
/// shutdown signal, then performs the same teardown the GUI does on exit and
/// returns. Returns an error — before booting — if another Codemux instance
/// already holds this machine's control endpoint.
pub fn run_serve(opts: ServeOptions) -> Result<(), String> {
    // 0. A serve started from an agent's shell must not pass that agent's
    //    identity to the terminals and tools it starts.
    crate::control_identity::scrub_inherited_agent_identity();

    // 1. Mutual exclusion: never boot a second backend alongside a running GUI
    //    or serve instance (they would fight over the control socket, the DB,
    //    the PTY daemon, and the web-remote port).
    //
    //    The socket catches a running backend that may predate the layout
    //    lock. Desktop performs the same probe after native launch routing.
    //    The lock also covers startup before the socket is listening.
    if crate::control::control_server_is_running() {
        return Err(
            "Codemux is already running on this machine (GUI or serve) — stop it first, \
             or use `codemux remote enable` to control the running instance."
                .to_string(),
        );
    }
    let _layout_lock = crate::state::acquire_layout_lock()?;

    // Install the listeners before setup starts the control server. Signals
    // received during startup remain pending until the shutdown wait begins.
    let shutdown_signal = tauri::async_runtime::block_on(async { register_shutdown_signal() })
        .map_err(|error| format!("Failed to register shutdown signals: {error}"))?;

    // 2. Boot the full app headless (DB, state, control server, PTY warmup,
    //    web_remote restore_on_boot, every background loop) on MockRuntime.
    //    `build_headless_app` runs the Tauri `setup` hook explicitly — Tauri
    //    itself only runs it from the event loop, which serve never enters —
    //    so the control server is listening (mutual exclusion in both
    //    directions, read-only CLI status calls,
    //    `web_remote_set_relay`) and the persisted web-remote config is
    //    hydrated before the enable below merges the CLI flags into it.
    let app = match crate::build_headless_app() {
        Ok(app) => app,
        Err(e) => return Err(format!("failed to boot the headless backend: {e}")),
    };
    let handle = app.handle().clone();

    // Run startup and the signal wait inside the async runtime. Keep `app`
    // alive through teardown.
    let result = tauri::async_runtime::block_on(async move {
        // 6. Debug-asset caveat: dev builds embed no frontend (devUrl), so the
        //    served UI only works when the source-tree `dist/` fallback exists.
        //    Warn once when neither is present (release builds embed it — no
        //    warning).
        #[cfg(debug_assertions)]
        if handle.asset_resolver().get("index.html".into()).is_none() && !dev_dist_available() {
            eprintln!(
                "warning: this debug build embeds no web UI bundle and no source-tree `dist/` \
                 was found — pairing works but the browser UI will not load. Use a release \
                 build (or run `npm run build` first) for the full experience."
            );
        }

        // 3. Bind the web-remote server (shared enable path + flag merge).
        let result = serve_startup(&handle, &opts).await?;

        // 4. Startup banner + pairing code. Relay-only has no pairing link:
        //    nothing listens on the network for it to point at.
        print_banner(&handle, &result);
        let relay_only = !result.status.lan_enabled;
        if relay_only {
            println!();
            println!(
                "Open https://app.codemux.org in a browser signed into your Codemux account \
                 and pick this machine."
            );
        } else {
            match web_remote::control_pair(&handle, None) {
                Ok(pairing) => {
                    let value = serde_json::to_value(&pairing).unwrap_or(serde_json::Value::Null);
                    crate::cli::print_pairing(&value);
                }
                Err(e) => {
                    // Shouldn't happen once the server is enabled. Keep serving;
                    // pairing can be managed in desktop Settings.
                    eprintln!(
                        "[codemux serve] could not mint a pairing code: {e} \
                         (the server is still running — stop it to manage pairing in desktop Settings)"
                    );
                }
            }
        }
        // The outside-assistant connector is independent of web remote and
        // was restored (or not) by setup; report it for relay-only too.
        let connector = crate::mcp_connector::agent_connector_status(handle.clone()).ok();
        if let Some(line) = connector.as_ref().and_then(connector_line) {
            println!("  {line}");
        }
        print_footer(relay_only, connector.is_some_and(|status| status.enabled));

        // 5. Keep-alive: block until SIGINT / SIGTERM. Still no `app.run()` —
        //    `MockRuntime::run` is an infinite polling loop with no windowing
        //    system to drive, and this process's "event loop" is the signal
        //    wait below. (`build_headless_app` runs the one thing `run()` would
        //    otherwise have been needed for: the `setup` hook.)
        shutdown_signal.await;
        eprintln!("\n[codemux serve] shutting down…");
        Ok::<(), String>(())
    });

    // Mirror the GUI's `RunEvent::Exit` cleanup, then let `app` drop normally
    // so managed provider/server state gets its destructors. A success-path
    // `process::exit` would skip those destructors and leak child processes.
    // Tokio timers must be constructed inside the runtime. Keep running
    // the remaining cleanup if a delegation owner failed to shut down.
    let (drain_result, coordinator_result) = tauri::async_runtime::block_on(async {
        let drain_result = app.state::<ShutdownAdmission>().close_and_drain().await;
        crate::mcp_connector::listener::stop(app.handle());
        let remote = app.state::<web_remote::WebRemoteState>().shared();
        web_remote::stop_lan(&remote);
        web_remote::stop_relay(&remote);
        if let Some(platform) =
            app.try_state::<crate::plugins::platform_transport::PluginPlatformState>()
        {
            let _ = tokio::time::timeout(Duration::from_secs(3), platform.shutdown()).await;
        }
        let coordinator_result =
            if let Some(coordinator) = app.try_state::<crate::delegation::coordinator::Coordinator>() {
                coordinator.shutdown().await
            } else {
                Ok(())
            };
        // Managed state need not drop on exit. Stop OpenCode explicitly.
        if let Some(manager) = app.try_state::<
            std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>,
        >() {
            if let Err(error) = manager.shutdown().await {
                eprintln!("[codemux::opencode] shutdown incomplete: {error}");
            }
        }
        // Remote-workspace SSH tunnels live in a global registry.
        #[cfg(unix)]
        let _ = tokio::time::timeout(Duration::from_secs(3), crate::ssh::shutdown_all()).await;
        (drain_result, coordinator_result)
    });
    let persistence_result = flush_shutdown_state(app.handle(), drain_result);
    crate::agent_browser::kill_stream_daemons();
    drop(app);
    result.and(coordinator_result).and(persistence_result)
}

fn flush_shutdown_state<R: Runtime>(app: &AppHandle<R>, drain_result: Result<(), String>) -> Result<(), String> {
    // A timed-out operation can still be running. Save the current snapshot
    // and report the timeout so the caller knows the drain was incomplete.
    let layout_result =
        crate::state::finish_layout_persistence(&app.state::<crate::state::AppStateStore>());
    let selection_result = crate::active_workspace_persistence::finish_persistence(app)
        .map_err(|error| format!("Failed to save the active workspace: {error}"));
    drain_result.and(layout_result).and(selection_result)
}

/// Which bind scope a `codemux serve` startup should request, given the
/// `--scope` flag and whether remote access was already persisted-enabled.
/// `None` means "leave the persisted scope alone". Kept pure so precedence is
/// unit-testable:
///
/// - an explicit `--scope` always wins;
/// - no flag on a machine that was already configured keeps its scope;
/// - no flag on first run defaults to `all` — the away-from-home SSH case,
///   where nothing else would be reachable.
fn resolve_scope(explicit: Option<String>, persisted_enabled: bool) -> Option<String> {
    match (explicit, persisted_enabled) {
        (Some(s), _) => Some(s),
        (None, true) => None,                     // keep persisted scope
        (None, false) => Some(BIND_SCOPE_ALL.to_string()), // first-run default
    }
}

/// The startup half of `codemux serve`: resolve the scope, bind through the
/// shared enable path, and apply `--relay`. Split out of [`run_serve`] — which
/// blocks on a shutdown signal and can therefore never be called from a test —
/// so the integration suite can drive the exact production startup sequence
/// against a seeded settings row.
///
/// Everything persisted is authoritative unless a flag overrides it — including
/// an explicit relay-only setup, which stays relay-only (nothing listens on the
/// network) unless `--scope` / `--port` asks for the listener. The app
/// hydrated the settings row during `setup` (`restore_on_boot`), and
/// `control_enable` re-checks that before folding in `--scope` / `--port`, so a
/// bare `codemux serve` re-binds exactly what the user configured — including
/// starting the iroh relay transport when `relay_mode_enabled` was persisted
/// on, which is what makes the `codemux connect` systemd unit's bare
/// `ExecStart=… serve` correct.
pub async fn serve_startup<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    opts: &ServeOptions,
) -> Result<web_remote::ControlEnableResult, String> {
    // `restore_on_boot` may already have bound the server (GUI mode); under
    // serve it only hydrates, and `control_enable` handles both (`already_running`).
    let persisted = web_remote::web_remote_status(handle.clone());
    // Decide relay-only on the switches as persisted, BEFORE `--relay` below
    // turns relay mode on (which would make a fresh config look relay-only).
    // An explicit relay-only setup stays relay-only even with the kill switch
    // off, and then keeps its persisted scope too — the first-run `all`
    // default below exists to open a listener.
    let keep_relay_only = web_remote::keeps_relay_only(
        persisted.relay_mode_enabled,
        persisted.lan_enabled,
        opts.scope.is_some() || opts.port.is_some(),
    );
    let scope = if keep_relay_only {
        None
    } else {
        resolve_scope(opts.scope.clone(), persisted.enabled)
    };

    // `--relay` first: flip relay mode on through the same config path the
    // Settings pane uses, BEFORE binding. Relay mode does not need the LAN
    // listener, so with it on, the enable below keeps running (relay-only)
    // even if the listener can't bind — ordering it after the enable would let
    // a taken port abort serve before relay mode was ever recorded. The flag is
    // NOT passed as `Some(false)` when absent, so an omitted `--relay` can
    // never turn a persisted relay off.
    if opts.relay {
        if let Err(e) = web_remote::control_set_relay(handle, true).await {
            eprintln!("[codemux serve] relay transport could not be enabled: {e}");
        }
    }

    // As configured, not "open the listener": a relay-only setup (the user
    // switched "On my network" off) must not come back listening on the LAN
    // after a restart. Explicit `--scope` / `--port` still turn it on.
    web_remote::control_enable_as_configured(handle, scope, opts.port, keep_relay_only)
        .await
        .map_err(|e| format!("could not enable the web-remote server: {e}"))
}

/// Print the "server bound" portion of the startup banner: port, bind scope,
/// and the reachable HTTP endpoints from the shared enumeration.
fn print_banner<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    result: &web_remote::ControlEnableResult,
) {
    let port = result.status.port;
    let scope = result.status.bind_scope.as_str();
    let scope_note = match scope {
        "tailscale" => "tailnet + loopback only",
        "loopback" => "this machine only",
        _ => "every interface",
    };

    println!();
    if result.already_running {
        println!("Codemux headless server is already running.");
    } else {
        println!("Codemux headless server started.");
    }
    if !result.status.lan_enabled {
        // An explicit relay-only setup, kept as configured: there is no port
        // or scope to report, and no address to list.
        println!("  Mode:         relay only — nothing listens on your network");
        println!(
            "  Relay:        {}",
            if result.status.relay_running { "up" } else { "starting" }
        );
        return;
    }
    println!("  Port:         {port}");
    println!("  Access scope: {scope} ({scope_note})");
    if scope == BIND_SCOPE_ALL {
        println!("                Plain HTTP on every interface. On untrusted networks use --scope tailscale or --relay.");
    }
    if let Some(err) = &result.status.bind_error {
        // Relay mode kept the server up without its LAN listener; say so
        // rather than printing addresses nothing is listening on.
        println!("  LAN listener: not running — {err}");
        println!("                (retrying in the background; relay access is unaffected)");
        return;
    }

    // Enumerate the reachable endpoints (the same list the Settings pane and
    // `codemux remote enable` surface).
    let endpoints = web_remote::web_remote_list_endpoints(handle.clone());
    if !endpoints.is_empty() {
        println!("  Reachable at:");
        for ep in &endpoints {
            let secure = if ep.secure { "secure context" } else { "plain HTTP" };
            println!("    {}  ({}, {})", ep.url, ep.kind, secure);
        }
    }
}

/// The banner line for the outside-assistant connector, `None` while it is off.
/// The connector listens on loopback only, so an origin is a proxy's address.
fn connector_line(status: &crate::mcp_connector::ConnectorStatus) -> Option<String> {
    if !status.enabled {
        return None;
    }
    Some(match (&status.local_url, &status.public_origin) {
        (Some(_), Some(origin)) => format!(
            "Agent connector: {origin}/mcp (proxy target http://127.0.0.1:{})",
            status.port
        ),
        (Some(local), None) => format!("Agent connector: {local} (this machine only)"),
        (None, _) => format!(
            "Agent connector: not running — {}",
            status.bind_error.as_deref().unwrap_or("the listener is stopped")
        ),
    })
}

/// Print the closing lines with the supported administration path and stop hint.
fn print_footer(relay_only: bool, connector_enabled: bool) {
    println!();
    if relay_only {
        println!("Remote Access is using the saved relay setting; stop serve to change its network scope in desktop Settings.");
    } else {
        println!("Stop serve to manage pairing and Remote Access in desktop Settings.");
    }
    if !connector_enabled {
        println!("The agent connector is off; enable it in desktop Settings before starting headless serve.");
    }
    println!("Account-mode / hosted access continues to work if it was configured.");
    println!("Press Ctrl-C (or send SIGTERM) to stop the server.");
}

/// Register the signal listeners and return the future that waits for them.
#[cfg(unix)]
fn register_shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    })
}

#[cfg(windows)]
fn register_shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        let _ = ctrl_c.recv().await;
    })
}

/// Dev-only: does the source-tree `dist/` (the debug asset fallback the
/// web-remote server uses) actually contain `index.html`? Mirrors
/// `server::dev_dist_fallback`'s base path so the caveat warning matches what
/// the server can actually serve.
#[cfg(debug_assertions)]
fn dev_dist_available() -> bool {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dist/index.html")
        .exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn timed_out_shutdown_keeps_requests_closed() {
        let admission = ShutdownAdmission::default();
        let in_flight = admission.try_admit().unwrap();
        assert!(admission.close_and_drain().await.is_err());
        drop(in_flight);
        assert!(admission.try_admit().is_err());
    }

    #[test]
    fn shutdown_saves_the_current_layout_even_when_the_drain_times_out() {
        if crate::test_isolation::isolated(
            "web_remote::serve::tests::shutdown_saves_the_current_layout_even_when_the_drain_times_out",
        ) {
            return;
        }
        let root = crate::test_isolation::isolated_root().unwrap();
        let app = tauri::test::mock_app();
        let state = crate::state::AppStateStore::default();
        let workspace = state.create_workspace_at_path(root.clone());
        app.manage(state);

        let error = "fixture drain timeout";
        assert_eq!(flush_shutdown_state(app.handle(), Err(error.into())).unwrap_err(), error);
        let layout = std::fs::read(root.join("layout.json")).unwrap();
        let layout: serde_json::Value = serde_json::from_slice(&layout).unwrap();
        assert!(layout["workspaces"].as_array().unwrap().iter().any(|entry| {
            entry["workspace_id"].as_str() == Some(workspace.0.as_str())
        }));
    }

    #[test]
    fn explicit_scope_flag_wins_over_persisted_config() {
        // `codemux serve --scope loopback` on a box configured for `all`.
        assert_eq!(
            resolve_scope(Some("loopback".to_string()), true),
            Some("loopback".to_string())
        );
        assert_eq!(
            resolve_scope(Some("tailscale".to_string()), false),
            Some("tailscale".to_string())
        );
    }

    #[test]
    fn no_scope_flag_keeps_a_persisted_enabled_scope() {
        // `None` = "don't touch the scope", so the persisted one survives the
        // enable. This is what makes the `codemux connect` systemd unit's bare
        // `ExecStart=… serve` honour a tuned setup instead of resetting it.
        assert_eq!(resolve_scope(None, true), None);
    }

    fn connector(enabled: bool, origin: Option<&str>, running: bool) -> crate::mcp_connector::ConnectorStatus {
        crate::mcp_connector::ConnectorStatus {
            enabled,
            public_origin: origin.map(str::to_owned),
            listener_running: running,
            port: 4378,
            bind_error: (!running).then(|| "bind 127.0.0.1:4378: Address in use".into()),
            local_url: running.then(|| "http://127.0.0.1:4378/mcp".into()),
            mcp_url: None,
            local_executable: "codemux".into(),
            local_command: "codemux mcp".into(),
            revoked_grants: None,
            pending: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn banner_names_the_connector_address_or_why_it_is_not_running() {
        assert_eq!(connector_line(&connector(false, None, false)), None);
        assert_eq!(
            connector_line(&connector(true, None, true)).as_deref(),
            Some("Agent connector: http://127.0.0.1:4378/mcp (this machine only)")
        );
        assert_eq!(
            connector_line(&connector(true, Some("https://box.example"), true)).as_deref(),
            Some("Agent connector: https://box.example/mcp (proxy target http://127.0.0.1:4378)")
        );
        assert_eq!(
            connector_line(&connector(true, Some("https://box.example"), false)).as_deref(),
            Some("Agent connector: not running — bind 127.0.0.1:4378: Address in use")
        );
    }

    #[test]
    fn no_scope_flag_on_first_run_defaults_to_all() {
        // Nothing configured yet: bind every interface, because the SSH box
        // this runs on is reachable no other way.
        assert_eq!(resolve_scope(None, false), Some("all".to_string()));
    }
}
