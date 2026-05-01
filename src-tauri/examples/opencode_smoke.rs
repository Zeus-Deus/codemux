//! Step 12 Stage 2 live smoke test for the OpenCode model harvest.
//!
//! Spawns the singleton `opencode serve` child via
//! `OpenCodeServerManager`, hits `GET /provider`, and prints a
//! summary. Used as the deliverable evidence that
//! `opencode_list_models` returns real data on a developer box.
//!
//! Invocation:
//!
//! ```bash
//! cargo run --manifest-path src-tauri/Cargo.toml \
//!     --example opencode_smoke
//! ```
//!
//! Skips politely when `opencode` isn't on PATH so the example is
//! safe to run on CI / fresh worktrees.

use codemux_lib::agent_provider::opencode::{
    OpenCodeClient, OpenCodeClientConfig, OpenCodeServerManager,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if which::which("opencode").is_err() {
        eprintln!("opencode not on PATH; skipping smoke test");
        return Ok(());
    }

    let manager = OpenCodeServerManager::new();
    let handle = manager.ensure_running().await?;
    println!("server URL: {}", handle.base_url);
    println!(
        "server password length: {} (suppressed)",
        handle.server_password.len()
    );

    let mut config = OpenCodeClientConfig::new(handle.base_url.clone());
    config.server_password = Some(handle.server_password.clone());
    let client = OpenCodeClient::new(config)?;
    let providers = client.list_models().await?;

    let total_models: usize = providers.iter().map(|p| p.models.len()).sum();
    let connected: Vec<&str> = providers
        .iter()
        .filter(|p| p.connected)
        .map(|p| p.id.as_str())
        .collect();
    println!(
        "providers: {} (connected: {}); total models: {}",
        providers.len(),
        connected.len(),
        total_models
    );
    println!("connected provider ids: {:?}", connected);

    println!("\ntop 5 providers by model count:");
    let mut by_count: Vec<_> = providers.iter().collect();
    by_count.sort_by_key(|p| std::cmp::Reverse(p.models.len()));
    for p in by_count.iter().take(5) {
        println!("  {} ({} models)", p.id, p.models.len());
    }

    if let Some(connected_first) = providers.iter().find(|p| p.connected) {
        println!(
            "\nsample model from a connected provider ({} / {}):",
            connected_first.id,
            connected_first.name
        );
        if let Some((slug, model)) = connected_first.models.iter().next() {
            println!(
                "  slug={} name={} context_window={:?} variants={:?}",
                slug, model.name, model.context_window, model.variants
            );
        }
    }

    manager.stop().await;
    Ok(())
}
