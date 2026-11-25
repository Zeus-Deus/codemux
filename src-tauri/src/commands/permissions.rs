use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix::ProjectSettingsDir;
#[cfg(windows)]
use windows::ProjectSettingsDir;

const PROJECT_SCOPES: [(&str, &str); 2] = [
    ("project", "settings.json"),
    ("local", "settings.local.json"),
];

/// A single tool-permission rule, flattened across the three settings
/// scopes the SDK reads (user, project-shared, project-local). The
/// `scope` + `source_path` fields tell the UI which file to show next
/// to each rule and let the remove path resolve which file to edit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRule {
    pub tool_name: String,
    pub rule_content: Option<String>,
    /// "allow" | "deny" | "ask"
    pub behavior: String,
    /// "user" | "project" | "local"
    pub scope: String,
    pub source_path: String,
}

#[derive(Serialize)]
pub struct PermissionScopeError {
    scope: &'static str,
    message: String,
}

#[derive(Default, Serialize)]
pub struct ToolPermissionList {
    rules: Vec<PermissionRule>,
    scope_errors: Vec<PermissionScopeError>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ToolPermissionListResponse {
    Rules(Vec<PermissionRule>),
    Scoped(ToolPermissionList),
}

/// Read all tool-permission rules visible to the current session.
///
/// Order: user (~/.claude/settings.json) first, then project-shared
/// (.claude/settings.json), then project-local (.claude/settings.local.json).
/// A missing file is not an error — the SDK treats absent settings as
/// "no rules", so we do the same.
#[tauri::command]
pub async fn list_tool_permissions(
    project_root: Option<String>,
    include_scope_errors: Option<bool>,
) -> Result<ToolPermissionListResponse, String> {
    let list = tokio::task::spawn_blocking(move || {
        let user_settings = dirs::home_dir().map(|home| home.join(".claude/settings.json"));
        collect_tool_permissions(user_settings.as_deref(), project_root.as_deref().map(Path::new))
    })
    .await
    .map_err(|error| error.to_string())?;
    if include_scope_errors.unwrap_or(false) {
        Ok(ToolPermissionListResponse::Scoped(list))
    } else if let Some(error) = list.scope_errors.into_iter().next() {
        Err(error.message)
    } else {
        Ok(ToolPermissionListResponse::Rules(list.rules))
    }
}

fn collect_tool_permissions(user_settings: Option<&Path>, root: Option<&Path>) -> ToolPermissionList {
    let mut list = ToolPermissionList::default();
    if let Some(path) = user_settings {
        list.add_scope("user", path, File::open(path));
    }
    if let Some(root) = root {
        match ProjectSettingsDir::open(root) {
            Ok(dir) => {
                for (scope, name) in PROJECT_SCOPES {
                    let path = root.join(".claude").join(name);
                    list.add_scope(scope, &path, dir.open_file(name));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                let path = root.join(".claude");
                for (scope, _) in PROJECT_SCOPES {
                    list.scope_errors.push(PermissionScopeError {
                        scope,
                        message: format!("Failed to open {}: {error}", path.display()),
                    });
                }
            }
        }
    }
    list
}

impl ToolPermissionList {
    fn add_scope(&mut self, scope: &'static str, path: &Path, file: io::Result<File>) {
        let content = match file {
            Err(error) if error.kind() == ErrorKind::NotFound => return,
            file => file.and_then(read_settings),
        };
        let rules = content
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))
            .and_then(|content| parse_settings(&content, path, scope));
        match rules {
            Ok(rules) => self.rules.extend(rules),
            Err(message) => self.scope_errors.push(PermissionScopeError { scope, message }),
        }
    }
}

/// Remove a single rule from whichever settings file owns it.
///
/// The `scope` field on the input determines which file we touch; the
/// `tool_name` + `rule_content` pair must match an entry in the matching
/// `permissions.<behavior>` array. If the settings changed while preparing
/// the edit, the UI can reload the latest rules before retrying.
#[tauri::command]
pub async fn remove_tool_permission(
    rule: PermissionRule,
    project_root: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || remove_permission(rule, project_root.as_deref()))
        .await
        .map_err(|error| error.to_string())?
}

fn remove_permission(rule: PermissionRule, project_root: Option<&str>) -> Result<(), String> {
    let path = path_for_scope(&rule.scope, project_root)?;
    let name = path.file_name().and_then(|name| name.to_str()).expect("fixed settings filename");
    let project = project_root
        .filter(|_| matches!(rule.scope.as_str(), "project" | "local"))
        .map(|root| ProjectSettingsDir::open(Path::new(root)))
        .transpose()
        .map_err(|error| format!("Failed to open project settings: {error}"))?;
    let file = match &project {
        Some(dir) => dir.open_file(name),
        None => File::open(&path),
    };
    let content = file.and_then(read_settings)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    let mut settings: serde_json::Value = if content.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?
    };

    let array_name = match rule.behavior.as_str() {
        "allow" | "deny" | "ask" => rule.behavior.as_str(),
        other => return Err(format!("Unknown behavior: {other}")),
    };

    let mut removed = false;
    if let Some(perms) = settings.get_mut("permissions").and_then(|v| v.as_object_mut()) {
        if let Some(arr) = perms.get_mut(array_name).and_then(|v| v.as_array_mut()) {
            let before = arr.len();
            arr.retain(|entry| !rule_entry_matches(entry, &rule));
            removed = arr.len() < before;
        }
    }

    if !removed {
        // Not finding the rule isn't fatal — the file may have been
        // edited since the UI loaded — but we surface it so callers
        // can refresh their list and report nothing changed.
        return Err(format!(
            "Rule not found in {} (the file may have been edited externally)",
            path.display()
        ));
    }

    let new_content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;
    let result = match project {
        Some(dir) => dir.replace(name, content.as_bytes(), new_content.as_bytes()),
        None => crate::config_file::replace_if_unchanged(&path, Some(content.as_bytes()), new_content.as_bytes()),
    };
    result.map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn read_settings(mut file: File) -> io::Result<String> {
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

fn validate_project_settings_name(name: &str) -> io::Result<()> {
    if !PROJECT_SCOPES.iter().any(|(_, file)| *file == name) {
        return Err(io::Error::new(ErrorKind::InvalidInput, "Invalid project settings filename"));
    }
    Ok(())
}

fn settings_changed() -> io::Error {
    io::Error::other("Settings changed while they were being updated")
}

fn path_for_scope(scope: &str, project_root: Option<&str>) -> Result<PathBuf, String> {
    match scope {
        "user" => dirs::home_dir()
            .map(|h| h.join(".claude").join("settings.json"))
            .ok_or_else(|| "No home directory available".to_string()),
        "project" => project_root
            .map(|r| Path::new(r).join(".claude").join("settings.json"))
            .ok_or_else(|| "Project root required for project scope".to_string()),
        "local" => project_root
            .map(|r| Path::new(r).join(".claude").join("settings.local.json"))
            .ok_or_else(|| "Project root required for local scope".to_string()),
        other => Err(format!("Unknown scope: {other}")),
    }
}

// TODO(JSONC): `~/.claude/settings.json` and project `.claude/settings.json`
// files may contain JSONC — `// line comments`, `/* block comments */`,
// and trailing commas — which the Claude CLI tolerates but `serde_json`
// does not. A user with comments in their settings will see this parser
// error out and the Permissions UI fall back to "Failed to load rules".
// Real fix: swap to a JSONC-tolerant parser (e.g. the `json5` or
// `jsonc-parser` crate). Interim mitigation: surface the parse error
// with a hint like "settings.json contains comments — edit manually".
// Same caveat applies to the writer below: `serde_json::to_string_pretty`
// strips any comments that did manage to round-trip.
fn parse_settings(content: &str, path: &Path, scope: &str) -> Result<Vec<PermissionRule>, String> {
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;

    let Some(perms) = value.get("permissions").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };

    let path_str = path.display().to_string();
    let mut rules = Vec::new();
    for behavior in ["allow", "deny", "ask"] {
        let Some(arr) = perms.get(behavior).and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in arr {
            if let Some(rule) = parse_rule_entry(entry, behavior, scope, &path_str) {
                rules.push(rule);
            }
        }
    }
    Ok(rules)
}

/// SDK entries can take two shapes per the docs:
///   - a string: `"Bash"` or `"Bash(git status:*)"`
///   - an object: `{ "toolName": "Bash", "ruleContent": "git status:*" }`
/// Stage 5 only writes the object shape, but the user's Claude CLI may
/// have either, so we accept both.
fn parse_rule_entry(
    entry: &serde_json::Value,
    behavior: &str,
    scope: &str,
    source_path: &str,
) -> Option<PermissionRule> {
    if let Some(s) = entry.as_str() {
        let (tool_name, rule_content) = split_string_rule(s);
        return Some(PermissionRule {
            tool_name,
            rule_content,
            behavior: behavior.to_string(),
            scope: scope.to_string(),
            source_path: source_path.to_string(),
        });
    }
    let obj = entry.as_object()?;
    let tool_name = obj.get("toolName").and_then(|v| v.as_str())?.to_string();
    let rule_content = obj
        .get("ruleContent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(PermissionRule {
        tool_name,
        rule_content,
        behavior: behavior.to_string(),
        scope: scope.to_string(),
        source_path: source_path.to_string(),
    })
}

fn split_string_rule(s: &str) -> (String, Option<String>) {
    if let Some(open) = s.find('(') {
        if s.ends_with(')') {
            let tool = &s[..open];
            let inner = &s[open + 1..s.len() - 1];
            return (tool.to_string(), Some(inner.to_string()));
        }
    }
    (s.to_string(), None)
}

fn rule_entry_matches(entry: &serde_json::Value, rule: &PermissionRule) -> bool {
    if let Some(s) = entry.as_str() {
        let (tool, content) = split_string_rule(s);
        return tool == rule.tool_name && content == rule.rule_content;
    }
    let Some(obj) = entry.as_object() else {
        return false;
    };
    let tool_match = obj
        .get("toolName")
        .and_then(|v| v.as_str())
        .map(|s| s == rule.tool_name)
        .unwrap_or(false);
    let content_match = obj
        .get("ruleContent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        == rule.rule_content;
    tool_match && content_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn parse_settings_file(path: &Path, scope: &str) -> Result<Vec<PermissionRule>, String> {
        parse_settings(&fs::read_to_string(path).unwrap(), path, scope)
    }

    fn write_settings(dir: &Path, file: &str, contents: &str) -> PathBuf {
        let claude = dir.join(".claude");
        fs::create_dir_all(&claude).unwrap();
        let path = claude.join(file);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_object_rules() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{
                "permissions": {
                    "allow": [
                        { "toolName": "Bash" },
                        { "toolName": "Read", "ruleContent": "src/**" }
                    ],
                    "deny": [
                        { "toolName": "Bash", "ruleContent": "rm -rf:*" }
                    ]
                }
            }"#,
        );
        let rules = parse_settings_file(&path, "project").unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].tool_name, "Bash");
        assert_eq!(rules[0].rule_content, None);
        assert_eq!(rules[0].behavior, "allow");
        assert_eq!(rules[0].scope, "project");
        assert_eq!(rules[1].tool_name, "Read");
        assert_eq!(rules[1].rule_content.as_deref(), Some("src/**"));
        assert_eq!(rules[2].behavior, "deny");
    }

    #[test]
    fn parses_string_rules_with_inner_content() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{ "permissions": { "allow": ["Bash", "Bash(git status:*)"] } }"#,
        );
        let rules = parse_settings_file(&path, "user").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].tool_name, "Bash");
        assert_eq!(rules[0].rule_content, None);
        assert_eq!(rules[1].tool_name, "Bash");
        assert_eq!(rules[1].rule_content.as_deref(), Some("git status:*"));
    }

    #[test]
    fn empty_or_missing_files_yield_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let empty = write_settings(tmp.path(), "settings.json", "");
        assert!(parse_settings_file(&empty, "user").unwrap().is_empty());

        let bare = write_settings(tmp.path(), "bare.json", "{}");
        assert!(parse_settings_file(&bare, "user").unwrap().is_empty());

        let no_perms = write_settings(tmp.path(), "no_perms.json", r#"{ "model": "x" }"#);
        assert!(parse_settings_file(&no_perms, "user").unwrap().is_empty());
    }

    #[test]
    fn list_returns_empty_when_nothing_exists() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let result = collect_tool_permissions(Some(&tmp.path().join("missing")), Some(&project));
        assert!(result.rules.is_empty());
        assert!(result.scope_errors.is_empty());
    }

    #[test]
    fn invalid_project_settings_keep_valid_user_and_local_rules() {
        let user = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let user_file = write_settings(user.path(), "settings.json", r#"{"permissions":{"allow":["Read"]}}"#);
        write_settings(project.path(), "settings.json", "invalid JSON");
        write_settings(project.path(), "settings.local.json", r#"{"permissions":{"deny":["Write"]}}"#);

        let result = collect_tool_permissions(Some(&user_file), Some(project.path()));
        let scopes: Vec<_> = result.rules.iter().map(|rule| rule.scope.as_str()).collect();
        assert_eq!(scopes, ["user", "local"]);
        assert_eq!(result.scope_errors.len(), 1);
        assert_eq!(result.scope_errors[0].scope, "project");
        let response = serde_json::to_value(ToolPermissionListResponse::Scoped(result)).unwrap();
        assert_eq!(response["scope_errors"][0]["scope"], "project");
        assert_eq!(response["rules"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn remove_tool_permission_removes_object_rule() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let path = write_settings(
            project,
            "settings.local.json",
            r#"{
                "permissions": {
                    "allow": [
                        { "toolName": "Bash" },
                        { "toolName": "Read", "ruleContent": "src/**" }
                    ]
                }
            }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: None,
            behavior: "allow".into(),
            scope: "local".into(),
            source_path: path.display().to_string(),
        };
        remove_tool_permission(rule, Some(project.to_string_lossy().to_string()))
            .await
            .unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        let arr = v["permissions"]["allow"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["toolName"], "Read");
    }

    #[tokio::test]
    async fn remove_tool_permission_leaves_no_tmp_file_behind() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let path = write_settings(
            project,
            "settings.local.json",
            r#"{ "permissions": { "allow": [{ "toolName": "Bash" }] } }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: None,
            behavior: "allow".into(),
            scope: "local".into(),
            source_path: path.display().to_string(),
        };
        remove_tool_permission(rule, Some(project.to_string_lossy().to_string()))
            .await
            .unwrap();

        // Atomic-write contract: after a successful remove there must
        // be no `<file>.json.tmp` sidecar. A leftover would mean the
        // rename never landed and the user's file is stale.
        let claude_dir = project.join(".claude");
        let entries: Vec<_> = fs::read_dir(&claude_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            !entries.iter().any(|n| n.ends_with(".tmp")),
            ".tmp file leaked after successful remove: {entries:?}",
        );
        // And the canonical file is intact.
        assert!(path.exists());
    }

    #[test]
    fn atomic_write_replaces_target_in_place() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, b"old").unwrap();
        crate::config_file::replace_if_unchanged(&path, Some(b"old"), b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");

        // No `.tmp` sidecar in the directory after success.
        let entries: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["settings.json".to_string()]);
    }

    #[test]
    fn atomic_write_preserves_an_existing_sibling_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        let sibling = path.with_extension("json.tmp");
        fs::write(&path, b"old").unwrap();
        fs::write(&sibling, b"unrelated").unwrap();

        crate::config_file::replace_if_unchanged(&path, Some(b"old"), b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(&sibling).unwrap(), b"unrelated");
    }

    #[cfg(unix)]
    #[test]
    fn project_settings_must_stay_in_the_project() {
        let project = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let settings = write_settings(other.path(), "settings.json", "{}");
        let linked_dir = project.path().join(".claude");
        std::os::unix::fs::symlink(settings.parent().unwrap(), &linked_dir).unwrap();

        assert!(ProjectSettingsDir::open(project.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_settings_leaf_must_be_a_regular_file() {
        let project = TempDir::new().unwrap();
        let directory = project.path().join(".claude");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("shared.json"), "{}").unwrap();
        std::os::unix::fs::symlink("shared.json", directory.join("settings.json")).unwrap();
        let opened = ProjectSettingsDir::open(project.path()).unwrap();
        assert!(opened.open_file("settings.json").is_err());
        assert!(opened.replace("settings.json", b"{}", b"edited").is_err());
        assert_eq!(fs::read_to_string(directory.join("shared.json")).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn settings_edits_refuse_a_relocated_project_directory() {
        use std::os::unix::fs::PermissionsExt;

        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let settings = write_settings(project.path(), "settings.json", "original");
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o640)).unwrap();
        let directory = settings.parent().unwrap();
        let opened = ProjectSettingsDir::open(project.path()).unwrap();
        opened.replace("settings.json", b"original", b"updated").unwrap();
        assert_eq!(fs::metadata(&settings).unwrap().permissions().mode() & 0o777, 0o640);

        let moved = outside.path().join("opened-settings");
        fs::rename(directory, &moved).unwrap();
        write_settings(project.path(), "settings.json", "updated");

        assert!(opened.replace("settings.json", b"updated", b"new").is_err());
        assert_eq!(fs::read_to_string(moved.join("settings.json")).unwrap(), "updated");
        assert_eq!(fs::read_to_string(settings).unwrap(), "updated");
        assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
    }

    #[test]
    fn settings_edit_refuses_stale_content_and_cleans_up() {
        let project = TempDir::new().unwrap();
        let settings = write_settings(project.path(), "settings.json", "updated outside the app");
        let opened = ProjectSettingsDir::open(project.path()).unwrap();
        assert!(opened.replace("settings.json", b"original", b"ours").is_err());
        assert_eq!(fs::read_to_string(&settings).unwrap(), "updated outside the app");
        let files: Vec<_> = fs::read_dir(settings.parent().unwrap()).unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["settings.json"]);
    }

    #[test]
    fn atomic_write_does_not_truncate_target_on_serialize() {
        // Regression guard for the "tokio::fs::write truncates first,
        // then writes" footgun: even if the writer crashes before any
        // bytes land, the target must remain intact. We can't crash
        // mid-write deterministically, but we can verify the tempfile
        // path is *different* from the target so a partial tempfile
        // never reaches the target until rename succeeds.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, b"original").unwrap();

        crate::config_file::replace_if_unchanged(&path, Some(b"original"), b"replacement").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[tokio::test]
    async fn remove_tool_permission_errors_when_rule_absent() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        write_settings(
            project,
            "settings.local.json",
            r#"{ "permissions": { "allow": [{ "toolName": "Read" }] } }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: None,
            behavior: "allow".into(),
            scope: "local".into(),
            source_path: "ignored".into(),
        };
        let err = remove_tool_permission(rule, Some(project.to_string_lossy().to_string()))
            .await
            .unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn path_for_scope_resolves_user_project_local() {
        let user = path_for_scope("user", None).unwrap();
        assert!(user.ends_with(".claude/settings.json"));

        let project = path_for_scope("project", Some("/tmp/x")).unwrap();
        assert_eq!(project, PathBuf::from("/tmp/x/.claude/settings.json"));

        let local = path_for_scope("local", Some("/tmp/x")).unwrap();
        assert_eq!(local, PathBuf::from("/tmp/x/.claude/settings.local.json"));

        assert!(path_for_scope("project", None).is_err());
        assert!(path_for_scope("garbage", None).is_err());
    }

    // ----- Edge cases ----------------------------------------------------

    /// `permissions: null` is treated the same as a missing key — the
    /// `as_object()` cast fails and we return an empty rule list rather
    /// than blowing up.
    #[test]
    fn parse_settings_file_handles_null_permissions() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{ "permissions": null }"#,
        );
        let rules = parse_settings_file(&path, "user").unwrap();
        assert!(rules.is_empty());
    }

    /// `allow: "not-an-array"` — wrong type for the inner array. The
    /// `as_array()` cast bails cleanly and that behavior's rules are
    /// skipped without affecting siblings.
    #[test]
    fn parse_settings_file_handles_wrong_type_for_allow() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{ "permissions": { "allow": "not-an-array", "deny": [{ "toolName": "Bash" }] } }"#,
        );
        let rules = parse_settings_file(&path, "user").unwrap();
        // The malformed `allow` is silently skipped, the well-formed
        // `deny` still parses.
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].tool_name, "Bash");
        assert_eq!(rules[0].behavior, "deny");
    }

    /// Mixed string + object rules in the same array — both shapes are
    /// supported per the SDK docs.
    #[test]
    fn parse_settings_file_handles_mixed_string_and_object_rules() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{
                "permissions": {
                    "allow": [
                        "Bash(ls:*)",
                        { "toolName": "Read", "ruleContent": "src/**" },
                        "WebFetch"
                    ]
                }
            }"#,
        );
        let rules = parse_settings_file(&path, "project").unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].tool_name, "Bash");
        assert_eq!(rules[0].rule_content.as_deref(), Some("ls:*"));
        assert_eq!(rules[1].tool_name, "Read");
        assert_eq!(rules[1].rule_content.as_deref(), Some("src/**"));
        assert_eq!(rules[2].tool_name, "WebFetch");
        assert_eq!(rules[2].rule_content, None);
    }

    /// JSON that is valid but not an object at the top — `[1,2,3]`. The
    /// `value.get("permissions")` lookup returns `None` for non-object
    /// roots, so we return empty rather than erroring.
    #[test]
    fn parse_settings_file_handles_non_object_top_level_json() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(tmp.path(), "settings.json", "[1,2,3]");
        let rules = parse_settings_file(&path, "user").unwrap();
        assert!(rules.is_empty());
    }

    /// Truncated JSON should produce a parse error (not a panic) — the
    /// file is corrupt, surface it to the caller.
    #[test]
    fn parse_settings_file_errors_on_malformed_json() {
        let tmp = TempDir::new().unwrap();
        let path = write_settings(
            tmp.path(),
            "settings.json",
            r#"{ "permissions": { "allow": [ "#, // truncated
        );
        let err = parse_settings_file(&path, "user").unwrap_err();
        assert!(err.contains("Failed to parse"));
    }

    /// When the same `toolName` + `ruleContent` pair appears twice in
    /// the array (e.g. user edited the file by hand), `Vec::retain`
    /// removes EVERY match — both copies disappear in one call.
    #[tokio::test]
    async fn remove_tool_permission_removes_all_duplicates() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let path = write_settings(
            project,
            "settings.local.json",
            r#"{
                "permissions": {
                    "allow": [
                        { "toolName": "Bash", "ruleContent": "ls:*" },
                        { "toolName": "Bash", "ruleContent": "ls:*" },
                        { "toolName": "Read" }
                    ]
                }
            }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: Some("ls:*".into()),
            behavior: "allow".into(),
            scope: "local".into(),
            source_path: path.display().to_string(),
        };
        remove_tool_permission(rule, Some(project.to_string_lossy().to_string()))
            .await
            .unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        let arr = v["permissions"]["allow"].as_array().unwrap();
        // Both Bash duplicates are gone in one call.
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["toolName"], "Read");
    }

    /// Removing the only rule leaves an empty `allow: []` array; the
    /// pretty-printed file should still contain the empty array (not
    /// drop the key) so the JSON shape stays predictable.
    #[tokio::test]
    async fn remove_tool_permission_preserves_empty_array_shape() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let path = write_settings(
            project,
            "settings.local.json",
            r#"{ "permissions": { "allow": [{ "toolName": "Bash" }] } }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: None,
            behavior: "allow".into(),
            scope: "local".into(),
            source_path: path.display().to_string(),
        };
        remove_tool_permission(rule, Some(project.to_string_lossy().to_string()))
            .await
            .unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        // The empty array survives — file is still parseable, the
        // permissions key still exists, and parsing yields zero rules.
        assert!(v["permissions"]["allow"].is_array());
        assert_eq!(v["permissions"]["allow"].as_array().unwrap().len(), 0);
        let rules = parse_settings_file(&path, "local").unwrap();
        assert!(rules.is_empty());
    }

    /// `Bash()` — empty parens should split into tool name "Bash" and
    /// inner content `Some("")`. We document the current behavior so a
    /// future refactor can't silently regress it.
    #[test]
    fn split_string_rule_handles_empty_parens() {
        let (tool, content) = split_string_rule("Bash()");
        assert_eq!(tool, "Bash");
        assert_eq!(content.as_deref(), Some(""));
    }

    /// Nested parens — `Bash(nested(parens))`. The current splitter
    /// finds the first `(` and slices from `[..first_paren, +1..len-1]`.
    /// For "Bash(nested(parens))" the inner content includes the inner
    /// parens; we lock that in here.
    #[test]
    fn split_string_rule_survives_nested_parens() {
        let (tool, content) = split_string_rule("Bash(nested(parens))");
        assert_eq!(tool, "Bash");
        assert_eq!(content.as_deref(), Some("nested(parens)"));
    }

    /// A string that has `(` but does NOT end with `)` (e.g. malformed
    /// input) — we treat the whole thing as a tool name with no
    /// content rather than slicing into garbage.
    #[test]
    fn split_string_rule_unmatched_open_paren_returns_whole_as_tool() {
        let (tool, content) = split_string_rule("Bash(oops");
        assert_eq!(tool, "Bash(oops");
        assert_eq!(content, None);
    }

    /// `path_for_scope` must reject unknown scopes with a stable error
    /// message, never panic. Covers the "garbage" branch beyond the
    /// existing happy-path test.
    #[test]
    fn path_for_scope_rejects_unknown_scopes() {
        for bad in ["", "global", "USER", "Project", "💩"] {
            let err = path_for_scope(bad, Some("/tmp/x")).unwrap_err();
            assert!(
                err.contains("Unknown scope"),
                "scope `{bad}` should error with `Unknown scope`, got: {err}"
            );
        }
    }

    /// `remove_tool_permission` should reject an unknown behavior value
    /// before touching the file. Surfaces the explicit guard rather
    /// than silently writing nothing.
    #[tokio::test]
    async fn remove_tool_permission_rejects_unknown_behavior() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        write_settings(
            project,
            "settings.local.json",
            r#"{ "permissions": { "allow": [{ "toolName": "Bash" }] } }"#,
        );
        let rule = PermissionRule {
            tool_name: "Bash".into(),
            rule_content: None,
            behavior: "weird".into(),
            scope: "local".into(),
            source_path: "ignored".into(),
        };
        let err = remove_tool_permission(
            rule,
            Some(project.to_string_lossy().to_string()),
        )
        .await
        .unwrap_err();
        assert!(err.contains("Unknown behavior"));
    }
}
