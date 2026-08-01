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
    // 1. Mutual exclusion: never boot a second backend alongside a running GUI
    //    or serve instance (they would fight over the control socket, the DB,
    //    the PTY daemon, and the web-remote port).
    //
    //    Both directions of the guard key off the SAME fact — who holds the
    //    control endpoint — so it only works if `serve` actually takes it: the
    //    check below stops a second `serve` (or a `serve` next to a GUI), and
    //    `main.rs` runs the identical check before building the GUI. The
    //    control server is spawned from the Tauri `setup` hook, which is why
    //    `build_headless_app` has to run that hook (see step 2). While setup
    //    was skipped, `serve` never took the endpoint: the GUI happily booted
    //    beside it, a second `serve` got as far as failing on the web-remote
    //    port with an Address-in-use error instead of this clear message, and
    //    `codemux remote pair` / `connect status` reported nothing running.
    if crate::control::control_server_is_running() {
        return Err(
            "Codemux is already running on this machine (GUI or serve) — stop it first, \
             or use `codemux remote enable` to control the running instance."
                .to_string(),
        );
    }

    // 2. Boot the full app headless (DB, state, control server, PTY warmup,
    //    web_remote restore_on_boot, every background loop) on MockRuntime.
    //    `build_headless_app` runs the Tauri `setup` hook explicitly — Tauri
    //    itself only runs it from the event loop, which serve never enters —
    //    so the control server is listening (mutual exclusion in both
    //    directions, `codemux remote pair`, `codemux connect status`,
    //    `web_remote_set_relay`) and the persisted web-remote config is
    //    hydrated before the enable below merges the CLI flags into it.
    let app = match crate::build_headless_app() {
        Ok(app) => app,
        Err(e) => return Err(format!("failed to boot the headless backend: {e}")),
    };
    let handle = app.handle().clone();

    // Everything from here is async: run it — and the signal wait — inside one
    // `block_on`. `app` stays owned in this scope so the backend lives for the
    // whole session; dropping it would tear the runtime down.
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

        // 4. Startup banner + pairing code.
        print_banner(&handle, &result);
        match web_remote::control_pair(&handle, None) {
            Ok(pairing) => {
                let value = serde_json::to_value(&pairing).unwrap_or(serde_json::Value::Null);
                crate::cli::print_pairing(&value);
            }
            Err(e) => {
                // Shouldn't happen once the server is enabled — keep serving so
                // `codemux remote pair` from another session can still work.
                eprintln!(
                    "[codemux serve] could not mint a pairing code: {e} \
                     (the server is still running — run `codemux remote pair` to try again)"
                );
            }
        }
        print_footer();

        // 5. Keep-alive: block until SIGINT / SIGTERM. Still no `app.run()` —
        //    `MockRuntime::run` is an infinite polling loop with no windowing
        //    system to drive, and this process's "event loop" is the signal
        //    wait below. (`build_headless_app` runs the one thing `run()` would
        //    otherwise have been needed for: the `setup` hook.)
        wait_for_shutdown_signal().await;
        eprintln!("\n[codemux serve] shutting down…");
        Ok::<(), String>(())
    });

    // Mirror the GUI's `RunEvent::Exit` cleanup, then let `app` drop normally
    // so managed provider/server state gets its destructors. A success-path
    // `process::exit` would skip those destructors and leak child processes.
    crate::agent_browser::kill_stream_daemons();
    drop(app);
    result
}

/// Which bind scope a `codemux serve` startup should request, given the
/// `--scope` flag and whether remote access was already persisted-enabled.
/// `None` means "leave the persisted scope alone". Pure so the documented
/// precedence (docs/features/web-remote-access.md § "Headless server mode") is
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
/// Everything persisted is authoritative unless a flag overrides it: the app
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
    let persisted_enabled = web_remote::web_remote_status(handle.clone()).enabled;
    let scope = resolve_scope(opts.scope.clone(), persisted_enabled);

    let mut result = web_remote::control_enable(handle, scope, opts.port)
        .await
        .map_err(|e| format!("could not enable the web-remote server: {e}"))?;

    // `--relay`: flip relay mode on through the same config path the Settings
    // pane uses; because the server is now running, `set_config_core` also
    // starts the iroh endpoint + registration. (When relay was already
    // persisted-on, `control_enable` above started it and this is a no-op —
    // note it is NOT passed as `Some(false)` when the flag is absent, so an
    // omitted `--relay` can never turn a persisted relay off.)
    if opts.relay {
        match web_remote::web_remote_set_config(
            handle.clone(),
            None,
            None,
            None,
            None,
            None,
            Some(true),
        )
        .await
        {
            Ok(status) => result.status = status,
            Err(e) => eprintln!("[codemux serve] relay transport could not be enabled: {e}"),
        }
    }

    Ok(result)
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
    println!("  Port:         {port}");
    println!("  Access scope: {scope} ({scope_note})");

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

/// Print the closing lines: how to mint more codes, the account-mode note, and
/// the stop hint.
fn print_footer() {
    println!();
    println!("Run `codemux remote pair` from another SSH session to mint more pairing codes.");
    println!("Account-mode / hosted access continues to work if it was configured.");
    println!("Press Ctrl-C (or send SIGTERM) to stop the server.");
}

/// Block until the process receives SIGINT or SIGTERM. On non-Unix, waits on
/// Ctrl-C only (SIGTERM has no portable equivalent).
#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    // If a handler can't be installed, fall back to Ctrl-C so we never busy-spin.
    match (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) {
        (Ok(mut sigint), Ok(mut sigterm)) => {
            tokio::select! {
                _ = sigint.recv() => {}
                _ = sigterm.recv() => {}
            }
        }
        _ => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
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

    #[test]
    fn no_scope_flag_on_first_run_defaults_to_all() {
        // Nothing configured yet: bind every interface, because the SSH box
        // this runs on is reachable no other way.
        assert_eq!(resolve_scope(None, false), Some("all".to_string()));
    }
}
