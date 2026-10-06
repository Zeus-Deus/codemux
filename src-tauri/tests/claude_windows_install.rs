//! Native Windows CLI discovery through the actual compiled Bun sidecar.
//! Every scenario runs in its own process, so PATH/USERPROFILE changes cannot
//! leak into other tests. No real account, network, or inference is involved.
#![cfg(windows)]

use codemux_lib::agent_provider::{claude, health, ProviderKind};
use std::path::Path;

fn scenario(installed_before_startup: bool) {
    let root = tempfile::Builder::new()
        .prefix("Claude user profile with spaces ")
        .tempdir()
        .unwrap();
    let sidecar = claude::sidecar_path::resolve_sidecar_path()
        .expect("CI must stage the real Claude sidecar");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "windows_native_install_worker", "--nocapture"])
        .env(
            "CODEMUX_TEST_CLAUDE_INSTALL",
            if installed_before_startup {
                "before"
            } else {
                "after"
            },
        )
        .env("USERPROFILE", root.path())
        .env(
            "PATH",
            Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32"),
        )
        .env(claude::sidecar_path::SIDECAR_PATH_ENV, sidecar)
        .status()
        .expect("start isolated Windows install scenario");
    assert!(status.success(), "Windows install worker failed: {status}");
}

#[test]
fn windows_claude_installed_without_parent_path() {
    scenario(true);
}

#[test]
fn windows_claude_installed_after_desktop_startup() {
    scenario(false);
}

#[test]
fn windows_native_install_worker() {
    let Ok(scenario) = std::env::var("CODEMUX_TEST_CLAUDE_INSTALL") else {
        return;
    };
    let profile = std::env::var_os("USERPROFILE").unwrap();
    let local_bin = Path::new(&profile).join(".local/bin");
    let cli = local_bin.join("claude.exe");
    let install = || {
        std::fs::create_dir_all(&local_bin).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_fake_claude_sidecar"), &cli).unwrap();
    };
    assert!(
        which::which("claude").is_err(),
        "Parent must not already find Claude"
    );
    if scenario == "before" {
        install();
    }
    // This is the same initialization main() performs before creating any
    // runtime/provider. It must reserve the directory even before installation.
    codemux_lib::execution::initialize_user_cli_path();
    if scenario == "after" {
        assert!(which::which("claude").is_err());
        install();
    }
    assert_eq!(which::which("claude").unwrap(), cli);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sidecar = claude::sidecar_path::resolve_sidecar_path().unwrap();
        let probe = claude::auth::probe_installed(&sidecar, None).await.unwrap();
        assert!(probe.installed, "Real sidecar must discover the native CLI");
        assert_eq!(probe.version.as_deref(), Some("9.9.9"));
        let report = health::check_provider_health(ProviderKind::Claude).await;
        assert!(
            report.installed,
            "GUI health must distinguish missing CLI from missing login"
        );
        assert_eq!(report.version.as_deref(), Some("9.9.9"));
        assert!(report.message.unwrap().contains("not authenticated"));
    });
}
