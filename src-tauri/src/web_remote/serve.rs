//! Headless `codemux serve` entrypoint (issue #176, Phase 3).
//!
//! Boots the FULL Codemux backend headless on `tauri::test::MockRuntime` (via
//! [`crate::build_headless_app`]) with no display attached, binds the
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

use crate::web_remote;

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
/// returns (the process exits cleanly). Exits non-zero — before booting — if
/// another Codemux instance already holds this machine's control endpoint.
pub fn run_serve(opts: ServeOptions) {
    // 1. Mutual exclusion: never boot a second backend alongside a running GUI
    //    or serve instance (they would fight over the control socket, the DB,
    //    the PTY daemon, and the web-remote port).
    if crate::control::control_server_is_running() {
        eprintln!(
            "Codemux is already running on this machine (GUI or serve) — stop it first, \
             or use `codemux remote enable` to control the running instance."
        );
        std::process::exit(1);
    }

    // 2. Boot the full app headless (DB, state, control server, PTY warmup,
    //    web_remote restore_on_boot, every background loop) on MockRuntime.
    let app = match crate::build_headless_app() {
        Ok(app) => app,
        Err(e) => {
            eprintln!("[codemux serve] failed to boot the headless backend: {e}");
            std::process::exit(1);
        }
    };
    let handle = app.handle().clone();

    // Everything from here is async: run it — and the signal wait — inside one
    // `block_on`. `app` stays owned in this scope so the backend lives for the
    // whole session; dropping it would tear the runtime down.
    tauri::async_runtime::block_on(async move {
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

        // 3. Enable the web-remote server through the SHARED enable path so
        //    bind-scope validation, rollback, config persistence, and
        //    `web-remote-state-changed` all behave identically to the GUI.
        //
        //    `restore_on_boot` may have already bound the server if config was
        //    persisted-enabled; `control_enable` handles that gracefully
        //    (`already_running`). Scope resolution: an explicit `--scope` wins;
        //    otherwise respect the persisted scope when remote access was
        //    already configured; else default to `all` for the SSH use case.
        let persisted_enabled = web_remote::web_remote_status(handle.clone()).enabled;
        let scope = match (&opts.scope, persisted_enabled) {
            (Some(_), _) => opts.scope.clone(),
            (None, true) => None,                       // keep persisted scope
            (None, false) => Some("all".to_string()),   // first-run default
        };

        let result = match web_remote::control_enable(&handle, scope, opts.port).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[codemux serve] could not enable the web-remote server: {e}");
                std::process::exit(1);
            }
        };

        // `--relay`: flip relay mode on through the same config path the
        //  Settings pane uses; because the server is now running,
        //  `set_config_core` also starts the iroh endpoint + registration.
        //  (When relay was already persisted-on, `enable_core` above started
        //  it, and this is a no-op.)
        if opts.relay {
            if let Err(e) =
                web_remote::web_remote_set_config(handle.clone(), None, None, None, None, None, Some(true))
                    .await
            {
                eprintln!("[codemux serve] relay transport could not be enabled: {e}");
            }
        }

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

        // 5. Keep-alive: block until SIGINT / SIGTERM. No `app.run()` — the
        //    MockRuntime has no event loop to drive.
        wait_for_shutdown_signal().await;
        eprintln!("\n[codemux serve] shutting down…");
    });

    // Graceful shutdown — mirror the `RunEvent::Exit` cleanup in `run()`.
    crate::agent_browser::kill_stream_daemons();
    std::process::exit(0);
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
