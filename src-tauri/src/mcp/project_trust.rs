//! Local consent for MCP servers defined by a project's own files.
//!
//! `<project>/.codemux/mcp.json`, `.mcp.json`, `.cursor/mcp.json` and
//! `opencode.json{,c}` are repository content: whoever wrote the repository
//! chose their commands, URLs and headers. Codemux starts such a server, or
//! sends it environment-derived credentials, only after the user approves
//! that exact definition for that project on this computer.
//!
//! Approvals live in their own local database table, never in the
//! repository or the settings paired devices can read. Each one
//! is keyed by a hash of the canonical project root and the server's full
//! definition, so a change to the command, arguments, environment, working
//! directory, URL or headers asks again. The project root of a registered
//! Git worktree is its main checkout, so every worktree of a project shares
//! the approvals of identical definitions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};

use super::http_client::{header_template_parts, HeaderTemplatePart};
use super::{is_project_source, McpServerConfig, McpTransport};
use crate::database::DatabaseStore;

/// Keys the registry adds to `raw` at runtime. They are not part of what
/// the project defines.
const RUNTIME_KEYS: [&str; 2] = ["runtimeGeneration", "pluginRegistrationToken"];

/// The root approvals for servers discovered in `checkout` are keyed by:
/// the canonical Git root, when `checkout` lies inside it or Git lists
/// `checkout` as one of that repository's worktrees, else `checkout` itself.
/// A `.git` file can point anywhere, so a folder only shares a project's
/// approvals when Git itself registered it there.
pub fn approval_root(checkout: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(checkout).unwrap_or_else(|_| checkout.to_path_buf());
    let Some(root) = crate::config::workspace_config::find_git_root(&canonical)
        .and_then(|root| std::fs::canonicalize(root).ok())
    else {
        return canonical;
    };
    let registered = || {
        crate::git::git_list_worktrees(&root).is_ok_and(|worktrees| {
            worktrees
                .iter()
                .filter(|worktree| !worktree.is_bare)
                .any(|worktree| {
                    std::fs::canonicalize(&worktree.path).ok().as_deref()
                        == Some(canonical.as_path())
                })
        })
    };
    if canonical.starts_with(&root) || registered() {
        root
    } else {
        canonical
    }
}

/// The approval fingerprint for a server that needs one, or `None` when the
/// definition Codemux would start comes from a user-owned file.
///
/// Dedupe merges only entries whose name, command, arguments, environment,
/// working directory, transport and headers all match, and makes the
/// user-owned entry canonical, so a row that any user-owned file also
/// defines starts exactly what the user wrote there.
pub fn fingerprint(config: &McpServerConfig) -> Option<String> {
    if config.sources.is_empty()
        || !config
            .sources
            .iter()
            .all(|source| is_project_source(*source))
    {
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
        .is_some_and(|db| db.mcp_project_server_approved(fingerprint))
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
    let root = config
        .project_root
        .as_deref()
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_default();
    db.approve_mcp_project_server(fingerprint, &root, &config.name)
}

/// Forget the approval of `fingerprint`. Returns whether there was one.
pub fn revoke_approval<R: Runtime>(app: &AppHandle<R>, fingerprint: &str) -> Result<bool, String> {
    app.try_state::<DatabaseStore>()
        .ok_or("Local settings are unavailable")?
        .revoke_mcp_project_approval(fingerprint)
}

/// What approving starts, in full: the command line with its environment
/// and working directory, or the URL with the environment variables its
/// headers would send. Shown only on this computer, never to paired devices.
/// Every part goes through [`escape`], so the repository cannot hide part
/// of it from the person reading it.
pub fn summary(config: &McpServerConfig) -> String {
    let mut parts = Vec::new();
    match config.transport {
        McpTransport::Stdio => {
            let line = std::iter::once(config.command.as_str())
                .chain(config.args.iter().map(String::as_str));
            parts.push(line.map(quote).collect::<Vec<_>>().join(" "));
            let env: BTreeMap<&String, &String> = config.env.iter().collect();
            if !env.is_empty() {
                let env = env
                    .iter()
                    .map(|(name, value)| format!("{}={}", quote(name), quote(value)));
                parts.push(format!("env {}", env.collect::<Vec<_>>().join(" ")));
            }
            if let Some(cwd) = config.raw.get("cwd").and_then(Value::as_str) {
                parts.push(format!("cwd {}", quote(cwd)));
            }
        }
        McpTransport::Http => parts.push(escape(&config.command)),
    }
    let sent = header_env_names(&config.raw);
    if !sent.is_empty() {
        let sent: Vec<String> = sent.iter().map(|name| escape(name)).collect();
        parts.push(format!("sends {}", sent.join(", ")));
    }
    parts.join(" · ")
}

/// `text` with every character that could rewrite or hide what a terminal
/// or the Settings page shows (control, bidi, zero-width and other
/// invisible formatting characters) written out as `\u{..}`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let invisible = matches!(c,
            '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}'
            | '\u{17B5}' | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E0FFF}');
        if c.is_control() || invisible {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// `part` as a word of the host's shell, quoted only when it needs to be,
/// then [`escape`]d.
fn quote(part: &str) -> String {
    // Windows paths read best unquoted, and `\` is not an escape there.
    let plain_extra = if cfg!(windows) {
        "-_./=:@,+%\\"
    } else {
        "-_./=:@,+%"
    };
    let plain = !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || plain_extra.contains(c));
    let quoted = if plain {
        part.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", part.replace('"', "\\\""))
    } else {
        format!("'{}'", part.replace('\'', r"'\''"))
    };
    escape(&quoted)
}

/// Environment variables whose values the HTTP client puts in headers.
fn header_env_names(raw: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for key in ["headers", "http_headers"] {
        for value in raw
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.values())
        {
            // The same tokenizer the HTTP client expands headers with.
            names.extend(
                header_template_parts(value.as_str().unwrap_or_default())
                    .into_iter()
                    .filter_map(|part| match part {
                        HeaderTemplatePart::Env(name) => Some(name.to_string()),
                        HeaderTemplatePart::Text(_) => None,
                    }),
            );
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

    /// A repository copying a server from the user's own files runs only
    /// what the user wrote: the user's entry is canonical even when the
    /// project file ranks first, so the copy cannot re-enable it, take
    /// over its id (Settings' off switch) or change its other fields.
    #[test]
    fn a_project_copy_of_a_user_server_keeps_the_users_entry() {
        for user_source in [
            McpConfigSource::ClaudeUser,
            McpConfigSource::CursorUser,
            McpConfigSource::CodexUser,
            McpConfigSource::OpenCodeUser,
        ] {
            let mut repo = project(McpConfigSource::CodemuxProject);
            repo.id = "repo".into();
            repo.raw["description"] = json!("from the repository");
            repo.project_root = Some("/repo".into());
            let mut user = project(user_source);
            user.id = "user".into();
            user.disabled = true;
            user.project_root = None;
            let rows = crate::mcp::dedupe_servers(vec![repo, user]);
            assert_eq!(rows.len(), 1, "{user_source:?}");
            let row = &rows[0];
            assert_eq!(row.id, "user");
            assert!(row.disabled);
            assert!(row.raw.get("description").is_none());
            assert_eq!(row.project_root, None);
            assert_eq!(
                row.sources,
                vec![user_source, McpConfigSource::CodemuxProject]
            );
            assert!(fingerprint(row).is_none());
        }
        let rows = crate::mcp::dedupe_servers(vec![
            project(McpConfigSource::CodemuxProject),
            project(McpConfigSource::ClaudeProject),
        ]);
        assert_eq!(rows.len(), 1);
        assert!(fingerprint(&rows[0]).is_some());
    }

    /// Dedupe keeps entries that differ only in their working directory
    /// apart, so a user entry cannot vouch for a project copy run elsewhere.
    #[test]
    fn dedupe_keeps_a_different_working_directory_apart() {
        let user = project(McpConfigSource::ClaudeUser);
        let mut repo = project(McpConfigSource::ClaudeProject);
        repo.raw["cwd"] = json!("./tools");
        let rows = crate::mcp::dedupe_servers(vec![user.clone(), repo]);
        assert_eq!(rows.len(), 2);
        assert!(fingerprint(&rows[1]).is_some());
        let rows = crate::mcp::dedupe_servers(vec![user, project(McpConfigSource::ClaudeProject)]);
        assert_eq!(rows.len(), 1);
        assert!(fingerprint(&rows[0]).is_none());
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
    fn summary_shows_the_whole_definition() {
        let q = |word: &str| {
            if cfg!(windows) {
                format!("\"{word}\"")
            } else {
                format!("'{word}'")
            }
        };
        let mut stdio = project(McpConfigSource::ClaudeProject);
        assert_eq!(summary(&stdio), format!("sh -c {}", q("echo hi")));
        stdio
            .env
            .insert("NODE_OPTIONS".into(), "--require ./x.js".into());
        stdio.env.insert("A".into(), "1".into());
        stdio.raw["cwd"] = json!("./tools dir");
        assert_eq!(
            summary(&stdio),
            format!(
                "sh -c {} · env A=1 NODE_OPTIONS={} · cwd {}",
                q("echo hi"),
                q("--require ./x.js"),
                q("./tools dir")
            )
        );
        stdio.args = vec!["it's".into()];
        let expected = if cfg!(windows) {
            r#""it's""#
        } else {
            r"'it'\''s'"
        };
        assert_eq!(
            summary(&stdio).split(" · ").next().unwrap(),
            format!("sh {expected}")
        );
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

    /// Terminal escapes, carriage returns and bidi or zero-width characters
    /// from the repository are shown, not interpreted.
    #[test]
    fn summary_cannot_hide_part_of_the_definition() {
        let mut stdio = project(McpConfigSource::ClaudeProject);
        stdio.command = "node".into();
        stdio.args = vec![
            "\u{1b}[2K\r  npx -y @safe/server".into(),
            "evil\u{202e}txt.js".into(),
            "a\u{200b}b".into(),
        ];
        let shown = summary(&stdio);
        assert!(
            !shown
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{202e}' | '\u{200b}')),
            "{shown:?}"
        );
        assert!(shown.contains("\\u{1b}[2K\\u{d}  npx"), "{shown}");
        assert!(shown.contains("evil\\u{202e}txt.js"), "{shown}");
        assert!(shown.contains("a\\u{200b}b"), "{shown}");
        assert_eq!(escape("plain C:\\dir é"), "plain C:\\dir é");
    }
}
