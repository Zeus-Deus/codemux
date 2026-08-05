//! End-to-end tests for child-process GUI environment hygiene.

use codemux_lib::execution::{
    gui_env_keys, gui_env_overrides, sanitize_gui_env_std, sanitize_gui_env_tokio,
};
use std::collections::{HashMap, HashSet};
use std::process::Command;

#[cfg(target_os = "linux")]
fn run_env(cmd: &mut Command) -> HashMap<String, String> {
    let output = cmd.output().expect("spawn env");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[cfg(target_os = "linux")]
async fn run_env_tokio(cmd: &mut tokio::process::Command) -> HashMap<String, String> {
    let output = cmd.output().await.expect("spawn env");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
#[cfg(target_os = "linux")]
fn std_sanitizer_strips_display_and_sets_neutralizers() {
    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":999")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test-instance");
    sanitize_gui_env_std(&mut cmd);
    let env = run_env(&mut cmd);

    assert!(!env.contains_key("DISPLAY"));
    assert!(!env.contains_key("WAYLAND_DISPLAY"));
    assert!(!env.contains_key("HYPRLAND_INSTANCE_SIGNATURE"));
    assert_eq!(env.get("BROWSER").map(String::as_str), Some("true"));
    assert_eq!(env.get("MOZ_NO_REMOTE").map(String::as_str), Some("1"));
    assert_eq!(
        env.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/dev/null")
    );
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn tokio_sanitizer_matches_std_contract() {
    let mut cmd = tokio::process::Command::new("env");
    cmd.env("DISPLAY", ":999")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test-instance");
    sanitize_gui_env_tokio(&mut cmd);
    let env = run_env_tokio(&mut cmd).await;

    assert!(!env.contains_key("DISPLAY"));
    assert!(!env.contains_key("WAYLAND_DISPLAY"));
    assert!(!env.contains_key("HYPRLAND_INSTANCE_SIGNATURE"));
    assert_eq!(env.get("BROWSER").map(String::as_str), Some("true"));
    assert_eq!(
        env.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/dev/null")
    );
}

#[test]
#[cfg(target_os = "linux")]
fn sanitizer_preserves_unrelated_environment() {
    let mut cmd = Command::new("/usr/bin/env");
    cmd.env("PATH", "/custom").env("CODEMUX_WORKSPACE_ID", "ws-1");
    sanitize_gui_env_std(&mut cmd);
    let env = run_env(&mut cmd);

    assert_eq!(env.get("PATH").map(String::as_str), Some("/custom"));
    assert_eq!(
        env.get("CODEMUX_WORKSPACE_ID").map(String::as_str),
        Some("ws-1")
    );
}

#[test]
#[cfg(target_os = "linux")]
fn sanitizer_handles_every_declared_gui_key() {
    let mut cmd = Command::new("env");
    for key in gui_env_keys() {
        cmd.env(key, "SENTINEL");
    }
    sanitize_gui_env_std(&mut cmd);
    let env = run_env(&mut cmd);
    let overrides: HashMap<&str, &str> = gui_env_overrides().iter().copied().collect();

    for key in gui_env_keys() {
        match overrides.get(key) {
            Some(expected) => assert_eq!(env.get(*key).map(String::as_str), Some(*expected)),
            None => assert!(!env.contains_key(*key), "{key} leaked into child"),
        }
    }
}

#[test]
fn gui_environment_lists_have_no_duplicates() {
    let keys: HashSet<&str> = gui_env_keys().iter().copied().collect();
    assert_eq!(keys.len(), gui_env_keys().len());

    let override_keys: HashSet<&str> =
        gui_env_overrides().iter().map(|(key, _)| *key).collect();
    assert_eq!(override_keys.len(), gui_env_overrides().len());
}
