use crate::browser::BrowserManager;
use crate::config::{read_shell_appearance_or_default, read_theme_colors_or_default, ShellAppearance, ThemeColors};
use crate::indexing::{
    rebuild_index, search_index, IndexSearchResult, ProjectIndexSnapshot, ProjectIndexStatus,
    ProjectIndexStore,
};
use crate::memory::{
    add_memory_entry, generate_handoff_packet, get_project_memory, update_project_memory,
    HandoffPacket, MemoryEntryKind, MemorySource, ProjectMemorySnapshot, ProjectMemoryUpdate,
};
use crate::openflow::{
    OpenFlowCreateRunRequest, OpenFlowDesignSpec, OpenFlowRunRecord, OpenFlowRunStatus,
    OpenFlowRuntimeSnapshot, OpenFlowRuntimeStore, AgentSessionStore,
};
use crate::openflow::agent::{AgentConfig, AgentSessionState, AgentSessionStatus};
use crate::openflow::adapters::AgentAdapter;
use crate::openflow::adapters::opencode::OpenCodeAdapter;
use crate::openflow::orchestrator::{Orchestrator, OrchestratorPhase};
use crate::observability::{
    FeatureFlags, LogLevel, ObservabilitySnapshot, ObservabilityStore, PermissionPolicy,
    SafetyConfig,
};
use crate::state::{AppStateSnapshot, AppStateStore, NotificationLevel};
use crate::state::WorkspacePresetLayout;
use crate::terminal;
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager, Runtime, State};
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

static BROWSER_AUTOMATION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProxyFetchResult {
    pub html: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserEvalResult {
    pub result: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct BrowserAutomationCoordinator {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<BrowserAutomationResult, String>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAutomationAction {
    OpenUrl { url: String },
    DomSnapshot,
    AccessibilitySnapshot,
    Click { selector: String },
    Fill { selector: String, value: String },
    TypeText { text: String },
    Scroll { x: f64, y: f64 },
    Evaluate { script: String },
    Screenshot,
    ConsoleLogs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationRequest {
    pub request_id: String,
    pub browser_id: String,
    pub action: BrowserAutomationAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationResult {
    pub request_id: String,
    pub browser_id: String,
    pub data: Value,
    pub message: Option<String>,
}

async fn dispatch_browser_automation(
    app: tauri::AppHandle,
    coordinator: &State<'_, BrowserAutomationCoordinator>,
    request: BrowserAutomationRequest,
) -> Result<BrowserAutomationResult, String> {
    let (tx, rx) = oneshot::channel();
    coordinator
        .pending
        .lock()
        .unwrap()
        .insert(request.request_id.clone(), tx);

    app.emit("browser-automation-request", &request)
        .map_err(|error| format!("Failed to emit browser automation request: {error}"))?;

    match timeout(Duration::from_secs(12), rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Browser automation channel closed unexpectedly".into()),
        Err(_) => {
            coordinator.pending.lock().unwrap().remove(&request.request_id);
            Err("Browser automation request timed out".into())
        }
    }
}

// ─── OpenFlow: CLI tool and model discovery ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliToolInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingModeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

fn which_tool(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[tauri::command]
pub fn list_available_cli_tools() -> Result<Vec<CliToolInfo>, String> {
    let known: &[(&str, &str)] = &[
        ("opencode", "OpenCode"),
        ("claude", "Claude CLI"),
        ("codex", "OpenAI Codex"),
        ("aider", "Aider"),
        ("gemini", "Gemini CLI"),
    ];

    Ok(known
        .iter()
        .map(|(id, name)| {
            let path = which_tool(id);
            CliToolInfo {
                id: id.to_string(),
                name: name.to_string(),
                available: path.is_some(),
                path,
            }
        })
        .collect())
}

fn parse_opencode_models(raw: &str) -> Vec<ModelInfo> {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|line| {
            let provider = line.split('/').next().map(|p| p.to_string());
            let short = line.split('/').last().unwrap_or(line).to_string();
            ModelInfo {
                id: line.to_string(),
                name: short,
                provider,
            }
        })
        .collect()
}

fn opencode_fallback_models() -> Vec<ModelInfo> {
    parse_opencode_models(
        "github-copilot/claude-sonnet-4.6\n\
         github-copilot/claude-sonnet-4.5\n\
         github-copilot/gpt-4.1\n\
         github-copilot/gpt-5\n\
         github-copilot/gpt-5-mini\n\
         github-copilot/gemini-2.5-pro\n\
         minimax-coding-plan/MiniMax-M2.5",
    )
}

fn claude_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo { id: "claude-opus-4-5".into(), name: "claude-opus-4-5".into(), provider: Some("anthropic".into()) },
        ModelInfo { id: "claude-sonnet-4-5".into(), name: "claude-sonnet-4-5".into(), provider: Some("anthropic".into()) },
        ModelInfo { id: "claude-haiku-3-5".into(), name: "claude-haiku-3-5".into(), provider: Some("anthropic".into()) },
    ]
}

fn codex_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo { id: "codex-mini-latest".into(), name: "codex-mini-latest".into(), provider: Some("openai".into()) },
    ]
}

fn aider_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo { id: "gpt-4o".into(), name: "gpt-4o".into(), provider: Some("openai".into()) },
        ModelInfo { id: "claude-sonnet-4-5".into(), name: "claude-sonnet-4-5".into(), provider: Some("anthropic".into()) },
    ]
}

fn gemini_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo { id: "gemini-2.5-pro".into(), name: "gemini-2.5-pro".into(), provider: Some("google".into()) },
        ModelInfo { id: "gemini-2.5-flash".into(), name: "gemini-2.5-flash".into(), provider: Some("google".into()) },
    ]
}

#[tauri::command]
pub fn list_models_for_tool(tool_id: String) -> Result<Vec<ModelInfo>, String> {
    match tool_id.as_str() {
        "opencode" => {
            let output = Command::new("opencode").arg("models").output();
            match output {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    let models = parse_opencode_models(&text);
                    if models.is_empty() {
                        Ok(opencode_fallback_models())
                    } else {
                        Ok(models)
                    }
                }
                _ => Ok(opencode_fallback_models()),
            }
        }
        "claude" => Ok(claude_default_models()),
        "codex" => Ok(codex_default_models()),
        "aider" => Ok(aider_default_models()),
        "gemini" => Ok(gemini_default_models()),
        _ => Err(format!("Unknown tool: {tool_id}")),
    }
}

#[tauri::command]
pub fn list_thinking_modes_for_tool(tool_id: String) -> Result<Vec<ThinkingModeInfo>, String> {
    // Thinking modes are relevant for models that support extended reasoning
    // Currently exposed as a configurable option when using Claude models
    let modes = match tool_id.as_str() {
        "opencode" => vec![
            ThinkingModeInfo { id: "auto".into(), name: "Auto".into(), description: "Let the model decide".into() },
            ThinkingModeInfo { id: "none".into(), name: "None".into(), description: "Disable extended thinking".into() },
            ThinkingModeInfo { id: "low".into(), name: "Low".into(), description: "Minimal thinking budget".into() },
            ThinkingModeInfo { id: "medium".into(), name: "Medium".into(), description: "Balanced thinking budget".into() },
            ThinkingModeInfo { id: "high".into(), name: "High".into(), description: "Deep reasoning budget".into() },
        ],
        _ => vec![],
    };
    Ok(modes)
}

// ─── OpenFlow: Agent spawning (Phase 2) ──────────────────────────────────────

/// Communication log path for a given run.
fn comm_log_path(run_id: &str) -> String {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".codemux")
        .join("runs")
        .join(run_id)
        .join("communication.log")
        .display()
        .to_string()
}

/// Resolve the adapter for a given CLI tool id.
fn adapter_for_tool(tool_id: &str) -> Result<Box<dyn AgentAdapter>, String> {
    match tool_id {
        "opencode" => Ok(Box::new(OpenCodeAdapter)),
        other => Err(format!("No adapter available for CLI tool: {other}")),
    }
}

/// Spawn one terminal pane per agent config and start the agent process inside
/// each pane.  Returns the list of created session IDs in config order.
#[tauri::command]
pub fn spawn_openflow_agents(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    agent_store: State<'_, AgentSessionStore>,
    workspace_id: String,
    run_id: String,
    goal: String,
    working_directory: String,
    agent_configs: Vec<AgentConfig>,
) -> Result<Vec<String>, String> {
    // Ensure the communication log directory exists.
    let log_path = comm_log_path(&run_id);
    if let Some(parent) = std::path::Path::new(&log_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create comm log directory: {e}"))?;
    }

    // Write the goal to a file so agents can read it
    let goal_path = std::path::Path::new(&log_path)
        .parent()
        .unwrap()
        .join("goal.txt");
    std::fs::write(&goal_path, &goal)
        .map_err(|e| format!("Failed to write goal file: {e}"))?;
    let goal_path_str = goal_path.display().to_string();

    // Write initial goal message to the communication log
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let roles_str: Vec<String> = agent_configs.iter().map(|c| c.role.as_str().to_string()).collect();
    let initial_msg = format!(
        "[{}] [SYSTEM] GOAL: {}\n[{}] [SYSTEM] AGENTS: {}\n",
        timestamp, goal, timestamp, roles_str.join(", ")
    );
    std::fs::write(&log_path, &initial_msg)
        .map_err(|e| format!("Failed to write initial log: {e}"))?;

    // Ensure system prompts exist and write role-specific prompts for this run.
    use crate::openflow::prompts::SystemPrompts;
    SystemPrompts::ensure_prompts_exist()
        .map_err(|e| format!("Failed to create prompts directory: {e}"))?;
    SystemPrompts::ensure_wrapper_exists()
        .map_err(|e| format!("Failed to create wrapper script: {e}"))?;
    
    for config in &agent_configs {
        SystemPrompts::write_prompt_for_run(&config.role, &run_id, &log_path, config.agent_index)
            .map_err(|e| format!("Failed to write prompt for {:?}: {}", config.role, e))?;
    }

    let mut session_ids = Vec::with_capacity(agent_configs.len());

    // Spawn agents with a small delay between each to prevent resource exhaustion
    // when launching many agents (20+). This prevents thread explosion at startup.
    for (i, config) in agent_configs.iter().enumerate() {
        let adapter = adapter_for_tool(&config.cli_tool)?;
        let spec = adapter.spawn_spec(config, &run_id, &log_path, &goal_path_str, &working_directory);

        // Create a terminal session record in the workspace with the correct directory.
        let session_id = state.add_agent_terminal_to_workspace(&workspace_id, spec.title.clone(), working_directory.clone())?;

        // Register in the agent session store.
        agent_store.insert(
            session_id.0.clone(),
            AgentSessionState {
                session_id: session_id.0.clone(),
                run_id: run_id.clone(),
                config: config.clone(),
                status: AgentSessionStatus::Spawning,
            },
        );

        // Spawn the PTY with the agent command and env vars.
        crate::terminal::spawn_pty_for_agent(
            app.clone(),
            session_id.0.clone(),
            workspace_id.clone(),
            spec.argv,
            spec.env.clone(),
        );

        // Mark as running once PTY spawn is initiated.
        agent_store.update_status(&session_id.0, AgentSessionStatus::Running);

        session_ids.push(session_id.0);

        // Small delay between spawns to prevent resource exhaustion with many agents
        // 100ms per agent means 20 agents = 2 seconds total, which is reasonable
        if i < agent_configs.len() - 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Write all "agent started" entries in a single file-open after the spawn loop,
    // avoiding one OS thread per agent that raced to append to the same file.
    {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            for config in &agent_configs {
                let entry = format!(
                    "[{}] [SYSTEM] Started agent: {}\n",
                    ts,
                    config.role.as_str().to_uppercase()
                );
                let _ = f.write_all(entry.as_bytes());
            }
        }
    }

    crate::state::emit_app_state(&app);
    Ok(session_ids)
}

/// Return all agent session states for a given OpenFlow run.
#[tauri::command]
pub fn get_agent_sessions_for_run(
    agent_store: State<'_, AgentSessionStore>,
    run_id: String,
) -> Result<Vec<AgentSessionState>, String> {
    Ok(agent_store.for_run(&run_id))
}

/// Read communication log entries for a given OpenFlow run.
/// Uses incremental reading when offset is provided - only reads new content since last read.
#[tauri::command]
pub fn get_communication_log(run_id: String, offset: Option<usize>) -> Result<(Vec<CommLogEntry>, usize), String> {
    let path = comm_log_path(&run_id);
    
    // Get file metadata to determine current size
    let metadata = std::fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            return "No communication log yet".to_string();
        }
        format!("Failed to read comm log metadata: {e}")
    })?;
    
    let current_size = metadata.len() as usize;
    let start_offset = offset.unwrap_or(0);
    
    // If no offset provided or offset is 0, read from beginning (full read)
    // If offset >= current size, there's nothing new to read
    if start_offset >= current_size {
        return Ok((vec![], current_size));
    }
    
    // Read only the new content from offset to end
    let mut file = std::fs::File::open(&path).map_err(|e| {
        format!("Failed to open comm log: {e}")
    })?;
    
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(start_offset as u64)).map_err(|e| {
        format!("Failed to seek comm log: {e}")
    })?;
    
    let mut new_content = String::new();
    std::io::Read::read_to_string(&mut file, &mut new_content).map_err(|e| {
        format!("Failed to read comm log: {e}")
    })?;
    
    // Parse the new lines
    let entries: Vec<CommLogEntry> = new_content
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| parse_comm_log_line(line))
        .collect();

    Ok((entries, current_size))
}

/// Inject a user message into the communication log (goes to orchestrator).
#[tauri::command]
pub fn inject_orchestrator_message(run_id: String, message: String) -> Result<(), String> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{}] [user/inject] {}", timestamp, message);

    crate::openflow::orchestrator::Orchestrator::write_to_comm_log(&run_id, &entry)
        .map_err(|e| format!("Failed to write to comm log: {e}"))
}

/// Trigger the orchestrator to analyze communication log and advance the run.
#[tauri::command]
pub fn trigger_orchestrator_cycle(
    runtime: State<'_, OpenFlowRuntimeStore>,
    agent_store: State<'_, AgentSessionStore>,
    pty_state: State<'_, crate::terminal::PtyState>,
    run_id: String,
    offset: Option<usize>,
) -> Result<OrchestratorTriggerResult, String> {
    // Targeted read: only fetch the current phase string, not a full runtime clone.
    let current_phase_str = runtime.get_run_phase(&run_id)?;
    let phase = OrchestratorPhase::from_string(&current_phase_str);

    #[cfg(debug_assertions)]
    {
        let sessions_guard = pty_state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let total_sessions = sessions_guard.len();
        let total_pending_chunks: usize = sessions_guard
            .values()
            .map(|rt| rt.pending_output.len())
            .sum();
        eprintln!(
            "[DEBUG] trigger_orchestrator_cycle run_id={} phase={} sessions={} pending_chunks={}",
            run_id,
            current_phase_str,
            total_sessions,
            total_pending_chunks
        );
    }

    // Read the comm log - use incremental reading if offset provided
    let (entries, new_offset) = if let Some(off) = offset {
        Orchestrator::read_communication_log_incremental(&run_id, off)
            .map_err(|e| format!("Failed to read comm log: {e}"))?
    } else {
        // Fallback to full read for backwards compatibility
        let entries = Orchestrator::read_communication_log(&run_id)
            .map_err(|e| format!("Failed to read comm log: {e}"))?;
        (entries, 0)
    };

    #[cfg(debug_assertions)]
    eprintln!("[DEBUG] Read {} entries from comm log", entries.len());

    let analysis = Orchestrator::analyze_comm_log(&entries);

    #[cfg(debug_assertions)]
    eprintln!(
        "[DEBUG] Analysis: completed={:?} blocked={:?} assignments={} instance_assignments={} injections={}/{}",
        analysis.completed_roles,
        analysis.blocked_roles,
        analysis.assignments.len(),
        analysis.instance_assignments.len(),
        analysis.user_injections.len(),
        analysis.total_injections
    );

    let mut actions_taken = Vec::new();

    // Forward any new instance-level ASSIGN messages to the target agent PTYs.
    // This is what enables true parallel execution: the orchestrator writes
    // "ASSIGN BUILDER-0: task A" and "ASSIGN BUILDER-1: task B" in the comm log,
    // and we deliver those tasks directly to the respective agent PTYs.
    if !analysis.instance_assignments.is_empty() {
        let all_sessions = agent_store.for_run(&run_id);
        let new_total_assignments = analysis.last_handled_assignments + analysis.instance_assignments.len();

        for ia in &analysis.instance_assignments {
            // Find the session whose instance id matches (e.g. "builder-0")
            let target_session = all_sessions.iter().find(|s| {
                let session_instance = if matches!(s.config.role, crate::openflow::OpenFlowRole::Orchestrator) {
                    s.config.role.as_str().to_string()
                } else {
                    format!("{}-{}", s.config.role.as_str(), s.config.agent_index)
                };
                session_instance == ia.instance_id
            });

            if let Some(session) = target_session {
                let cmd = format!("opencode run \"{}\"\n", ia.task.replace('"', "\\\""));
                let session_id = session.session_id.clone();
                let write_result = {
                    let mut sessions = pty_state.sessions.lock().unwrap();
                    if let Some(pty_runtime) = sessions.get_mut(&session_id) {
                        if let Some(writer) = pty_runtime.writer.as_mut() {
                            use std::io::Write;
                            writer
                                .write_all(cmd.as_bytes())
                                .and_then(|_| writer.flush())
                                .map_err(|e| format!("PTY write error: {e}"))
                        } else {
                            Err(format!("No writer for session {session_id}"))
                        }
                    } else {
                        Err(format!("Session {session_id} not found"))
                    }
                };
                match write_result {
                    Ok(()) => {
                        #[cfg(debug_assertions)]
                        eprintln!("[DEBUG] Forwarded task to {} (session {})", ia.instance_id, session_id);
                        actions_taken.push(format!("Forwarded task to {}", ia.instance_id));
                    }
                    Err(e) => {
                        actions_taken.push(format!("Failed to reach {} PTY: {}", ia.instance_id, e));
                    }
                }
            } else {
                actions_taken.push(format!("No session found for instance {}", ia.instance_id));
            }
        }

        // Mark assignments as handled so they are not re-forwarded on the next cycle.
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let marker = format!("[{}] [SYSTEM] HANDLED_ASSIGNMENTS: {}", ts, new_total_assignments);
        let _ = Orchestrator::write_to_comm_log(&run_id, &marker);
    }

    // Determine phase: injection takes priority; otherwise use log-based logic.
    // We reuse the `analysis` already computed above — no second file read.
    let next_phase = if !analysis.user_injections.is_empty() {
        let new_total = analysis.total_injections;
        let injection_text = analysis.user_injections.join(" | ");

        // Find the orchestrator session for this run.
        let orchestrator_session = agent_store
            .for_run(&run_id)
            .into_iter()
            .find(|s| matches!(s.config.role, crate::openflow::OpenFlowRole::Orchestrator));

        if let Some(orch_session) = orchestrator_session {
            let goal_path = comm_log_path(&run_id)
                .replace("communication.log", "goal.txt");
            let prompt = format!(
                "A user message arrived during the run. Current phase: {}. User message: \"{}\". \
                Please address this message. If it asks a question, answer it directly in the comm log. \
                If it requests changes, replan accordingly. The goal file is at: {}",
                phase.as_str(),
                injection_text,
                goal_path
            );

            // Write to the PTY while holding the sessions lock for the minimum time
            // (build the command string before locking).
            let cmd = format!("opencode run \"{}\"\n", prompt.replace('"', "\\\""));
            let session_id = orch_session.session_id.clone();
            let write_result = {
                let mut sessions = pty_state.sessions.lock().unwrap();
                if let Some(pty_runtime) = sessions.get_mut(&session_id) {
                    if let Some(writer) = pty_runtime.writer.as_mut() {
                        use std::io::Write;
                        writer
                            .write_all(cmd.as_bytes())
                            .and_then(|_| writer.flush())
                            .map_err(|e| format!("PTY write error: {e}"))
                    } else {
                        Err(format!("No writer for session {session_id}"))
                    }
                } else {
                    Err(format!("Session {session_id} not found in PtyState"))
                }
            };
            match write_result {
                Ok(()) => {
                    #[cfg(debug_assertions)]
                    eprintln!("[DEBUG] Wrote prompt to orchestrator PTY {}", session_id);
                    actions_taken.push(format!(
                        "Forwarded {} user injection(s) to orchestrator PTY",
                        analysis.user_injections.len()
                    ));
                }
                Err(e) => {
                    actions_taken.push(format!("Failed to reach orchestrator PTY: {}", e));
                }
            }
        } else {
            actions_taken.push("No orchestrator session found; injection logged only".to_string());
        }

        // Acknowledge in the comm log so agents can see it.
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let ack = format!(
            "[{}] [ORCHESTRATOR] Received user message: \"{}\". Processing...",
            ts, injection_text
        );
        let _ = Orchestrator::write_to_comm_log(&run_id, &ack);

        // Mark injections as handled so they are not re-processed on the next cycle.
        let marker = format!("[{}] [SYSTEM] HANDLED_INJECTIONS: {}", ts, new_total);
        let _ = Orchestrator::write_to_comm_log(&run_id, &marker);

        Some(OrchestratorPhase::Replanning)
    } else {
        // Build per-role instance counts so the phase logic can require ALL instances
        // of a role to complete before advancing (enables true parallel execution).
        let instance_counts: std::collections::HashMap<String, usize> = {
            let sessions = agent_store.for_run(&run_id);
            let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for session in &sessions {
                if !matches!(session.config.role, crate::openflow::OpenFlowRole::Orchestrator) {
                    *counts.entry(session.config.role.as_str().to_string()).or_insert(0) += 1;
                }
            }
            counts
        };
        // No injections — use the analysis already computed above (no second read).
        Orchestrator::determine_next_phase(&phase, &analysis, &instance_counts)
    };

    #[cfg(debug_assertions)]
    eprintln!("[DEBUG] next_phase={:?}", next_phase);

    // Apply the phase change if one was determined.
    if let Some(ref new_phase) = next_phase {
        use crate::openflow::OpenFlowRunStatus;
        let new_status = match new_phase {
            OrchestratorPhase::Planning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Assigning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Executing => OpenFlowRunStatus::Executing,
            OrchestratorPhase::Verifying => OpenFlowRunStatus::Executing,
            OrchestratorPhase::Reviewing => OpenFlowRunStatus::Reviewing,
            OrchestratorPhase::WaitingApproval => OpenFlowRunStatus::AwaitingApproval,
            OrchestratorPhase::Replanning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Completed => OpenFlowRunStatus::Completed,
        };
        runtime.set_run_phase(&run_id, new_phase.as_str(), new_status)?;
    }

    Ok(OrchestratorTriggerResult {
        current_phase: phase.as_str().to_string(),
        next_phase: next_phase.as_ref().map(|p| p.as_str().to_string()),
        analysis: OrchestratorAnalysisDto {
            completed_roles: analysis
                .completed_roles
                .iter()
                .map(|r| r.as_str().to_string())
                .collect(),
            blocked_roles: analysis
                .blocked_roles
                .iter()
                .map(|r| r.as_str().to_string())
                .collect(),
            assignments_count: analysis.assignments.len(),
            user_injections_count: analysis.user_injections.len(),
        },
        actions_taken,
        comm_log_offset: new_offset,
    })
}

#[derive(serde::Serialize)]
pub struct OrchestratorTriggerResult {
    pub current_phase: String,
    pub next_phase: Option<String>,
    pub analysis: OrchestratorAnalysisDto,
    pub actions_taken: Vec<String>,
    pub comm_log_offset: usize,
}

#[derive(serde::Serialize)]
pub struct OrchestratorAnalysisDto {
    pub completed_roles: Vec<String>,
    pub blocked_roles: Vec<String>,
    pub assignments_count: usize,
    pub user_injections_count: usize,
}

/// Parse a single line from the communication log.
fn parse_comm_log_line(line: &str) -> Option<CommLogEntry> {
    // Format: [TIMESTAMP] [ROLE] message
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }

    let timestamp_end = line.find("] ")? + 2;
    let timestamp = &line[1..timestamp_end - 2];

    let remaining = &line[timestamp_end..];
    if !remaining.starts_with('[') {
        return None;
    }

    let role_end = remaining.find("] ")? + 2;
    let role = &remaining[1..role_end - 2];
    let message = &remaining[role_end..];

    Some(CommLogEntry {
        timestamp: timestamp.to_string(),
        role: role.to_string(),
        message: message.to_string(),
    })
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct CommLogEntry {
    pub timestamp: String,
    pub role: String,
    pub message: String,
}

// ─────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_current_theme() -> Result<ThemeColors, String> {
    Ok(read_theme_colors_or_default())
}

#[tauri::command]
pub fn get_shell_appearance() -> Result<ShellAppearance, String> {
    Ok(read_shell_appearance_or_default())
}

#[tauri::command]
pub fn get_app_state(state: State<'_, AppStateStore>) -> Result<AppStateSnapshot, String> {
    Ok(state.snapshot())
}

#[tauri::command]
pub fn create_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    cwd: Option<String>,
) -> Result<String, String> {
    let workspace_id = match cwd {
        Some(path) => state.create_workspace_at_path(PathBuf::from(path)),
        None => state.create_workspace(),
    };
    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }
    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn create_openflow_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    title: String,
    goal: String,
    cwd: Option<String>,
) -> Result<String, String> {
    let workspace_id = match cwd {
        Some(path) => state.create_openflow_workspace_at_path(title, goal, PathBuf::from(path)),
        None => state.create_openflow_workspace(title, goal),
    };
    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn create_workspace_with_preset(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    cwd: Option<String>,
    layout: String,
) -> Result<String, String> {
    let layout = match layout.as_str() {
        "single" => WorkspacePresetLayout::Single,
        "pair" => WorkspacePresetLayout::Pair,
        "quad" => WorkspacePresetLayout::Quad,
        "six" => WorkspacePresetLayout::Six,
        "eight" => WorkspacePresetLayout::Eight,
        "shell_browser" => WorkspacePresetLayout::ShellBrowser,
        _ => return Err(format!("Unsupported workspace preset layout: {layout}")),
    };

    let workspace_id = match cwd {
        Some(path) => state.create_workspace_with_layout(PathBuf::from(path), layout),
        None => state.create_workspace_with_layout(crate::project::current_project_root(), layout),
    };

    let snapshot = state.snapshot();
    let session_ids = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id.0 == workspace_id.0)
        .map(|workspace| crate::state::collect_terminal_sessions(&workspace.surfaces))
        .unwrap_or_default();

    for session_id in session_ids {
        terminal::spawn_pty_for_session(app.clone(), session_id);
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn activate_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    if state.activate_workspace(&workspace_id) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

#[tauri::command]
pub fn rename_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    title: String,
) -> Result<(), String> {
    if state.rename_workspace(&workspace_id, title) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

#[tauri::command]
pub fn update_workspace_cwd(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    cwd: String,
) -> Result<(), String> {
    if state.update_workspace_cwd(&workspace_id, cwd) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

#[tauri::command]
pub fn close_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<String, String> {
    let fallback = state.close_workspace(&workspace_id)?;
    crate::state::emit_app_state(&app);
    Ok(fallback.0)
}

#[tauri::command]
pub fn cycle_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    step: isize,
) -> Result<String, String> {
    let workspace_id = state
        .workspace_navigation_target(step)
        .ok_or_else(|| "No workspace navigation target available".to_string())?;

    if state.activate_workspace(&workspace_id.0) {
        crate::state::emit_app_state(&app);
        Ok(workspace_id.0)
    } else {
        Err(format!("No workspace found for {}", workspace_id.0))
    }
}

#[tauri::command]
pub fn split_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
    direction: String,
) -> Result<String, String> {
    let direction = match direction.as_str() {
        "horizontal" => crate::state::SplitDirection::Horizontal,
        "vertical" => crate::state::SplitDirection::Vertical,
        _ => return Err(format!("Unsupported split direction: {direction}")),
    };

    let session_id = state.split_pane(&pane_id, direction)?;
    terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());
    crate::state::emit_app_state(&app);
    Ok(session_id.0)
}

#[tauri::command]
pub fn activate_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<(), String> {
    eprintln!("[DEBUG] activate_pane called with pane_id: {}", pane_id);
    if state.activate_pane(&pane_id) {
        eprintln!("[DEBUG] activate_pane succeeded, emitting app state");
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        eprintln!("[DEBUG] activate_pane failed - pane not found: {}", pane_id);
        Err(format!("No pane found for {pane_id}"))
    }
}

#[tauri::command]
pub fn cycle_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    step: isize,
) -> Result<String, String> {
    let pane_id = state
        .pane_navigation_target(step)
        .ok_or_else(|| "No pane navigation target available".to_string())?;
    if state.activate_pane(&pane_id.0) {
        crate::state::emit_app_state(&app);
        Ok(pane_id.0)
    } else {
        Err(format!("No pane found for {}", pane_id.0))
    }
}

#[tauri::command]
pub fn close_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<Option<String>, String> {
    let removed_browser_id = state.pane_browser_id(&pane_id);
    let removed = state.close_pane(&pane_id)?;
    // If this pane hosted a terminal session, clean up its PTY runtime so that the
    // child process and associated threads do not leak after the pane is closed.
    if let Some(ref session_id) = removed {
        let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
        crate::terminal::close_terminal_session(
            app.clone(),
            terminal_state,
            state.clone(),
            session_id.0.clone(),
        )
        .ok();
    }
    if let Some(browser_id) = removed_browser_id {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager: State<'_, BrowserManager> = app_handle.state();
            if let Err(error) = manager.close_browser(&browser_id).await {
                eprintln!("[BROWSER] Failed to close browser {browser_id}: {error}");
            }
        });
    }
    crate::state::emit_app_state(&app);
    Ok(removed.map(|session_id| session_id.0))
}

#[tauri::command]
pub fn swap_panes(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    source_pane_id: String,
    target_pane_id: String,
) -> Result<(), String> {
    state.swap_panes(&source_pane_id, &target_pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_split(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
    child_sizes: Vec<f32>,
) -> Result<(), String> {
    state.resize_split(&pane_id, child_sizes)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_active_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    delta: f32,
) -> Result<(), String> {
    state.resize_active_pane(delta)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn notify_attention(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    message: String,
    session_id: Option<String>,
    pane_id: Option<String>,
    desktop: Option<bool>,
) -> Result<String, String> {
    let body = message.clone();
    let notification_id =
        state.add_notification(session_id, pane_id, message, NotificationLevel::Attention)?;
    if desktop.unwrap_or(true) {
        let _ = Notification::new()
            .summary("Codemux")
            .body(&body)
            .hint(notify_rust::Hint::DesktopEntry("com.codemux.app".to_string()))
            .hint(notify_rust::Hint::Transient(true))
            .urgency(notify_rust::Urgency::Critical)
            .show();
        
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
        
        let _ = std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", "class:com.codemux.app"])
            .output();
    }
    crate::state::emit_app_state(&app);
    Ok(notification_id)
}

#[tauri::command]
pub fn mark_workspace_notifications_read(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    if state.mark_workspace_notifications_read(&workspace_id) {
        crate::state::emit_app_state(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn set_notification_sound_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_notification_sound_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn create_browser_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<String, String> {
    let (pane_id, _browser_id) = state.create_browser_pane(&pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(pane_id.0)
}

#[tauri::command]
pub fn browser_open_url(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    state.update_browser_url(&browser_id, url)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_history_back(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, -1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_history_forward(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, 1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_reload(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.reload_browser(&browser_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_set_loading_state(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
    is_loading: bool,
    error: Option<String>,
) -> Result<(), String> {
    state.set_browser_loading_state(&browser_id, is_loading, error)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_capture_screenshot(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<String, String> {
    let base = dirs::cache_dir()
        .ok_or_else(|| "Could not determine cache directory".to_string())?
        .join("codemux")
        .join("screenshots");
    std::fs::create_dir_all(&base)
        .map_err(|error| format!("Failed to create screenshot directory: {error}"))?;

    let output = base.join(format!("{browser_id}.png"));
    let status = Command::new("grim")
        .arg(output.as_os_str())
        .status()
        .map_err(|error| format!("Failed to run grim for screenshot capture: {error}"))?;

    if !status.success() {
        return Err("Screenshot capture command failed".into());
    }

    let path = output.display().to_string();
    state.set_browser_screenshot_path(&browser_id, path.clone())?;
    crate::state::emit_app_state(&app);
    Ok(path)
}

#[tauri::command]
pub async fn browser_automation_run(
    app: tauri::AppHandle,
    coordinator: State<'_, BrowserAutomationCoordinator>,
    browser_id: String,
    action: BrowserAutomationAction,
) -> Result<BrowserAutomationResult, String> {
    let request = BrowserAutomationRequest {
        request_id: format!(
            "browser-automation-{}",
            BROWSER_AUTOMATION_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
        browser_id,
        action,
    };

    dispatch_browser_automation(app, &coordinator, request).await
}

#[tauri::command]
pub fn browser_automation_complete(
    coordinator: State<'_, BrowserAutomationCoordinator>,
    request_id: String,
    result: Result<BrowserAutomationResult, String>,
) -> Result<(), String> {
    let sender = coordinator
        .pending
        .lock()
        .unwrap()
        .remove(&request_id)
        .ok_or_else(|| format!("No pending browser automation request found for {request_id}"))?;

    sender
        .send(result)
        .map_err(|_| format!("Failed to deliver browser automation result for {request_id}"))?;

    Ok(())
}

#[tauri::command]
pub fn browser_proxy_fetch(url: String) -> Result<BrowserProxyFetchResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let final_url = response.url().to_string();
    let html = response.text().map_err(|e| format!("Failed to read response: {}", e))?;

    Ok(BrowserProxyFetchResult {
        html,
        final_url,
        status,
        content_type,
    })
}

#[tauri::command]
pub fn browser_proxy_screenshot(url: String) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "image/png".to_string());

    let bytes = response.bytes().map_err(|e| format!("Failed to read response: {}", e))?;

    let base64_data = BASE64.encode(&bytes);

    let mime_type = if content_type.contains("jpeg") || content_type.contains("jpg") {
        "image/jpeg"
    } else if content_type.contains("png") {
        "image/png"
    } else if content_type.contains("gif") {
        "image/gif"
    } else if content_type.contains("webp") {
        "image/webp"
    } else {
        "image/png"
    };

    Ok(format!("data:{};base64,{}", mime_type, base64_data))
}

#[tauri::command]
pub fn get_project_memory_snapshot(project_root: Option<String>) -> Result<ProjectMemorySnapshot, String> {
    get_project_memory(project_root)
}

#[tauri::command]
pub fn update_project_memory_snapshot(
    project_root: Option<String>,
    update: ProjectMemoryUpdate,
) -> Result<ProjectMemorySnapshot, String> {
    update_project_memory(project_root, update)
}

#[tauri::command]
pub fn add_project_memory_entry(
    project_root: Option<String>,
    kind: MemoryEntryKind,
    source: MemorySource,
    content: String,
    tags: Vec<String>,
    tool_name: Option<String>,
    session_label: Option<String>,
) -> Result<ProjectMemorySnapshot, String> {
    add_memory_entry(
        project_root,
        kind,
        source,
        content,
        tags,
        tool_name,
        session_label,
    )
}

#[tauri::command]
pub fn generate_project_handoff(project_root: Option<String>) -> Result<HandoffPacket, String> {
    generate_handoff_packet(project_root)
}

#[tauri::command]
pub fn rebuild_project_index(
    store: State<'_, ProjectIndexStore>,
    project_root: Option<String>,
) -> Result<ProjectIndexSnapshot, String> {
    let snapshot = rebuild_index(project_root)?;
    store.replace_snapshot(snapshot.clone());
    Ok(snapshot)
}

#[tauri::command]
pub fn get_project_index_status(store: State<'_, ProjectIndexStore>) -> Result<ProjectIndexStatus, String> {
    Ok(store.status())
}

#[tauri::command]
pub fn search_project_index(
    store: State<'_, ProjectIndexStore>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<IndexSearchResult>, String> {
    Ok(search_index(&store, &query, limit))
}

#[tauri::command]
pub fn get_openflow_design_spec() -> Result<OpenFlowDesignSpec, String> {
    Ok(crate::openflow::default_openflow_spec())
}

#[tauri::command]
pub fn get_openflow_runtime_snapshot(
    store: State<'_, OpenFlowRuntimeStore>,
) -> Result<OpenFlowRuntimeSnapshot, String> {
    Ok(store.snapshot())
}

#[tauri::command]
pub fn create_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    request: OpenFlowCreateRunRequest,
) -> Result<OpenFlowRunRecord, String> {
    Ok(store.create_run(request))
}

#[tauri::command]
pub fn advance_openflow_run_phase(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.advance_run_phase(&run_id)
}

#[tauri::command]
pub fn retry_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.retry_run(&run_id)
}

#[tauri::command]
pub fn run_openflow_autonomous_loop(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.run_autonomous_loop(&run_id)
}

#[tauri::command]
pub fn apply_openflow_review_result(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
    reviewer_score: u8,
    accepted: bool,
    issue: Option<String>,
) -> Result<OpenFlowRunRecord, String> {
    store.apply_review_result(&run_id, reviewer_score, accepted, issue)
}

#[tauri::command]
pub fn stop_openflow_run(
    app: tauri::AppHandle,
    store: State<'_, OpenFlowRuntimeStore>,
    agent_store: State<'_, AgentSessionStore>,
    app_state: State<'_, AppStateStore>,
    terminal_state: State<'_, crate::terminal::PtyState>,
    run_id: String,
    status: String,
    reason: String,
) -> Result<OpenFlowRunRecord, String> {
    let status = match status.as_str() {
        "failed" => OpenFlowRunStatus::Failed,
        "cancelled" => OpenFlowRunStatus::Cancelled,
        "awaiting_approval" => OpenFlowRunStatus::AwaitingApproval,
        _ => OpenFlowRunStatus::Failed,
    };
    let record = store.stop_run(&run_id, status, reason)?;

    // Collect all agent sessions for this run so we can tear down their PTYs
    // and remove their terminal session entries when the run is stopped.
    let agent_sessions = agent_store.for_run(&run_id);
    let session_ids: Vec<String> = agent_sessions.iter().map(|s| s.session_id.clone()).collect();
    let openflow_workspace_id = session_ids
        .first()
        .and_then(|id| app_state.workspace_id_for_session(id));

    {
        let mut sessions_guard = terminal_state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        for session_id in &session_ids {
            sessions_guard.remove(session_id);
        }
    }
    app_state.remove_terminal_sessions(&session_ids);

    if let Some(workspace_id) = openflow_workspace_id {
        let _ = app_state.close_workspace(&workspace_id.0);
    }

    // After a run is explicitly stopped, clean up in-memory runtime and agent
    // session state, and release any comm-log file handles for this run.
    let _ = store.remove_run(&run_id);
    agent_store.remove_for_run(&run_id);
    let log_path = comm_log_path(&run_id);
    crate::terminal::release_comm_log_lock(&log_path);

    crate::state::emit_app_state(&app);
    Ok(record)
}

#[tauri::command]
pub fn get_observability_snapshot(
    store: State<'_, ObservabilityStore>,
) -> Result<ObservabilitySnapshot, String> {
    Ok(store.snapshot())
}

#[tauri::command]
pub fn add_structured_log(
    store: State<'_, ObservabilityStore>,
    source: String,
    level: String,
    message: String,
    metadata: Vec<(String, String)>,
) -> Result<(), String> {
    let level = match level.as_str() {
        "warning" => LogLevel::Warning,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    store.log(&source, level, message, metadata);
    Ok(())
}

#[tauri::command]
pub fn update_feature_flags(
    store: State<'_, ObservabilityStore>,
    flags: FeatureFlags,
) -> Result<(), String> {
    store.set_feature_flags(flags);
    Ok(())
}

#[tauri::command]
pub fn update_permission_policy(
    store: State<'_, ObservabilityStore>,
    policy: PermissionPolicy,
) -> Result<(), String> {
    store.set_permission_policy(policy);
    Ok(())
}

#[tauri::command]
pub fn update_safety_config(
    store: State<'_, ObservabilityStore>,
    config: SafetyConfig,
) -> Result<(), String> {
    store.set_safety_config(config);
    Ok(())
}

#[tauri::command]
pub fn add_replay_record(
    store: State<'_, ObservabilityStore>,
    title: String,
    summary: String,
) -> Result<(), String> {
    store.add_replay_record(title, summary);
    Ok(())
}

/// Open a native folder-picker dialog, properly parented to the calling window on all
/// desktop platforms (including Linux/Wayland). The built-in JS `open()` from
/// `tauri-plugin-dialog` skips `set_parent` on Linux due to an upstream bug
/// (https://github.com/tauri-apps/plugins-workspace/issues — `commands.rs` uses
/// `#[cfg(any(windows, target_os = "macos"))]` instead of `#[cfg(desktop)]`), which
/// means the portal-gtk dialog opens with no transient-for relationship and tiles
/// instead of floating. This command fixes that by calling `set_parent` unconditionally
/// on all desktop platforms.
#[tauri::command]
pub async fn pick_folder_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();

    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Choose folder"));

    #[cfg(desktop)]
    {
        builder = builder.set_parent(&window);
    }

    builder.pick_folder(move |path| {
        let _ = tx.send(path.map(|p| p.to_string()));
    });

    rx.await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_spawn(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.spawn_browser(browser_id).await?;
    Ok("Browser spawned".to_string())
}

#[tauri::command]
pub async fn browser_navigate(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    url: String,
) -> Result<String, String> {
    manager.navigate(&browser_id, &url).await
}

#[tauri::command]
pub async fn browser_screenshot(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.screenshot(&browser_id).await
}

#[tauri::command]
pub async fn browser_click(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    x: f64,
    y: f64,
) -> Result<String, String> {
    manager.click(&browser_id, x, y).await
}

#[tauri::command]
pub async fn browser_type(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    text: String,
) -> Result<String, String> {
    manager.type_text(&browser_id, &text).await
}

#[tauri::command]
pub async fn browser_close(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.close_browser(&browser_id).await
}

#[tauri::command]
pub async fn browser_resize_viewport(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    width: u32,
    height: u32,
) -> Result<(), String> {
    manager.resize_viewport(&browser_id, width, height).await
}

#[tauri::command]
pub async fn agent_browser_spawn(
    manager: State<'_, crate::agent_browser::AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.spawn(&browser_id).await
}

#[tauri::command]
pub async fn agent_browser_run(
    manager: State<'_, crate::agent_browser::AgentBrowserManager>,
    browser_id: String,
    action: String,
    params: serde_json::Value,
) -> Result<crate::agent_browser::BrowserAutomationResult, String> {
    manager.run_command(&browser_id, &action, params).await
}

#[tauri::command]
pub async fn agent_browser_close(
    manager: State<'_, crate::agent_browser::AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.close(&browser_id).await
}

#[tauri::command]
pub fn agent_browser_get_stream_url(
    manager: State<'_, crate::agent_browser::AgentBrowserManager>,
) -> Result<String, String> {
    Ok(manager.get_stream_url())
}

#[tauri::command]
pub async fn agent_browser_screenshot(
    manager: State<'_, crate::agent_browser::AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.get_screenshot(&browser_id).await
}
