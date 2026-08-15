// Cross-provider MCP server discovery + registry.
//
// Stage 1 (with Issue 1/2 follow-up): enumerate every place a user might
// keep MCP server configs, parse each into a unified `McpServerConfig`,
// dedupe identical configs that appear in multiple sources, and surface
// the results in the read-only Settings UI. No spawn or runtime here —
// Stage 2 will wire `JsonRpcChild` against the discovered specs.

pub mod codemux_self;
pub mod gateway;
pub mod http_client;
pub mod parser;
pub mod paths;
pub mod registry;
pub mod runtime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpConfigSource {
    /// Codemux's own hardcoded MCP server (always-on, not user-toggleable).
    Codemux,
    /// `~/.codemux/mcp.json` — the canonical Codemux-managed MCPs.
    CodemuxUser,
    /// `<project>/.codemux/mcp.json`
    CodemuxProject,
    /// `~/.claude.json` top-level `mcpServers` — the file Claude Code
    /// updates when the user runs `claude mcp add --scope user`.
    ClaudeUser,
    /// `~/.claude.json` `projects.<absolute-path>.mcpServers` — per-user
    /// per-project local scope ("local" in Claude Code's terminology).
    ClaudeLocal,
    /// `<project>/.mcp.json` — the project-checked-in scope shared with
    /// the team. Codemux's own `upsert_mcp_config` writes here too.
    ClaudeProject,
    /// `~/.cursor/mcp.json`
    CursorUser,
    /// `<project>/.cursor/mcp.json`
    CursorProject,
    /// `~/.codex/config.toml`
    CodexUser,
    /// `~/.config/opencode/opencode.json{,c}`
    OpenCodeUser,
    /// `<project>/opencode.json{,c}`
    OpenCodeProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    /// Streamable HTTP (and legacy `sse` entries where the endpoint also
    /// accepts POST requests).
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Stable id derived from `(canonical config path | server name)`.
    /// After dedupe, the canonical (lowest-rank) source's id wins.
    pub id: String,
    /// User-facing alias (the key inside `mcpServers`).
    pub name: String,
    /// All locations this exact config (same `command`+`args`+`env`) was
    /// found in. Length 1 for un-deduped entries; length > 1 when the
    /// same MCP is configured in multiple files. Codemux's hardcoded
    /// entry never merges into other sources — it stays its own row.
    pub sources: Vec<McpConfigSource>,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    /// Reflective of UI state — Stage 4 will persist this in zustand on
    /// the frontend. Always `false` from the backend until then.
    pub disabled: bool,
    pub transport: McpTransport,
    /// Pass-through for unknown fields so we don't lose forward-compat info
    /// when round-tripping configs.
    pub raw: serde_json::Value,
}

impl McpServerConfig {
    /// Lowest-rank (most canonical) source for this entry. After dedupe
    /// `sources[0]` is always the canonical one because we sort before
    /// merging.
    pub fn primary_source(&self) -> McpConfigSource {
        self.sources.first().copied().unwrap_or(McpConfigSource::Codemux)
    }
}

/// Stable id for an MCP server entry. Truncated SHA-256 hex (16 chars) over
/// `<config-path>|<server-name>` — same shape as `skill_id_for_path` so
/// the frontend can treat ids opaquely.
pub fn mcp_server_id(config_path: &str, server_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config_path.as_bytes());
    hasher.update(b"|");
    hasher.update(server_name.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

/// Lower number = more canonical. Used both for sort-before-dedupe and for
/// the Settings UI's group ordering. Keep in sync with the TS-side
/// `SOURCE_ORDER` constant in `mcp-section.tsx`.
pub fn source_rank(s: McpConfigSource) -> u8 {
    match s {
        McpConfigSource::Codemux => 0,
        McpConfigSource::CodemuxUser => 1,
        McpConfigSource::CodemuxProject => 2,
        McpConfigSource::ClaudeUser => 3,
        McpConfigSource::ClaudeLocal => 4,
        McpConfigSource::ClaudeProject => 5,
        McpConfigSource::CursorUser => 6,
        McpConfigSource::CursorProject => 7,
        McpConfigSource::CodexUser => 8,
        McpConfigSource::OpenCodeUser => 9,
        McpConfigSource::OpenCodeProject => 10,
    }
}

/// Merge identical configs across sources into single rows.
///
/// **Dedupe key:** `(name, command, args, env)`. Two entries with the
/// same name but different command/args stay as two separate rows so the
/// UI can show a source disambiguator (e.g. two different versions of
/// the same MCP).
///
/// **Codemux exception:** rows whose source is `McpConfigSource::Codemux`
/// (the hardcoded always-on entry) never merge with anything — they
/// always stay as their own row. This preserves the "always on" badge
/// even when a project file references Codemux's own command.
///
/// The input is expected to be sorted by `(source_rank, name)` already so
/// the first occurrence becomes canonical.
pub fn dedupe_servers(servers: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
    use std::collections::HashMap as Map;

    // (name, command, args, env-as-sorted-pairs) → index in `out`.
    type Key = (
        String,
        String,
        Vec<String>,
        Vec<(String, String)>,
        u8,
        Vec<(String, String)>,
    );
    let mut by_key: Map<Key, usize> = Map::new();
    let mut out: Vec<McpServerConfig> = Vec::with_capacity(servers.len());

    for srv in servers {
        let primary = srv.primary_source();
        // Hardcoded Codemux row: always its own row.
        if primary == McpConfigSource::Codemux {
            out.push(srv);
            continue;
        }

        let mut env_pairs: Vec<(String, String)> = srv
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env_pairs.sort();
        let mut http_headers = Vec::new();
        for field in ["headers", "http_headers", "env_http_headers"] {
            if let Some(values) = srv.raw.get(field).and_then(serde_json::Value::as_object) {
                for (name, value) in values {
                    http_headers.push((
                        name.to_ascii_lowercase(),
                        value.as_str().unwrap_or_default().to_string(),
                    ));
                }
            }
        }
        if let Some(env_name) = srv
            .raw
            .get("bearer_token_env_var")
            .and_then(serde_json::Value::as_str)
        {
            http_headers.push(("authorization".into(), format!("env:{env_name}")));
        }
        http_headers.sort();
        let transport = match srv.transport {
            McpTransport::Stdio => 0,
            McpTransport::Http => 1,
        };
        let key: Key = (
            srv.name.clone(),
            srv.command.clone(),
            srv.args.clone(),
            env_pairs,
            transport,
            http_headers,
        );

        if let Some(&idx) = by_key.get(&key) {
            // Merge: append the additional source(s) onto the canonical
            // row, deduplicating in case the input already had a merged
            // entry for some reason.
            for s in &srv.sources {
                if !out[idx].sources.contains(s) {
                    out[idx].sources.push(*s);
                }
            }
        } else {
            let new_idx = out.len();
            by_key.insert(key, new_idx);
            out.push(srv);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(name: &str, source: McpConfigSource, command: &str, args: &[&str]) -> McpServerConfig {
        McpServerConfig {
            id: mcp_server_id(&format!("/test/{:?}", source), name),
            name: name.into(),
            sources: vec![source],
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            disabled: false,
            transport: McpTransport::Stdio,
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn id_is_stable_and_deterministic() {
        let a = mcp_server_id("/home/u/.codemux/mcp.json", "github");
        let b = mcp_server_id("/home/u/.codemux/mcp.json", "github");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn id_differs_per_path_or_name() {
        let a = mcp_server_id("/a", "x");
        let b = mcp_server_id("/b", "x");
        let c = mcp_server_id("/a", "y");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn source_serializes_camel_case() {
        let v = serde_json::to_value(McpConfigSource::ClaudeUser).unwrap();
        assert_eq!(v, serde_json::Value::String("claudeUser".into()));
        let v = serde_json::to_value(McpConfigSource::ClaudeLocal).unwrap();
        assert_eq!(v, serde_json::Value::String("claudeLocal".into()));
    }

    #[test]
    fn transport_serializes_lowercase() {
        let v = serde_json::to_value(McpTransport::Stdio).unwrap();
        assert_eq!(v, serde_json::Value::String("stdio".into()));
    }

    #[test]
    fn server_serializes_camel_case_keys_with_sources_vec() {
        let cfg = make("github", McpConfigSource::CodemuxUser, "npx", &["-y", "@scope/srv"]);
        let json = serde_json::to_value(&cfg).unwrap();
        assert!(json.get("sources").is_some());
        assert!(json.get("source").is_none(), "old single-source field must not leak");
        assert_eq!(json["sources"][0], serde_json::Value::String("codemuxUser".into()));
    }

    #[test]
    fn dedupe_merges_identical_configs() {
        let a = make("omarchy-kb", McpConfigSource::ClaudeUser, "docker", &["exec", "-i", "x"]);
        let b = make("omarchy-kb", McpConfigSource::CursorUser, "docker", &["exec", "-i", "x"]);
        let result = dedupe_servers(vec![a, b]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].sources.len(), 2);
        assert_eq!(result[0].sources[0], McpConfigSource::ClaudeUser);
        assert_eq!(result[0].sources[1], McpConfigSource::CursorUser);
    }

    #[test]
    fn dedupe_keeps_distinct_configs_separate() {
        let a = make("omarchy-kb", McpConfigSource::ClaudeUser, "docker", &["exec", "-i", "v1"]);
        let b = make("omarchy-kb", McpConfigSource::CursorUser, "docker", &["exec", "-i", "v2"]);
        let result = dedupe_servers(vec![a, b]);
        assert_eq!(result.len(), 2, "different args must not collapse");
    }

    #[test]
    fn dedupe_respects_env_differences() {
        let mut a = make("server", McpConfigSource::ClaudeUser, "x", &[]);
        a.env.insert("KEY".into(), "v1".into());
        let mut b = make("server", McpConfigSource::CursorUser, "x", &[]);
        b.env.insert("KEY".into(), "v2".into());
        let result = dedupe_servers(vec![a, b]);
        assert_eq!(result.len(), 2, "different env values are different configs");
    }

    #[test]
    fn dedupe_merges_when_env_matches_exactly() {
        let mut a = make("server", McpConfigSource::ClaudeUser, "x", &[]);
        a.env.insert("KEY".into(), "v".into());
        let mut b = make("server", McpConfigSource::CursorUser, "x", &[]);
        b.env.insert("KEY".into(), "v".into());
        let result = dedupe_servers(vec![a, b]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].sources.len(), 2);
    }

    #[test]
    fn codemux_source_never_dedupes() {
        // Even if a project file references the exact same command, the
        // Codemux hardcoded row stays distinct so the always-on badge
        // never disappears.
        let cdmx = make("codemux", McpConfigSource::Codemux, "/usr/bin/codemux", &["mcp"]);
        let proj = make("codemux", McpConfigSource::ClaudeProject, "/usr/bin/codemux", &["mcp"]);
        let result = dedupe_servers(vec![cdmx, proj]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].sources, vec![McpConfigSource::Codemux]);
        assert_eq!(result[1].sources, vec![McpConfigSource::ClaudeProject]);
    }

    #[test]
    fn dedupe_preserves_canonical_order() {
        // Canonical (lower rank) source goes first in the merged sources
        // list — this is what the UI uses to pick the "primary" group.
        let claude = make("k", McpConfigSource::ClaudeUser, "x", &[]);
        let cursor = make("k", McpConfigSource::CursorUser, "x", &[]);
        // Caller must sort by source_rank first.
        let mut input = vec![cursor, claude];
        input.sort_by_key(|s| source_rank(s.primary_source()));
        let result = dedupe_servers(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].primary_source(), McpConfigSource::ClaudeUser);
    }

    #[test]
    fn primary_source_returns_first() {
        let mut srv = make("x", McpConfigSource::ClaudeUser, "y", &[]);
        srv.sources.push(McpConfigSource::CursorUser);
        assert_eq!(srv.primary_source(), McpConfigSource::ClaudeUser);
    }
}
