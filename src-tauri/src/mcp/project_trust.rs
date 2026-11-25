//! Local consent for MCP servers defined by a project's own files.
//!
//! `<project>/.codemux/mcp.json`, `.mcp.json`, `.cursor/mcp.json` and
//! `opencode.json{,c}` are repository content: whoever wrote the repository
//! chose their commands, URLs and headers. Codemux starts such a server, or
//! sends it environment-derived credentials, only after the user approves
//! that exact definition for that project on this computer.
//!
//! Approvals live in the local database, never in the repository. Each one
//! is keyed by a hash of the canonical project root and the server's full
//! definition, so a change to the command, arguments, environment, working
//! directory, URL or headers asks again.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};

use super::{is_project_source, McpServerConfig, McpTransport};
use crate::database::DatabaseStore;

const SETTING_PREFIX: &str = "mcp.project_approval:";

/// Keys the registry adds to `raw` at runtime. They are not part of what
/// the project defines.
const RUNTIME_KEYS: [&str; 2] = ["runtimeGeneration", "pluginRegistrationToken"];

/// The approval fingerprint for a server that needs one, or `None` when the
/// definition Codemux would start comes from a user-owned file.
///
/// Dedupe can merge an identical user entry into a project row and the
/// other way round; the row starts the definition of its primary (first)
/// source, so that source alone decides.
pub fn fingerprint(config: &McpServerConfig) -> Option<String> {
    if !is_project_source(config.primary_source()) {
        return None;
    }
    let mut raw = config.raw.clone();
    if let Some(raw) = raw.as_object_mut() {
        for key in RUNTIME_KEYS {
            raw.remove(key);
        }
    }
    let env: BTreeMap<&String, &String> = config.env.iter().collect();
    let root = config
        .project_root
        .as_deref()
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_default();
    // serde_json objects serialize with sorted keys, so the encoding is stable.
    let definition = json!({
        "v": 1,
        "projectRoot": root,
        "name": config.name,
        "transport": config.transport,
        "command": config.command,
        "args": config.args,
        "env": env,
        "raw": raw,
    });
    let digest = Sha256::digest(definition.to_string().as_bytes());
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Whether the user approved `fingerprint` on this computer. Without an app
/// (tests, early startup) nothing is approved.
pub fn is_approved<R: Runtime>(app: Option<&AppHandle<R>>, fingerprint: &str) -> bool {
    app.and_then(|app| app.try_state::<DatabaseStore>())
        .is_some_and(|db| db.get_setting(&setting_key(fingerprint)).is_some())
}

/// Record the user's approval of `config`, whose fingerprint is `fingerprint`.
pub fn record_approval<R: Runtime>(
    app: &AppHandle<R>,
    fingerprint: &str,
    config: &McpServerConfig,
) -> Result<(), String> {
    let db = app
        .try_state::<DatabaseStore>()
        .ok_or("Local settings are unavailable")?;
    let value = json!({
        "projectRoot": config.project_root,
        "name": config.name,
    });
    db.set_setting(&setting_key(fingerprint), &value.to_string())
}

/// One line describing what approving starts: the full command line or URL,
/// plus the environment variables an HTTP server's headers would send.
pub fn summary(config: &McpServerConfig) -> String {
    let mut line = match config.transport {
        McpTransport::Stdio => std::iter::once(config.command.as_str())
            .chain(config.args.iter().map(String::as_str))
            .map(|part| {
                if !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+%".contains(c))
                {
                    part.to_string()
                } else {
                    crate::ssh::exec::sh_quote(part)
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        McpTransport::Http => config.command.clone(),
    };
    let sent = header_env_names(&config.raw);
    if !sent.is_empty() {
        line.push_str(" · sends ");
        line.push_str(&sent.join(", "));
    }
    line
}

/// Environment variables whose values the HTTP client puts in headers.
fn header_env_names(raw: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for key in ["headers", "http_headers"] {
        for value in raw.get(key).and_then(Value::as_object).into_iter().flat_map(|m| m.values()) {
            let mut rest = value.as_str().unwrap_or_default();
            while let Some(start) = rest.find("{env:") {
                let Some(end) = rest[start + 5..].find('}') else { break };
                names.push(rest[start + 5..start + 5 + end].to_string());
                rest = &rest[start + 5 + end..];
            }
        }
    }
    names.extend(
        raw.get("env_http_headers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(Value::as_str)
            .map(str::to_string),
    );
    names.extend(
        raw.get("bearer_token_env_var")
            .and_then(Value::as_str)
            .map(str::to_string),
    );
    names.sort();
    names.dedup();
    names
}

fn setting_key(fingerprint: &str) -> String {
    format!("{SETTING_PREFIX}{fingerprint}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpConfigSource;
    use std::collections::HashMap;

    fn project(source: McpConfigSource) -> McpServerConfig {
        McpServerConfig {
            id: "id".into(),
            name: "lint".into(),
            sources: vec![source],
            command: "sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            env: HashMap::new(),
            disabled: false,
            transport: McpTransport::Stdio,
            raw: json!({"command": "sh", "args": ["-c", "echo hi"]}),
            project_root: Some("/repo".into()),
        }
    }

    #[test]
    fn only_project_sources_need_approval() {
        for source in [
            McpConfigSource::CodemuxProject,
            McpConfigSource::ClaudeProject,
            McpConfigSource::CursorProject,
            McpConfigSource::OpenCodeProject,
        ] {
            assert!(fingerprint(&project(source)).is_some(), "{source:?}");
        }
        for source in [
            McpConfigSource::CodemuxUser,
            McpConfigSource::ClaudeUser,
            McpConfigSource::ClaudeLocal,
            McpConfigSource::CursorUser,
            McpConfigSource::CodexUser,
            McpConfigSource::OpenCodeUser,
        ] {
            assert!(fingerprint(&project(source)).is_none(), "{source:?}");
        }
    }

    /// The row starts its primary source's definition, so a user entry
    /// merged into a project row does not make the project's copy trusted.
    #[test]
    fn merged_rows_follow_the_definition_they_start() {
        let mut project_first = project(McpConfigSource::CodemuxProject);
        project_first.sources.push(McpConfigSource::ClaudeUser);
        assert!(fingerprint(&project_first).is_some());
        let mut user_first = project(McpConfigSource::CodemuxUser);
        user_first.sources.push(McpConfigSource::ClaudeProject);
        assert!(fingerprint(&user_first).is_none());
    }

    #[test]
    fn any_change_to_the_definition_or_project_changes_the_fingerprint() {
        let base = project(McpConfigSource::ClaudeProject);
        let original = fingerprint(&base).unwrap();
        let mut runtime = base.clone();
        runtime.raw["runtimeGeneration"] = json!("generation");
        assert_eq!(fingerprint(&runtime).unwrap(), original);

        let changes: [fn(&mut McpServerConfig); 6] = [
            |c| c.args[1] = "echo bye".into(),
            |c| c.command = "bash".into(),
            |c| drop(c.env.insert("LD_PRELOAD".into(), "x".into())),
            |c| c.raw["cwd"] = json!("/tmp"),
            |c| c.raw["headers"] = json!({"X": "{env:TOKEN}"}),
            |c| c.project_root = Some("/other".into()),
        ];
        for change in changes {
            let mut changed = base.clone();
            change(&mut changed);
            assert_ne!(fingerprint(&changed).unwrap(), original);
        }
    }

    #[test]
    fn summary_shows_the_command_and_header_secrets() {
        let stdio = project(McpConfigSource::ClaudeProject);
        assert_eq!(summary(&stdio), "sh -c 'echo hi'");
        let mut http = project(McpConfigSource::ClaudeProject);
        http.transport = McpTransport::Http;
        http.command = "http://example.test/mcp".into();
        http.raw = json!({
            "headers": {"X": "{env:B}-{env:A}"},
            "env_http_headers": {"Y": "C"},
            "bearer_token_env_var": "A",
        });
        assert_eq!(summary(&http), "http://example.test/mcp · sends A, B, C");
    }
}
