//! `codemux doctor` — local environment diagnostics.
//!
//! Runs entirely offline against the local machine (no control
//! socket, no running app needed) so it works precisely when the app
//! itself is misbehaving. Designed for support: "run `codemux doctor`
//! and paste the output" should be enough to triage environment
//! problems like issue #95 (file dialogs silently failing on minimal
//! window-manager setups).

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

pub async fn run() {
    println!("codemux doctor (v{})", env!("CARGO_PKG_VERSION"));
    println!();

    // ── Environment ─────────────────────────────────────────────
    println!("environment");
    println!("  os:               {}", std::env::consts::OS);
    #[cfg(target_os = "linux")]
    {
        println!(
            "  desktop:          {}",
            env_or("XDG_CURRENT_DESKTOP", "(unset)")
        );
        println!(
            "  session type:     {}",
            env_or("XDG_SESSION_TYPE", "(unset)")
        );
        println!(
            "  wayland display:  {}",
            env_or("WAYLAND_DISPLAY", "(unset)")
        );
        println!("  x11 display:      {}", env_or("DISPLAY", "(unset)"));
        println!(
            "  session bus:      {}",
            env_or("DBUS_SESSION_BUS_ADDRESS", "(unset)")
        );
    }
    println!();

    // ── File dialogs (Linux-only concern) ───────────────────────
    // The compiled dialog backend is portal-first with a zenity
    // fallback; macOS/Windows use native dialogs that need nothing.
    #[cfg(target_os = "linux")]
    {
        println!("file dialogs");
        let diagnosis = crate::dialog_preflight::diagnose().await;
        match &diagnosis.portal {
            Ok(version) => println!(
                "  [ok]   desktop portal file chooser available (interface version {version})"
            ),
            Err(reason) => {
                println!("  [FAIL] desktop portal file chooser unavailable: {reason}")
            }
        }
        match &diagnosis.zenity {
            Some(path) => println!("  [ok]   zenity fallback found at {}", path.display()),
            None => println!("  [warn] zenity fallback not found on PATH"),
        }
        if diagnosis.usable() {
            println!("  [ok]   file dialogs should work");
        } else {
            println!("  [FAIL] file dialogs cannot open on this system.");
            let reason = diagnosis.portal.as_ref().err().cloned().unwrap_or_default();
            println!(
                "         {}",
                crate::dialog_preflight::no_backend_remediation(&reason)
            );
        }
        println!();
    }

    // ── Logs ────────────────────────────────────────────────────
    println!("logs");
    match crate::app_logs::app_log_file() {
        Some(path) if path.exists() => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("  [ok]   log file: {} ({size} bytes)", path.display());
            println!("         view recent entries with: codemux logs");
        }
        Some(path) => {
            println!("  [warn] no log file yet at {}", path.display());
            println!("         it is created the first time the desktop app runs");
        }
        None => println!("  [warn] could not resolve the platform log directory"),
    }
}
