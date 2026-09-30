//! Opt-in read-only local transcript import. No provider runtime is started.
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct ParsedSession {
    pub source_id: String,
    pub provider: String,
    pub cwd: String,
    pub last_active_at: String,
    pub messages: Vec<(String, String)>,
}
impl ParsedSession {
    pub fn thread_id(&self) -> String {
        format!("local-import-{}", self.source_id)
    }
    pub fn title(&self) -> String {
        self.messages
            .iter()
            .find(|(role, _)| role == "user")
            .map(|(_, text)| {
                text.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(100)
                    .collect()
            })
            .unwrap_or_else(|| "Imported conversation".into())
    }
}
fn visible_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn codex_text(content: &Value, role: &str) -> String {
    let kind = if role == "user" {
        "input_text"
    } else {
        "output_text"
    };
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == kind)
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn parse_rows(provider: &str, rows: &[Value]) -> Option<ParsedSession> {
    if !matches!(provider, "claude" | "codex") {
        return None;
    }
    let rows: Vec<&Value> = rows
        .iter()
        .filter(|r| {
            r["isSidechain"] != true
                && r["isMeta"] != true
                && r["isCompactSummary"] != true
                && r["agentId"].is_null()
                && r["subagent_id"].is_null()
        })
        .collect();
    let meta = rows
        .iter()
        .find(|r| r["type"] == "session_meta")
        .map(|r| &r["payload"]);
    let id = if provider == "codex" {
        meta?["id"].as_str()?
    } else {
        rows.iter().find_map(|r| r["sessionId"].as_str())?
    };
    let native_id = Uuid::parse_str(id).ok()?;
    if let Some(meta) = meta {
        if (!meta["parent_thread_id"].is_null() && meta["parent_thread_id"] != "")
            || meta["source"].is_object()
            || meta["source"]
                .as_str()
                .is_some_and(|s| s.contains("subagent"))
        {
            return None;
        }
    }
    let cwd = if provider == "codex" {
        meta?["cwd"].as_str()?
    } else {
        rows.iter().find_map(|r| r["cwd"].as_str())?
    }
    .to_string();
    if !std::path::Path::new(&cwd).is_absolute()
        || cwd.len() > 4096
        || cwd.chars().any(char::is_control)
    {
        return None;
    }
    let last_active_at = rows
        .iter()
        .filter_map(|r| r["timestamp"].as_str())
        .filter_map(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .max()?
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut messages = Vec::new();
    // Pair only adjacent visible-message runs, bounded in raw rows, and never
    // across a known turn boundary. Equal prompts elsewhere are real messages,
    // not a file-global budget of event mirrors. Keep the first occurrence so
    // either event/canonical ordering preserves transcript chronology.
    let mut mirrors = Vec::<(bool, usize)>::new();
    let mut turn_id: Option<&str> = None;
    for (row_index, row) in rows.into_iter().enumerate() {
        if provider == "codex" {
            let p = &row["payload"];
            let next_turn = p["turn_id"].as_str();
            if row["type"] == "turn_context"
                || (row["type"] == "event_msg"
                    && matches!(p["type"].as_str(), Some("task_started" | "task_complete")))
                || next_turn.is_some_and(|id| Some(id) != turn_id)
            {
                mirrors.clear();
            }
            if next_turn.is_some() {
                turn_id = next_turn;
            }
        }
        let (role, text) = if provider == "codex" {
            let p = &row["payload"];
            if row["type"] == "response_item"
                && p["type"] == "message"
                && p["channel"] != "analysis"
                && p["phase"] != "analysis"
            {
                let role = p["role"].as_str().unwrap_or("");
                (role, codex_text(&p["content"], role))
            } else if row["type"] == "event_msg" {
                let role = match p["type"].as_str() {
                    Some("user_message") => "user",
                    Some("agent_message" | "assistant_message") => "assistant",
                    _ => continue,
                };
                let text = p["message"].as_str().unwrap_or("").to_string();
                (role, text)
            } else {
                continue;
            }
        } else {
            if row["isSidechain"] == true
                || row["isMeta"] == true
                || row["isCompactSummary"] == true
            {
                continue;
            }
            if !matches!(row["type"].as_str(), Some("user" | "assistant")) {
                continue;
            }
            let role = row["message"]["role"].as_str().unwrap_or("");
            (role, visible_text(&row["message"]["content"]))
        };
        if role == "user"
            && [
                "<environment_context>",
                "# AGENTS.md instructions for",
                "<permissions instructions>",
                "<local-command-caveat>",
                "<system-reminder>",
            ]
            .iter()
            .any(|prefix| text.trim_start().starts_with(prefix))
        {
            continue;
        }
        if matches!(role, "user" | "assistant") && !text.trim().is_empty() {
            if provider == "codex" {
                let event = row["type"] == "event_msg";
                let same_message = messages
                    .last()
                    .is_some_and(|(previous_role, previous_text)| {
                        previous_role == role && previous_text == &text
                    });
                if !same_message
                    || mirrors
                        .last()
                        .is_some_and(|(_, index)| row_index - index > 32)
                {
                    mirrors.clear();
                }
                if mirrors
                    .last()
                    .is_some_and(|(previous_event, _)| *previous_event != event)
                {
                    mirrors.pop();
                    continue;
                }
                mirrors.push((event, row_index));
            }
            messages.push((role.into(), text));
        }
    }
    if messages.is_empty() {
        return None;
    }
    Some(ParsedSession {
        source_id: Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("codemux-local:{provider}:{native_id}").as_bytes(),
        )
        .to_string(),
        provider: provider.into(),
        cwd,
        last_active_at,
        messages,
    })
}
pub(crate) fn require_live_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    thread: &str,
) -> Result<(), String> {
    use tauri::Manager;
    require_live_thread(thread)?;
    if let Some(db) = app.try_state::<crate::database::DatabaseStore>() {
        if db.local_import_provider(thread)?.is_some() {
            return Err("imported_snapshot_read_only: Imported conversations are read-only copies; start a new chat to continue".into());
        }
    }
    Ok(())
}
pub fn require_live_thread(thread: &str) -> Result<(), String> {
    if thread.starts_with("local-import-") {
        Err("imported_snapshot_read_only: Imported conversations are read-only copies; start a new chat to continue".into())
    } else {
        Ok(())
    }
}
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ImportResponse {
    pub imported: Vec<ImportedSession>,
    pub skipped: usize,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedSession {
    pub source_id: String,
    pub thread_id: String,
    pub workspace_id: String,
}
#[derive(Default)]
struct ScanData {
    sessions: Vec<ParsedSession>,
    warnings: Vec<String>,
}
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 256 * 1024;
const MAX_FILES: usize = 2000;
const MAX_ENTRIES: usize = 10000;
const MAX_ROWS: usize = 20000;

// Open each component relative to an already-open directory, refusing symlinks
// and non-regular final files. O_NONBLOCK also prevents malicious FIFO hangs.
#[cfg(unix)]
fn secure_open(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut file = std::fs::File::open("/")?;
    let components: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        let name = std::ffi::CString::new(name.as_bytes())?;
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        if index + 1 < components.len() {
            flags |= libc::O_DIRECTORY;
        }
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        file = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(file)
}
#[cfg(windows)]
fn secure_open(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut part = std::path::PathBuf::new();
    let mut pinned = Vec::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        part.push(component.as_os_str());
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        // Pin each ancestor without FILE_SHARE_DELETE, preventing its path from
        // being replaced while opening the next component. Inspect reparse
        // points themselves (symlinks AND junctions), never their targets.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x00200000 | 0x02000000)
            .open(&part)?;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        pinned.push(file);
    }
    pinned
        .pop()
        .ok_or_else(|| std::io::ErrorKind::InvalidInput.into())
}
#[cfg(not(any(unix, windows)))]
fn secure_open(_path: &std::path::Path) -> std::io::Result<std::fs::File> {
    Err(std::io::ErrorKind::Unsupported.into())
}
fn read_rows(path: &std::path::Path, budget: u64) -> Result<(Vec<Value>, u64), String> {
    use std::io::{BufRead, Read};
    let file = secure_open(path).map_err(|_| "Skipped unreadable or symlinked session")?;
    let meta = file.metadata().map_err(|_| "Skipped unreadable session")?;
    let limit = MAX_FILE_BYTES.min(budget);
    if !meta.is_file() || meta.len() > limit {
        return Err("Skipped oversized or non-regular session".into());
    }
    let mut reader = std::io::BufReader::new(file.take(limit + 1));
    let mut rows = Vec::new();
    let mut total = 0u64;
    loop {
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| "Skipped unreadable session")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if n > MAX_LINE_BYTES || total > limit || rows.len() >= MAX_ROWS {
            return Err("Skipped session exceeding byte/line/row limits".into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        rows.push(serde_json::from_slice(&line).map_err(|_| "Skipped malformed JSONL session")?);
    }
    Ok((rows, total))
}
fn warning(data: &mut ScanData, message: impl Into<String>) {
    if data.warnings.len() < 100 {
        data.warnings.push(message.into());
    }
}
fn scan_roots(
    roots: &[(String, std::path::PathBuf)],
    now: chrono::DateTime<chrono::Utc>,
) -> ScanData {
    let mut data = ScanData::default();
    let mut entries_seen = 0;
    let mut files_seen = 0;
    let mut bytes = 0;
    let mut seen = std::collections::HashSet::new();
    for (provider, root) in roots {
        if !root.exists() {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if entries_seen >= MAX_ENTRIES || files_seen >= MAX_FILES || bytes >= MAX_TOTAL_BYTES {
                warning(
                    &mut data,
                    "Local session discovery reached safety limits; some sessions were not scanned",
                );
                return data;
            }
            if secure_open(&dir)
                .and_then(|f| f.metadata())
                .map(|m| !m.is_dir())
                .unwrap_or(true)
            {
                warning(
                    &mut data,
                    "Skipped symlinked or unreadable session directory",
                );
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                warning(&mut data, "Skipped unreadable session directory");
                continue;
            };
            let mut entries: Vec<_> = entries
                .take(MAX_ENTRIES - entries_seen + 1)
                .filter_map(Result::ok)
                .collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                entries_seen += 1;
                if entries_seen > MAX_ENTRIES {
                    warning(&mut data, "Local session entry limit reached");
                    return data;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    warning(&mut data, "Skipped symlinked session entry");
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if kind.is_dir() {
                    if name == "subagents" {
                        continue;
                    }
                    if (provider == "claude" && depth == 0) || (provider == "codex" && depth < 8) {
                        stack.push((entry.path(), depth + 1));
                    }
                    continue;
                }
                if !kind.is_file()
                    || !name.ends_with(".jsonl")
                    || name.starts_with("agent-")
                    || (provider == "claude" && depth != 1)
                    || (provider == "codex" && !name.starts_with("rollout-"))
                {
                    continue;
                }
                files_seen += 1;
                if files_seen > MAX_FILES {
                    warning(&mut data, "Local session file limit reached");
                    return data;
                }
                let size = entry
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(MAX_FILE_BYTES + 1);
                if size > MAX_FILE_BYTES || bytes.saturating_add(size) > MAX_TOTAL_BYTES {
                    warning(&mut data, "Skipped session exceeding byte limits");
                    continue;
                }
                match read_rows(&entry.path(), MAX_TOTAL_BYTES - bytes) {
                    Ok((rows, actual_bytes)) => {
                        bytes += actual_bytes;
                        match parse_rows(provider, &rows) {
                            Some(session) => {
                                let time =
                                    chrono::DateTime::parse_from_rfc3339(&session.last_active_at)
                                        .unwrap()
                                        .with_timezone(&chrono::Utc);
                                if time < now - chrono::Duration::days(30)
                                    || time > now + chrono::Duration::days(1)
                                {
                                    warning(
                                        &mut data,
                                        "Skipped session outside the recent 30-day window",
                                    );
                                    continue;
                                }
                                if seen.insert(session.source_id.clone()) {
                                    data.sessions.push(session);
                                }
                            }
                            None => warning(
                                &mut data,
                                "Skipped unsupported, empty, or subagent session",
                            ),
                        }
                    }
                    Err(message) => {
                        bytes += MAX_FILE_BYTES.min(MAX_TOTAL_BYTES - bytes);
                        warning(&mut data, message);
                    }
                }
            }
        }
    }
    data.sessions.sort_by(|a, b| {
        b.last_active_at
            .cmp(&a.last_active_at)
            .then(a.source_id.cmp(&b.source_id))
    });
    data
}
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ScanResponse {
    pub sessions: Vec<SessionCandidate>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionCandidate {
    pub source_id: String,
    pub provider: String,
    pub title: String,
    pub cwd: String,
    pub last_active_at: String,
    pub message_count: usize,
    pub already_imported: bool,
}
fn scan_service(
    db: &crate::database::DatabaseStore,
    roots: &[(String, std::path::PathBuf)],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ScanResponse, String> {
    let scan = scan_roots(roots, now);
    let imported = db.local_import_source_ids()?;
    Ok(ScanResponse {
        sessions: scan
            .sessions
            .iter()
            .map(|s| SessionCandidate {
                source_id: s.source_id.clone(),
                provider: s.provider.clone(),
                title: s.title(),
                cwd: s.cwd.clone(),
                last_active_at: s.last_active_at.clone(),
                message_count: s.messages.len(),
                already_imported: imported.contains(&s.source_id),
            })
            .collect(),
        warnings: scan.warnings,
    })
}
fn import_service(
    state: &crate::state::AppStateStore,
    db: &crate::database::DatabaseStore,
    roots: &[(String, std::path::PathBuf)],
    now: chrono::DateTime<chrono::Utc>,
    ids: Vec<String>,
) -> Result<ImportResponse, String> {
    if ids.len() > MAX_FILES {
        return Err("too_many_local_session_ids".into());
    }
    // Validate the literal token before lookup; never normalize client IDs.
    if ids.iter().any(|id| {
        Uuid::parse_str(id)
            .map(|u| u.to_string() != *id)
            .unwrap_or(true)
    }) {
        return Err("invalid_local_session_source_id".into());
    }
    if ids.is_empty() {
        return Ok(ImportResponse::default());
    }
    let scan = scan_roots(roots, now);
    let mut warnings = scan.warnings;
    let mut skipped = 0;
    let mut seen = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            skipped += 1;
            continue;
        }
        if let Some(session) = scan.sessions.iter().find(|s| s.source_id == id) {
            selected.push(session.clone());
        } else {
            skipped += 1;
            if warnings.len() < 100 {
                warnings
                    .push("Selected session is no longer available in allowed recent roots".into());
            }
        }
    }
    let mut result = state.import_local_sessions(db, &selected)?;
    result.skipped += skipped;
    result.warnings = warnings;
    Ok(result)
}
fn configured_roots() -> Result<Vec<(String, std::path::PathBuf)>, String> {
    let home = dirs::home_dir().ok_or("home_directory_unavailable")?;
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let codex = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    if !claude.is_absolute() || !codex.is_absolute() {
        return Err("local_session_roots_must_be_absolute".into());
    }
    Ok(vec![
        ("claude".into(), claude.join("projects")),
        ("codex".into(), codex.join("sessions")),
    ])
}
#[tauri::command]
pub async fn agent_chat_scan_local_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ScanResponse, String> {
    use tauri::Manager;
    crate::commands::agent_chat::feature_flag_on(
        &app.state::<crate::observability::ObservabilityStore>(),
    )?;
    let roots = configured_roots()?;
    tauri::async_runtime::spawn_blocking(move || {
        scan_service(
            &app.state::<crate::database::DatabaseStore>(),
            &roots,
            chrono::Utc::now(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn agent_chat_import_local_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    source_ids: Vec<String>,
) -> Result<ImportResponse, String> {
    use tauri::Manager;
    crate::commands::agent_chat::feature_flag_on(
        &app.state::<crate::observability::ObservabilityStore>(),
    )?;
    let roots = configured_roots()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<crate::state::AppStateStore>();
        let db = app.state::<crate::database::DatabaseStore>();
        let mut result = import_service(&state, &db, &roots, chrono::Utc::now(), source_ids)?;
        if !result.imported.is_empty() {
            crate::state::emit_app_state(&app);
            let ids = result
                .imported
                .iter()
                .map(|s| s.source_id.clone())
                .collect::<Vec<_>>();
            if let Err(error) = state.persist_local_import_layout(&db, &ids) {
                result.warnings.push(format!(
                    "Imported transcript is durable; layout recovery remains pending: {error}"
                ));
            }
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn claude_fixture(id: &str, timestamp: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"user","sessionId":id,"cwd":"/synthetic/project","timestamp":timestamp,"message":{"role":"user","content":"Human prompt"}})
        )
    }
    #[test]
    fn local_import_scan_is_recent_bounded_readonly_no_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let project = root.join("one");
        std::fs::create_dir_all(&project).unwrap();
        let content = claude_fixture(
            "11111111-1111-4111-8111-111111111111",
            "2026-09-29T10:00:00Z",
        );
        std::fs::write(project.join("good.jsonl"), &content).unwrap();
        std::fs::write(
            project.join("old.jsonl"),
            claude_fixture(
                "22222222-2222-4222-8222-222222222222",
                "2026-07-01T00:00:00Z",
            ),
        )
        .unwrap();
        std::fs::write(project.join("bad.jsonl"), "{bad json\n").unwrap();
        std::fs::write(project.join("large.jsonl"), "x".repeat(300_000)).unwrap();
        let sub = project.join("subagents");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("agent-a.jsonl"), &content).unwrap();
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside.jsonl");
            std::fs::write(
                &outside,
                claude_fixture(
                    "33333333-3333-4333-8333-333333333333",
                    "2026-09-29T00:00:00Z",
                ),
            )
            .unwrap();
            std::os::unix::fs::symlink(outside, project.join("escape.jsonl")).unwrap();
        }
        let result = scan_roots(
            &[("claude".into(), root)],
            "2026-09-30T00:00:00Z".parse().unwrap(),
        );
        assert_eq!(result.sessions.len(), 1);
        assert!(!result.warnings.is_empty());
        assert_eq!(
            std::fs::read_to_string(project.join("good.jsonl")).unwrap(),
            content
        );
        assert_eq!(result.sessions[0].messages.len(), 1);
    }
    #[test]
    fn local_import_transaction_groups_cwd_deduplicates_without_live_runtime() {
        let db = crate::database::DatabaseStore::new_in_memory();
        let state = crate::state::AppStateStore::default();
        state.clear_workspaces();
        let a = parse_rows(
            "claude",
            &[serde_json::from_str(&claude_fixture(
                "11111111-1111-4111-8111-111111111111",
                "2026-09-29T10:00:00Z",
            ))
            .unwrap()],
        )
        .unwrap();
        let mut b = a.clone();
        b.source_id = "22222222-2222-4222-8222-222222222222".into();
        b.provider = "codex".into();
        b.messages
            .push(("assistant".into(), "Visible answer".into()));
        let result = state
            .import_local_sessions(&db, &[a.clone(), b.clone()])
            .unwrap();
        assert_eq!(result.imported.len(), 2);
        assert_eq!(
            result.imported[0].workspace_id,
            result.imported[1].workspace_id
        );
        let snap = state.snapshot();
        assert_eq!(snap.workspaces.len(), 1);
        assert!(snap.terminal_sessions.is_empty());
        assert_eq!(snap.workspaces[0].surfaces.len(), 2);
        let record = db.get_agent_chat_session(&a.thread_id()).unwrap();
        assert!(record.sdk_session_id.is_none());
        let payloads = db.list_agent_chat_messages(&b.thread_id());
        assert_eq!(payloads.len(), 3);
        assert!(payloads.iter().any(|p| p.contains("assistant_text")));
        let repeat = state.import_local_sessions(&db, &[a, b]).unwrap();
        assert!(repeat.imported.is_empty());
        assert_eq!(repeat.skipped, 2);
        assert_eq!(db.list_agent_chat_messages(&record.thread_id).len(), 1);
    }
    #[test]
    fn local_import_recovery_rebuilds_only_pending_snapshot_panes() {
        let db = crate::database::DatabaseStore::new_in_memory();
        let state = crate::state::AppStateStore::default();
        state.clear_workspaces();
        let a = parse_rows(
            "claude",
            &[serde_json::from_str(&claude_fixture(
                "11111111-1111-4111-8111-111111111111",
                "2026-09-29T10:00:00Z",
            ))
            .unwrap()],
        )
        .unwrap();
        let result = state.import_local_sessions(&db, &[a.clone()]).unwrap();
        state.clear_workspaces();
        let recovered = state.recover_local_import_layout(&db).unwrap();
        assert_eq!(recovered, vec![a.source_id.clone()]);
        assert_eq!(state.snapshot().workspaces.len(), 1);
        assert_eq!(
            state.snapshot().workspaces[0].workspace_id.0,
            result.imported[0].workspace_id
        );
        assert!(state.snapshot().terminal_sessions.is_empty());
        state.recover_local_import_layout(&db).unwrap();
        assert_eq!(state.snapshot().workspaces[0].surfaces.len(), 1);
        db.complete_local_import_layout(&recovered).unwrap();
        state.clear_workspaces();
        assert!(state.recover_local_import_layout(&db).unwrap().is_empty());
        assert!(state.snapshot().workspaces.is_empty());
    }
    #[tokio::test]
    async fn local_import_readonly_provenance_blocks_native_provider_start() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(crate::commands::agent_chat::ProviderRegistry::new());
        app.manage(crate::observability::ObservabilityStore::default());
        let thread = "local-import-11111111-1111-4111-8111-111111111111";
        let result = crate::commands::agent_chat::ensure_live_session(
            app.handle(),
            crate::agent_provider::ProviderKind::Claude,
            &crate::agent_provider::ThreadId(thread.into()),
        )
        .await;
        assert!(result
            .unwrap_err()
            .starts_with("imported_snapshot_read_only"));
        let input = serde_json::from_value(
            json!({"thread_id":thread,"cwd":"/synthetic/project","additional_directories":[]}),
        )
        .unwrap();
        let result = crate::commands::agent_chat::agent_chat_start_session(
            app.handle().clone(),
            "unused-pane".into(),
            crate::agent_provider::ProviderKind::Codex,
            input,
            None,
        )
        .await;
        assert!(result
            .unwrap_err()
            .starts_with("imported_snapshot_read_only"));
        let db = crate::database::DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session(thread, "ws", Some("/synthetic/project"), "claude")
            .unwrap();
        let record = serde_json::to_value(db.get_agent_chat_session(thread).unwrap()).unwrap();
        assert!(
            record["imported_from"].is_null(),
            "reserved namespace is not provenance"
        );
    }
    #[test]
    fn local_import_opt_in_service_validates_ids_rescans_and_reports_imported() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects/one");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("good.jsonl");
        std::fs::write(
            &file,
            claude_fixture(
                "11111111-1111-4111-8111-111111111111",
                "2026-09-29T10:00:00Z",
            ),
        )
        .unwrap();
        let roots = vec![("claude".into(), dir.path().join("projects"))];
        let now = "2026-09-30T00:00:00Z".parse().unwrap();
        let db = crate::database::DatabaseStore::new_in_memory();
        let state = crate::state::AppStateStore::default();
        state.clear_workspaces();
        let scan = scan_service(&db, &roots, now).unwrap();
        assert_eq!(scan.sessions.len(), 1);
        assert!(!scan.sessions[0].already_imported);
        assert!(state.snapshot().workspaces.is_empty());
        assert!(db.local_import_source_ids().unwrap().is_empty());
        assert!(import_service(
            &state,
            &db,
            &roots,
            now,
            vec!["../../arbitrary.jsonl".into()]
        )
        .is_err());
        let id = scan.sessions[0].source_id.clone();
        let result =
            import_service(&state, &db, &roots, now, vec![id.clone(), id.clone()]).unwrap();
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.skipped, 1);
        assert!(scan_service(&db, &roots, now).unwrap().sessions[0].already_imported);
        std::fs::remove_file(file).unwrap();
        let stale = import_service(&state, &db, &roots, now, vec![id]).unwrap();
        assert_eq!(stale.skipped, 1);
        assert!(!stale.warnings.is_empty());
    }
    #[test]
    fn local_import_mixed_sidechains_never_own_main_session_identity() {
        let child = json!({"type":"user","isSidechain":true,"sessionId":"33333333-3333-4333-8333-333333333333","cwd":"/child","timestamp":"2026-09-29T20:00:00Z","message":{"role":"user","content":"CHILD SECRET"}});
        let main: Value = serde_json::from_str(&claude_fixture(
            "11111111-1111-4111-8111-111111111111",
            "2026-09-29T10:00:00Z",
        ))
        .unwrap();
        let normal = parse_rows("claude", &[main.clone()]).unwrap();
        let mixed = parse_rows("claude", &[child.clone(), main]).unwrap();
        assert_eq!(mixed.source_id, normal.source_id);
        assert_eq!(mixed.cwd, normal.cwd);
        assert_eq!(mixed.last_active_at, normal.last_active_at);
        assert_eq!(mixed.messages, normal.messages);
        assert!(parse_rows("claude", &[child]).is_none());
    }
    #[test]
    fn local_import_preserves_selection_in_existing_populated_workspace() {
        let db = crate::database::DatabaseStore::new_in_memory();
        let state = crate::state::AppStateStore::default();
        state.clear_workspaces();
        let workspace =
            state.create_empty_workspace_at_path(std::path::PathBuf::from("/synthetic/project"));
        state
            .create_or_reuse_agent_chat_pane(
                &workspace.0,
                Some(crate::agent_provider::ProviderKind::Claude),
                Some("/synthetic/project".into()),
                None,
                Some("existing-live-thread".into()),
            )
            .unwrap();
        let before = state.snapshot();
        let a = parse_rows(
            "claude",
            &[serde_json::from_str(&claude_fixture(
                "11111111-1111-4111-8111-111111111111",
                "2026-09-29T10:00:00Z",
            ))
            .unwrap()],
        )
        .unwrap();
        state.import_local_sessions(&db, &[a]).unwrap();
        let after = state.snapshot();
        assert_eq!(after.active_workspace_id, before.active_workspace_id);
        assert_eq!(
            after.workspaces[0].active_tab_id,
            before.workspaces[0].active_tab_id
        );
        assert_eq!(
            after.workspaces[0].active_surface_id,
            before.workspaces[0].active_surface_id
        );
        assert_eq!(
            after.workspaces[0].surfaces[0].active_pane_id,
            before.workspaces[0].surfaces[0].active_pane_id
        );
        assert_eq!(after.workspaces[0].tabs.len(), 2);
    }
    #[test]
    fn local_import_history_includes_snapshot_without_resume_cursor() {
        let db = crate::database::DatabaseStore::new_in_memory();
        let state = crate::state::AppStateStore::default();
        state.clear_workspaces();
        let a = parse_rows(
            "claude",
            &[serde_json::from_str(&claude_fixture(
                "11111111-1111-4111-8111-111111111111",
                "2026-09-29T10:00:00Z",
            ))
            .unwrap()],
        )
        .unwrap();
        let result = state.import_local_sessions(&db, &[a.clone()]).unwrap();
        let rows = db.list_agent_chat_sessions(&result.imported[0].workspace_id, None, 20);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].imported_from.as_deref(), Some("claude"));
        assert!(rows[0].sdk_session_id.is_none());
    }
    #[test]
    fn local_import_codex_excludes_injected_context_and_analysis_channels() {
        let meta = json!({"type":"session_meta","timestamp":"2026-09-29T10:00:00Z","payload":{"id":"22222222-2222-4222-8222-222222222222","cwd":"/synthetic/project","source":"cli"}});
        let context = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>SECRET</environment_context>"}]}});
        let analysis = json!({"type":"response_item","payload":{"type":"message","role":"assistant","channel":"analysis","content":[{"type":"output_text","text":"SECRET reasoning"}]}});
        let visible =
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Human prompt"}});
        let result = parse_rows("codex", &[meta, context, analysis, visible]).unwrap();
        assert_eq!(
            result.messages,
            vec![("user".into(), "Human prompt".into())]
        );
    }
    #[test]
    fn local_import_codex_mirrors_are_turn_scoped_and_preserve_repeated_prompts() {
        let meta = json!({"type":"session_meta","timestamp":"2026-09-29T10:00:00Z","payload":{"id":"22222222-2222-4222-8222-222222222222","cwd":"/synthetic/project","source":"cli"}});
        let event = |role: &str, text: &str| json!({"type":"event_msg","payload":{"type":if role == "user" {"user_message"} else {"agent_message"},"message":text}});
        let canonical = |role: &str, text: &str| json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":if role == "user" {"input_text"} else {"output_text"},"text":text}]}});
        for canonical_first in [false, true] {
            for boundaries in [false, true] {
                let mut rows = vec![meta.clone()];
                if boundaries {
                    rows.push(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"first"}}));
                }
                rows.extend([event("user", "Hello"), event("assistant", "First answer")]);
                if boundaries {
                    rows.push(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"first"}}));
                    rows.push(json!({"type":"turn_context","payload":{"turn_id":"second"}}));
                }
                let pair = if canonical_first {
                    [canonical("user", "Hello"), event("user", "Hello")]
                } else {
                    [event("user", "Hello"), canonical("user", "Hello")]
                };
                rows.extend(pair);
                rows.extend([
                    canonical("assistant", "Second answer"),
                    event("assistant", "Second answer"),
                ]);
                assert_eq!(
                    parse_rows("codex", &rows).unwrap().messages,
                    vec![
                        ("user".into(), "Hello".into()),
                        ("assistant".into(), "First answer".into()),
                        ("user".into(), "Hello".into()),
                        ("assistant".into(), "Second answer".into())
                    ],
                    "canonical_first={canonical_first} boundaries={boundaries}"
                );
            }
        }
        // No intervening answer: explicit turn boundaries still protect an
        // earlier event-only identical prompt from a later canonical mirror.
        let rows = vec![
            meta,
            json!({"type":"turn_context","payload":{"turn_id":"first"}}),
            event("user", "Hello"),
            json!({"type":"turn_context","payload":{"turn_id":"second"}}),
            event("user", "Hello"),
            canonical("user", "Hello"),
        ];
        assert_eq!(
            parse_rows("codex", &rows).unwrap().messages,
            vec![
                ("user".into(), "Hello".into()),
                ("user".into(), "Hello".into())
            ]
        );
        let mut rows = vec![rows[0].clone(), event("user", "Hello")];
        rows.extend(
            (0..33).map(|_| json!({"type":"response_item","payload":{"type":"reasoning"}})),
        );
        rows.push(canonical("user", "Hello"));
        assert_eq!(
            parse_rows("codex", &rows).unwrap().messages.len(),
            2,
            "distant equal messages must not pair"
        );
        let rows = vec![
            rows[0].clone(),
            canonical("user", "Hello"),
            canonical("user", "Hello"),
            event("user", "Hello"),
            event("user", "Hello"),
        ];
        assert_eq!(
            parse_rows("codex", &rows).unwrap().messages.len(),
            2,
            "pair each occurrence once"
        );
    }
    #[test]
    fn local_import_codex_rejects_parent_thread_even_with_cli_source() {
        let mut meta = json!({"type":"session_meta","timestamp":"2026-09-29T10:00:00Z","payload":{"id":"22222222-2222-4222-8222-222222222222","cwd":"/synthetic/project","source":"cli"}});
        let prompt =
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Child prompt"}});
        for parent in [Value::Null, json!("")] {
            meta["payload"]["parent_thread_id"] = parent;
            assert!(parse_rows("codex", &[meta.clone(), prompt.clone()]).is_some());
        }
        for parent in [
            json!("33333333-3333-4333-8333-333333333333"),
            json!({"unexpected":"parent"}),
        ] {
            meta["payload"]["parent_thread_id"] = parent;
            assert!(
                parse_rows("codex", &[meta.clone(), prompt.clone()]).is_none(),
                "populated parent must be excluded regardless of source"
            );
        }
    }
    #[test]
    fn local_import_codex_deduplicates_event_mirrors() {
        let rows = vec![
            json!({"type":"session_meta","timestamp":"2026-09-29T10:00:00Z","payload":{"id":"22222222-2222-4222-8222-222222222222","cwd":"/synthetic/project","source":"cli"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Explain widgets"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Explain widgets"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Visible answer"}]}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"Visible answer"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"SECRET"}]}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"SECRET"}]}}),
        ];
        let session = parse_rows("codex", &rows).expect("codex session");
        assert_eq!(
            session.messages,
            vec![
                ("user".into(), "Explain widgets".into()),
                ("assistant".into(), "Visible answer".into())
            ]
        );
        let mut child = rows.clone();
        child[0]["payload"]["source"] = json!({"subagent":{"thread_spawn":{}}});
        assert!(parse_rows("codex", &child).is_none());
        let events_only = vec![rows[0].clone(), rows[1].clone(), rows[4].clone()];
        assert_eq!(
            parse_rows("codex", &events_only).unwrap().messages,
            session.messages
        );
    }
    #[test]
    fn local_import_claude_visible_text_only() {
        let rows = vec![
            json!({"type":"user","sessionId":"11111111-1111-4111-8111-111111111111","cwd":"/synthetic/project","timestamp":"2026-09-29T10:00:00Z","message":{"role":"user","content":"Explain widgets"}}),
            json!({"type":"assistant","timestamp":"2026-09-29T10:01:00Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"SECRET"},{"type":"text","text":"Widgets are useful"},{"type":"tool_use","input":{"token":"SECRET"}}]}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"SECRET"}]}}),
        ];
        let session = parse_rows("claude", &rows).expect("visible conversation");
        assert_eq!(
            session.messages,
            vec![
                ("user".into(), "Explain widgets".into()),
                ("assistant".into(), "Widgets are useful".into())
            ]
        );
        assert_eq!(session.cwd, "/synthetic/project");
    }
}
