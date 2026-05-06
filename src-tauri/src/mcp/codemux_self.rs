// Hardcoded entry for Codemux's own MCP server.
//
// Step 9 keeps the always-on Codemux MCP infrastructure separate from the
// user-toggleable list. The Settings UI pins this row to the top with an
// "always on" badge and no toggle. The runtime (Stage 2) treats it just
// like any other entry except `disabled` is locked to `false`.

use std::collections::HashMap;

use super::{McpConfigSource, McpServerConfig, McpTransport};

/// Build the descriptor for Codemux's own MCP server. Mirrors what
/// `mcp_server::codemux_mcp_entry` writes into project `.mcp.json` files
/// today: `<current_exe> mcp` with no per-server env (Stage 2 will inject
/// the workspace id at spawn time, the same way the workspace lifecycle
/// upserts do at `commands/workspace.rs:75` etc.).
pub fn codemux_self_config() -> McpServerConfig {
    let command = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "codemux".to_string());

    McpServerConfig {
        id: "codemux-self".to_string(),
        name: "codemux".to_string(),
        sources: vec![McpConfigSource::Codemux],
        command,
        args: vec!["mcp".to_string()],
        env: HashMap::new(),
        // Always-on. Toggling is suppressed in the UI; the field is here
        // only to keep the type uniform with user-installed entries.
        disabled: false,
        transport: McpTransport::Stdio,
        raw: serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_matches_published_entry_shape() {
        let cfg = codemux_self_config();
        assert_eq!(cfg.name, "codemux");
        assert_eq!(cfg.id, "codemux-self");
        assert_eq!(cfg.sources, vec![McpConfigSource::Codemux]);
        assert_eq!(cfg.args, vec!["mcp".to_string()]);
        assert!(cfg.env.is_empty());
        assert!(!cfg.disabled);
        assert!(matches!(cfg.transport, McpTransport::Stdio));
        // The command is non-empty even when current_exe fails (falls back
        // to the literal "codemux").
        assert!(!cfg.command.is_empty());
    }

    #[test]
    fn id_is_stable() {
        let a = codemux_self_config();
        let b = codemux_self_config();
        assert_eq!(a.id, b.id);
    }
}
