use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PROJECT_MEMORY_SCHEMA_VERSION: u32 = 1;
const MAX_PINNED_CONTEXT_ITEMS: usize = 24;
const MAX_DECISION_ITEMS: usize = 40;
const MAX_NEXT_STEP_ITEMS: usize = 24;
const MAX_SESSION_SUMMARIES: usize = 40;

static MEMORY_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Human,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryKind {
    PinnedContext,
    Decision,
    NextStep,
    SessionSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub entry_id: String,
    pub kind: MemoryEntryKind,
    pub source: MemorySource,
    pub content: String,
    pub tags: Vec<String>,
    pub tool_name: Option<String>,
    pub session_label: Option<String>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMemorySnapshot {
    pub schema_version: u32,
    pub project_root: String,
    pub project_name: String,
    pub project_brief: Option<String>,
    pub current_goal: Option<String>,
    pub current_focus: Option<String>,
    pub constraints: Vec<String>,
    pub pinned_context: Vec<MemoryEntry>,
    pub recent_decisions: Vec<MemoryEntry>,
    pub next_steps: Vec<MemoryEntry>,
    pub session_summaries: Vec<MemoryEntry>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMemoryUpdate {
    pub project_brief: Option<String>,
    pub current_goal: Option<String>,
    pub current_focus: Option<String>,
    pub constraints: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffPacket {
    pub project_name: String,
    pub project_root: String,
    pub summary: String,
    pub suggested_prompt: String,
    pub current_goal: Option<String>,
    pub current_focus: Option<String>,
    pub constraints: Vec<String>,
    pub pinned_context: Vec<String>,
    pub recent_decisions: Vec<String>,
    pub next_steps: Vec<String>,
}

pub fn get_project_memory(project_root: Option<String>) -> Result<ProjectMemorySnapshot, String> {
    load_project_memory(project_root)
}

pub fn update_project_memory(
    project_root: Option<String>,
    update: ProjectMemoryUpdate,
) -> Result<ProjectMemorySnapshot, String> {
    let mut snapshot = load_project_memory(project_root)?;

    if let Some(project_brief) = update.project_brief {
        snapshot.project_brief = Some(project_brief);
    }
    if let Some(current_goal) = update.current_goal {
        snapshot.current_goal = Some(current_goal);
    }
    if let Some(current_focus) = update.current_focus {
        snapshot.current_focus = Some(current_focus);
    }
    if let Some(constraints) = update.constraints {
        snapshot.constraints = constraints;
    }

    snapshot.updated_at_ms = current_time_ms();
    save_project_memory(&snapshot)?;
    Ok(snapshot)
}

pub fn add_memory_entry(
    project_root: Option<String>,
    kind: MemoryEntryKind,
    source: MemorySource,
    content: String,
    tags: Vec<String>,
    tool_name: Option<String>,
    session_label: Option<String>,
) -> Result<ProjectMemorySnapshot, String> {
    let mut snapshot = load_project_memory(project_root)?;
    let entry = MemoryEntry {
        entry_id: next_memory_id(),
        kind: kind.clone(),
        source,
        content,
        tags,
        tool_name,
        session_label,
        created_at_ms: current_time_ms(),
    };

    match kind {
        MemoryEntryKind::PinnedContext => {
            snapshot.pinned_context.push(entry);
            trim_entries(&mut snapshot.pinned_context, MAX_PINNED_CONTEXT_ITEMS);
        }
        MemoryEntryKind::Decision => {
            snapshot.recent_decisions.push(entry);
            trim_entries(&mut snapshot.recent_decisions, MAX_DECISION_ITEMS);
        }
        MemoryEntryKind::NextStep => {
            snapshot.next_steps.push(entry);
            trim_entries(&mut snapshot.next_steps, MAX_NEXT_STEP_ITEMS);
        }
        MemoryEntryKind::SessionSummary => {
            snapshot.session_summaries.push(entry);
            trim_entries(&mut snapshot.session_summaries, MAX_SESSION_SUMMARIES);
        }
    }

    snapshot.updated_at_ms = current_time_ms();
    save_project_memory(&snapshot)?;
    Ok(snapshot)
}

pub fn generate_handoff_packet(project_root: Option<String>) -> Result<HandoffPacket, String> {
    let snapshot = load_project_memory(project_root)?;
    let pinned_context = snapshot
        .pinned_context
        .iter()
        .rev()
        .take(8)
        .map(|entry| entry.content.clone())
        .collect::<Vec<_>>();
    let recent_decisions = snapshot
        .recent_decisions
        .iter()
        .rev()
        .take(8)
        .map(|entry| entry.content.clone())
        .collect::<Vec<_>>();
    let next_steps = snapshot
        .next_steps
        .iter()
        .rev()
        .take(8)
        .map(|entry| entry.content.clone())
        .collect::<Vec<_>>();
    let recent_sessions = snapshot
        .session_summaries
        .iter()
        .rev()
        .take(4)
        .map(|entry| match (&entry.tool_name, &entry.session_label) {
            (Some(tool), Some(label)) => format!("{tool}/{label}: {}", entry.content),
            (Some(tool), None) => format!("{tool}: {}", entry.content),
            _ => entry.content.clone(),
        })
        .collect::<Vec<_>>();

    let summary = build_summary(
        &snapshot,
        &pinned_context,
        &recent_decisions,
        &next_steps,
        &recent_sessions,
    );
    let suggested_prompt = build_handoff_prompt(
        &snapshot,
        &pinned_context,
        &recent_decisions,
        &next_steps,
        &recent_sessions,
    );

    Ok(HandoffPacket {
        project_name: snapshot.project_name,
        project_root: snapshot.project_root,
        summary,
        suggested_prompt,
        current_goal: snapshot.current_goal,
        current_focus: snapshot.current_focus,
        constraints: snapshot.constraints,
        pinned_context,
        recent_decisions,
        next_steps,
    })
}

fn load_project_memory(project_root: Option<String>) -> Result<ProjectMemorySnapshot, String> {
    let root = resolve_project_root(project_root)?;
    let path = project_memory_path(&root);

    if !path.exists() {
        let snapshot = default_project_memory(root);
        save_project_memory(&snapshot)?;
        return Ok(snapshot);
    }

    let contents = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Failed to read project memory file {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(&contents).map_err(|error| {
        format!(
            "Failed to parse project memory file {}: {error}",
            path.display()
        )
    })
}

fn save_project_memory(snapshot: &ProjectMemorySnapshot) -> Result<(), String> {
    let root = PathBuf::from(&snapshot.project_root);
    let dir = root.join(".codemux");
    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "Failed to create project memory directory {}: {error}",
            dir.display()
        )
    })?;
    let path = project_memory_path(&root);
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize project memory: {error}"))?;
    fs::write(&path, json).map_err(|error| {
        format!(
            "Failed to write project memory file {}: {error}",
            path.display()
        )
    })
}

fn resolve_project_root(project_root: Option<String>) -> Result<PathBuf, String> {
    match project_root {
        Some(root) => Ok(PathBuf::from(root)),
        None => env::current_dir()
            .map_err(|error| format!("Failed to determine current project root: {error}")),
    }
}

fn project_memory_path(project_root: &Path) -> PathBuf {
    project_root.join(".codemux").join("project-memory.json")
}

fn default_project_memory(project_root: PathBuf) -> ProjectMemorySnapshot {
    let project_name = project_root
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "codemux-project".to_string());

    ProjectMemorySnapshot {
        schema_version: PROJECT_MEMORY_SCHEMA_VERSION,
        project_root: project_root.display().to_string(),
        project_name,
        project_brief: None,
        current_goal: None,
        current_focus: None,
        constraints: vec![],
        pinned_context: vec![],
        recent_decisions: vec![],
        next_steps: vec![],
        session_summaries: vec![],
        updated_at_ms: current_time_ms(),
    }
}

fn build_summary(
    snapshot: &ProjectMemorySnapshot,
    pinned_context: &[String],
    recent_decisions: &[String],
    next_steps: &[String],
    recent_sessions: &[String],
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Project: {}", snapshot.project_name));
    if let Some(brief) = &snapshot.project_brief {
        lines.push(format!("Brief: {brief}"));
    }
    if let Some(goal) = &snapshot.current_goal {
        lines.push(format!("Goal: {goal}"));
    }
    if let Some(focus) = &snapshot.current_focus {
        lines.push(format!("Focus: {focus}"));
    }
    if !snapshot.constraints.is_empty() {
        lines.push(format!("Constraints: {}", snapshot.constraints.join("; ")));
    }
    if !pinned_context.is_empty() {
        lines.push(format!("Pinned context: {}", pinned_context.join(" | ")));
    }
    if !recent_decisions.is_empty() {
        lines.push(format!(
            "Recent decisions: {}",
            recent_decisions.join(" | ")
        ));
    }
    if !next_steps.is_empty() {
        lines.push(format!("Next steps: {}", next_steps.join(" | ")));
    }
    if !recent_sessions.is_empty() {
        lines.push(format!(
            "Recent session handoffs: {}",
            recent_sessions.join(" | ")
        ));
    }

    lines.join("\n")
}

fn build_handoff_prompt(
    snapshot: &ProjectMemorySnapshot,
    pinned_context: &[String],
    recent_decisions: &[String],
    next_steps: &[String],
    recent_sessions: &[String],
) -> String {
    let mut prompt = Vec::new();
    prompt.push(format!("Project: {}", snapshot.project_name));
    if let Some(brief) = &snapshot.project_brief {
        prompt.push(format!("Project brief: {brief}"));
    }
    if let Some(goal) = &snapshot.current_goal {
        prompt.push(format!("Current goal: {goal}"));
    }
    if let Some(focus) = &snapshot.current_focus {
        prompt.push(format!("Current focus: {focus}"));
    }
    if !snapshot.constraints.is_empty() {
        prompt.push(format!("Constraints: {}", snapshot.constraints.join("; ")));
    }
    if !pinned_context.is_empty() {
        prompt.push("Pinned context:".into());
        prompt.extend(pinned_context.iter().map(|item| format!("- {item}")));
    }
    if !recent_decisions.is_empty() {
        prompt.push("Recent decisions:".into());
        prompt.extend(recent_decisions.iter().map(|item| format!("- {item}")));
    }
    if !next_steps.is_empty() {
        prompt.push("Suggested next steps:".into());
        prompt.extend(next_steps.iter().map(|item| format!("- {item}")));
    }
    if !recent_sessions.is_empty() {
        prompt.push("Recent tool/session summaries:".into());
        prompt.extend(recent_sessions.iter().map(|item| format!("- {item}")));
    }

    prompt.join("\n")
}

fn trim_entries(entries: &mut Vec<MemoryEntry>, max_len: usize) {
    if entries.len() > max_len {
        let remove_count = entries.len() - max_len;
        entries.drain(0..remove_count);
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn next_memory_id() -> String {
    format!(
        "memory-{}",
        MEMORY_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("codemux-memory-test-{}", current_time_ms()));
        let _ = fs::create_dir_all(&root);
        root
    }

    #[test]
    fn project_memory_roundtrip_works() {
        let root = temp_project_root().join("roundtrip");
        let _ = fs::create_dir_all(&root);
        let root_str = root.display().to_string();

        let snapshot = update_project_memory(
            Some(root_str.clone()),
            ProjectMemoryUpdate {
                project_brief: Some("Build Codemux".into()),
                current_goal: Some("Add portable memory".into()),
                current_focus: Some("Implement local project memory storage".into()),
                constraints: Some(vec!["Keep memory local-first".into()]),
            },
        )
        .unwrap();

        assert_eq!(snapshot.project_brief.as_deref(), Some("Build Codemux"));

        let loaded = get_project_memory(Some(root_str.clone())).unwrap();
        assert_eq!(loaded.current_goal.as_deref(), Some("Add portable memory"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn handoff_packet_is_compact_and_useful() {
        let root = temp_project_root().join("handoff");
        let _ = fs::create_dir_all(&root);
        let root_str = root.display().to_string();

        let _ = update_project_memory(
            Some(root_str.clone()),
            ProjectMemoryUpdate {
                project_brief: Some("Linux-first AI coding workspace".into()),
                current_goal: Some("Implement portable memory".into()),
                current_focus: Some("Shared handoff packets".into()),
                constraints: Some(vec!["Do not store full chats".into()]),
            },
        )
        .unwrap();
        let _ = add_memory_entry(
            Some(root_str.clone()),
            MemoryEntryKind::PinnedContext,
            MemorySource::Human,
            "Codemux should support multiple tools".into(),
            vec!["vision".into()],
            None,
            None,
        )
        .unwrap();
        let _ = add_memory_entry(
            Some(root_str.clone()),
            MemoryEntryKind::Decision,
            MemorySource::System,
            "Use local project-scoped JSON first".into(),
            vec!["storage".into()],
            Some("opencode".into()),
            Some("session-a".into()),
        )
        .unwrap();

        let handoff = generate_handoff_packet(Some(root_str.clone())).unwrap();
        assert!(handoff.summary.contains("Implement portable memory"));
        assert!(handoff.suggested_prompt.contains("Do not store full chats"));
        assert!(handoff
            .suggested_prompt
            .contains("Use local project-scoped JSON first"));

        let _ = fs::remove_dir_all(root);
    }
}
