//! Step 12 Stage 9 live smoke for the Codex capability harvest.
//!
//! Spawns a real `codex app-server`, runs the
//! `initialize` → `account/read` → `model/list` handshake, and prints
//! the resulting picker catalog. Skips politely when `codex` isn't on
//! PATH so the example is safe on CI / fresh worktrees.

use codemux_lib::agent_provider::codex::capabilities::{harvest_codex_capabilities, HarvestError};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(binary) = which::which("codex") else {
        eprintln!("[codex-smoke] codex not on PATH; skipping");
        return Ok(());
    };
    eprintln!("[codex-smoke] binary: {}", binary.display());

    match harvest_codex_capabilities(&binary, None).await {
        Ok(caps) => {
            eprintln!("[codex-smoke] harvested {} models", caps.models.len());
            for m in &caps.models {
                eprintln!(
                    "  - {} ({}) effort={:?} default={:?} images={} fast={}",
                    m.id,
                    m.label,
                    m.effort_levels,
                    m.default_effort,
                    m.supports_images,
                    m.supports_fast_mode
                );
            }
            eprintln!(
                "[codex-smoke] permission modes: {}",
                caps.permission_modes
                    .iter()
                    .map(|p| p.value.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Err(HarvestError::NotInstalled { hint }) => {
            eprintln!("[codex-smoke] codex_not_installed: {hint}");
        }
        Err(HarvestError::NotAuthenticated { hint }) => {
            eprintln!("[codex-smoke] codex_not_authenticated: {hint}");
            eprintln!("[codex-smoke] (this is fine — picker would render a clean login hint)");
        }
        Err(HarvestError::HarvestFailed { message }) => {
            eprintln!("[codex-smoke] codex_harvest_failed: {message}");
            std::process::exit(1);
        }
    }
    Ok(())
}
