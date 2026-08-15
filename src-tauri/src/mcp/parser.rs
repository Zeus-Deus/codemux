// MCP config parser.
//
// Two file shapes are supported:
//
// 1. **Standard `mcpServers` files** (Claude `.mcp.json`, Cursor
//    `mcp.json`, Codemux `mcp.json`). Top-level object with one
//    `mcpServers` key. Handled by `parse_mcp_config_file`.
//
// 2. **Claude wrapped config** (`~/.claude.json`). A single
//    multi-section JSON file holding the user's entire Claude Code
//    state. Top-level `mcpServers` is the user scope; per-project
//    overrides live under `projects.<absolute-path>.mcpServers`. Handled
//    by `parse_claude_wrapped_config`.
//
// Unknown fields are preserved on `raw` so the parser stays
// forward-compatible with whatever upstream conventions add next.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{mcp_server_id, McpConfigSource, McpServerConfig, McpTransport};

/// Parse a standard `mcpServers`-shape file. Errors are returned for
/// unreadable files or malformed JSON; a syntactically valid file with no
/// `mcpServers` key returns `Ok(vec![])` (we surface "no servers" as
/// success, not failure, since users routinely have empty config files).
///
/// Per-server records that are not JSON objects are silently skipped — a
/// single bad entry should not nuke the rest of the file.
pub fn parse_mcp_config_file(
    path: &Path,
    source: McpConfigSource,
) -> Result<Vec<McpServerConfig>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;

    let root: Value = serde_json::from_str(&content)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;

    let servers = match root.get("mcpServers").and_then(Value::as_object) {
        Some(map) => map,
        None => return Ok(Vec::new()),
    };

    let path_str = path.display().to_string();
    let mut out = extract_servers(servers, &path_str, source);
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Parse Codex's TOML configuration (`[mcp_servers.<name>]`). Both local
/// `command` entries and remote `url` entries are normalized into the shared
/// registry representation.
pub fn parse_codex_config_file(path: &Path) -> Result<Vec<McpServerConfig>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    let root: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;
    let root = serde_json::to_value(root)
        .map_err(|e| format!("normalize {}: {}", path.display(), e))?;
    let Some(servers) = root.get("mcp_servers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut out = extract_servers(
        servers,
        &path.display().to_string(),
        McpConfigSource::CodexUser,
    );
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Parse OpenCode's JSON/JSONC configuration. Current OpenCode stores server
/// names directly under `mcp`; the v2 preview nests them under `mcp.servers`,
/// so the importer accepts both without coupling Codemux to one release line.
pub fn parse_opencode_config_file(
    path: &Path,
    source: McpConfigSource,
) -> Result<Vec<McpServerConfig>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    let root: Value = json5::from_str(&content)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;
    let Some(mcp) = root.get("mcp").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let servers = mcp
        .get("servers")
        .and_then(Value::as_object)
        .unwrap_or(mcp);
    let mut out = extract_servers(servers, &path.display().to_string(), source);
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Parse `~/.claude.json` — a multi-section file. Returns the union of
/// the top-level user-scope `mcpServers` (tagged `ClaudeUser`) and the
/// per-project local-scope entries under
/// `projects.<absolute-path>.mcpServers` (tagged `ClaudeLocal`) for the
/// active project, when one is given. Reading other projects' entries is
/// suppressed because they belong to other workspaces.
pub fn parse_claude_wrapped_config(
    path: &Path,
    project_root: Option<&Path>,
) -> Result<Vec<McpServerConfig>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;

    let root: Value = serde_json::from_str(&content)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;

    let path_str = path.display().to_string();
    let mut out: Vec<McpServerConfig> = Vec::new();

    // User scope.
    if let Some(servers) = root.get("mcpServers").and_then(Value::as_object) {
        out.extend(extract_servers(servers, &path_str, McpConfigSource::ClaudeUser));
    }

    // Per-project local scope. Claude keys these by the absolute project
    // path; we only surface the active project's entries so other
    // workspaces don't leak in.
    if let Some(root_path) = project_root {
        let key = root_path.display().to_string();
        if let Some(project_obj) = root
            .get("projects")
            .and_then(Value::as_object)
            .and_then(|m| m.get(&key))
            .and_then(Value::as_object)
        {
            if let Some(servers) =
                project_obj.get("mcpServers").and_then(Value::as_object)
            {
                // Use a synthetic id-path that distinguishes local-scope
                // entries from the same name in user-scope.
                let local_id_path = format!("{}#projects.{}", path_str, key);
                out.extend(extract_servers(
                    servers,
                    &local_id_path,
                    McpConfigSource::ClaudeLocal,
                ));
            }
        }
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Translate one `mcpServers` JSON object into our typed entries. Shared
/// between the standard parser and the wrapped-config parser — the only
/// thing that differs between callers is the source and the path used to
/// derive the id.
fn extract_servers(
    servers: &serde_json::Map<String, Value>,
    config_path: &str,
    source: McpConfigSource,
) -> Vec<McpServerConfig> {
    let mut out: Vec<McpServerConfig> = Vec::new();

    for (name, value) in servers {
        let obj = match value.as_object() {
            Some(o) => o,
            None => continue,
        };

        let transport_str = obj
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .unwrap_or_else(|| "stdio".to_string());
        let transport = match transport_str.as_str() {
            "http" | "sse" | "remote" | "streamable-http" => McpTransport::Http,
            _ if obj.contains_key("url") => McpTransport::Http,
            _ => McpTransport::Stdio,
        };

        let command_array = obj.get("command").and_then(Value::as_array);
        let command = match transport {
            McpTransport::Stdio => obj
                .get("command")
                .and_then(Value::as_str)
                .or_else(|| command_array.and_then(|parts| parts.first()).and_then(Value::as_str))
                .unwrap_or_default()
                .to_string(),
            McpTransport::Http => obj
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };

        let mut args: Vec<String> = obj
            .get("args")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if args.is_empty() {
            args = command_array
                .map(|parts| {
                    parts
                        .iter()
                        .skip(1)
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
        }

        let env: HashMap<String, String> = obj
            .get("env")
            .or_else(|| obj.get("environment"))
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        v.as_str().map(|s| (k.clone(), s.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(McpServerConfig {
            id: mcp_server_id(config_path, name),
            name: name.clone(),
            sources: vec![source],
            command,
            args,
            env,
            disabled: obj.get("disabled").and_then(Value::as_bool).unwrap_or(false)
                || obj.get("enabled").and_then(Value::as_bool) == Some(false),
            transport,
            raw: value.clone(),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &std::path::Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn parses_standard_format() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "github": {
                  "command": "npx",
                  "args": ["-y", "@modelcontextprotocol/server-github"],
                  "env": { "GITHUB_TOKEN": "abc" }
                }
              }
            }"#,
        );

        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::CodemuxUser).unwrap();
        assert_eq!(parsed.len(), 1);
        let s = &parsed[0];
        assert_eq!(s.name, "github");
        assert_eq!(s.command, "npx");
        assert_eq!(s.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(s.env.get("GITHUB_TOKEN"), Some(&"abc".to_string()));
        assert!(matches!(s.transport, McpTransport::Stdio));
        assert_eq!(s.sources, vec![McpConfigSource::CodemuxUser]);
        assert!(!s.disabled);
    }

    #[test]
    fn parses_multiple_servers_sorted_by_name() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "zeta":   { "command": "z" },
                "alpha":  { "command": "a" }
              }
            }"#,
        );

        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::ClaudeUser).unwrap();
        let names: Vec<_> = parsed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn server_without_env_defaults_empty() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(&p, r#"{ "mcpServers": { "x": { "command": "y" } } }"#);
        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::CodemuxUser).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].env.is_empty());
        assert!(parsed[0].args.is_empty());
    }

    #[test]
    fn missing_mcp_servers_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(&p, "{}");
        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::CodemuxUser).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn malformed_json_returns_error() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(&p, "{ not valid json");
        let result = parse_mcp_config_file(&p, McpConfigSource::CodemuxUser);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_fields_preserved_in_raw() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "weird": {
                  "command": "x",
                  "supportsParallelToolCalls": true,
                  "future_field": [1,2,3]
                }
              }
            }"#,
        );
        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::CodemuxUser).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].raw.get("supportsParallelToolCalls"),
            Some(&Value::Bool(true))
        );
        assert!(parsed[0].raw.get("future_field").is_some());
    }

    #[test]
    fn http_transport_recognized() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "linear": { "type": "http", "url": "https://mcp.linear.app/mcp" }
              }
            }"#,
        );
        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::ClaudeUser).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(matches!(parsed[0].transport, McpTransport::Http));
        assert_eq!(parsed[0].command, "https://mcp.linear.app/mcp");
    }

    #[test]
    fn non_object_entries_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("mcp.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "ok":   { "command": "a" },
                "junk": "not-an-object"
              }
            }"#,
        );
        let parsed =
            parse_mcp_config_file(&p, McpConfigSource::CodemuxUser).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "ok");
    }

    #[test]
    fn id_is_per_path_per_name() {
        let tmp = TempDir::new().unwrap();
        let p1 = tmp.path().join("a.json");
        let p2 = tmp.path().join("b.json");
        let body = r#"{ "mcpServers": { "x": { "command": "y" } } }"#;
        write(&p1, body);
        write(&p2, body);

        let a = parse_mcp_config_file(&p1, McpConfigSource::CodemuxUser).unwrap();
        let b = parse_mcp_config_file(&p2, McpConfigSource::CodemuxUser).unwrap();
        assert_ne!(a[0].id, b[0].id);
    }

    // ── parse_claude_wrapped_config ──────────────────────────────────

    #[test]
    fn wrapped_config_extracts_user_scope() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".claude.json");
        write(
            &p,
            r#"{
              "mcpServers": {
                "omarchy-kb": {
                  "type": "stdio",
                  "command": "docker",
                  "args": ["exec", "-i", "x"]
                }
              },
              "projects": {}
            }"#,
        );
        let parsed = parse_claude_wrapped_config(&p, None).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "omarchy-kb");
        assert_eq!(parsed[0].sources, vec![McpConfigSource::ClaudeUser]);
    }

    #[test]
    fn wrapped_config_extracts_project_local_scope() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".claude.json");
        let project_root = tmp.path().join("workspace");
        // The project key is interpolated into a JSON string literal,
        // so any backslashes the host's path uses (Windows tempdirs
        // resolve to `C:\Users\...`) must be JSON-escaped or the
        // parser rejects the doc with "invalid escape". Going through
        // `serde_json` produces the correctly-quoted-and-escaped form
        // for any platform.
        let project_key = serde_json::to_string(&project_root.display().to_string())
            .expect("serialize project key");
        let body = format!(
            r#"{{
              "mcpServers": {{
                "user-scoped": {{ "command": "u" }}
              }},
              "projects": {{
                {}: {{
                  "mcpServers": {{
                    "local-scoped": {{ "command": "l" }}
                  }}
                }}
              }}
            }}"#,
            project_key
        );
        write(&p, &body);

        let parsed =
            parse_claude_wrapped_config(&p, Some(&project_root)).unwrap();

        // Both scopes surface; sorted alphabetically by name.
        assert_eq!(parsed.len(), 2);
        let by_name: HashMap<_, _> =
            parsed.iter().map(|s| (s.name.as_str(), s)).collect();
        assert_eq!(
            by_name["user-scoped"].sources,
            vec![McpConfigSource::ClaudeUser]
        );
        assert_eq!(
            by_name["local-scoped"].sources,
            vec![McpConfigSource::ClaudeLocal]
        );
    }

    #[test]
    fn wrapped_config_other_projects_are_suppressed() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".claude.json");
        let project_root = tmp.path().join("active");
        // Same reason as `wrapped_config_extracts_project_local_scope`:
        // serialize through serde_json so Windows path backslashes
        // become valid JSON escapes.
        let project_key = serde_json::to_string(&project_root.display().to_string())
            .expect("serialize project key");
        let other_key = serde_json::to_string("/some/other/project").unwrap();
        let body = format!(
            r#"{{
              "mcpServers": {{}},
              "projects": {{
                {}: {{ "mcpServers": {{ "active-only": {{ "command": "a" }} }} }},
                {}: {{ "mcpServers": {{ "other-only":  {{ "command": "o" }} }} }}
              }}
            }}"#,
            project_key, other_key
        );
        write(&p, &body);

        let parsed =
            parse_claude_wrapped_config(&p, Some(&project_root)).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "active-only");
    }

    #[test]
    fn wrapped_config_no_project_root_only_user_scope() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".claude.json");
        write(
            &p,
            r#"{
              "mcpServers": { "u": { "command": "x" } },
              "projects": {
                "/some/path": { "mcpServers": { "p": { "command": "y" } } }
              }
            }"#,
        );
        let parsed = parse_claude_wrapped_config(&p, None).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "u");
    }

    #[test]
    fn wrapped_config_missing_top_level_is_ok() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".claude.json");
        write(&p, "{}");
        let parsed = parse_claude_wrapped_config(&p, None).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_codex_stdio_and_remote_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        write(
            &path,
            r#"
              [mcp_servers.local]
              command = "docker"
              args = ["exec", "-i", "server"]
              env = { TOKEN = "x" }

              [mcp_servers.docs]
              url = "https://example.test/mcp"
              http_headers = { X-Test = "yes" }
            "#,
        );
        let parsed = parse_codex_config_file(&path).unwrap();
        assert_eq!(parsed.len(), 2);
        let docs = parsed.iter().find(|server| server.name == "docs").unwrap();
        assert!(matches!(docs.transport, McpTransport::Http));
        assert_eq!(docs.command, "https://example.test/mcp");
        let local = parsed.iter().find(|server| server.name == "local").unwrap();
        assert_eq!(local.command, "docker");
        assert_eq!(local.args, vec!["exec", "-i", "server"]);
    }

    #[test]
    fn parses_opencode_jsonc_command_arrays_and_disabled_state() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("opencode.jsonc");
        write(
            &path,
            r#"{
              // JSONC is the documented OpenCode format.
              "mcp": {
                "local": {
                  "type": "local",
                  "command": ["uvx", "demo-server", "--flag"],
                  "environment": {"TOKEN": "x"}
                },
                "off": {
                  "type": "remote",
                  "url": "https://example.test/mcp",
                  "enabled": false
                }
              }
            }"#,
        );
        let parsed =
            parse_opencode_config_file(&path, McpConfigSource::OpenCodeUser).unwrap();
        let local = parsed.iter().find(|server| server.name == "local").unwrap();
        assert_eq!(local.command, "uvx");
        assert_eq!(local.args, vec!["demo-server", "--flag"]);
        assert_eq!(local.env.get("TOKEN").map(String::as_str), Some("x"));
        assert!(parsed.iter().find(|server| server.name == "off").unwrap().disabled);
    }
}
