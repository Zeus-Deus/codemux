use crate::browser::BrowserManager;
use crate::openflow::adapters::claude::ClaudeAdapter;
use crate::openflow::adapters::opencode::OpenCodeAdapter;
use crate::openflow::adapters::AgentAdapter;
use crate::openflow::agent::{AgentConfig, AgentSessionState, AgentSessionStatus};
use crate::openflow::orchestrator::{Orchestrator, OrchestratorAnalysis, OrchestratorPhase};
use crate::openflow::{
    AgentSessionStore, OpenFlowCreateRunRequest, OpenFlowDesignSpec, OpenFlowRunRecord,
    OpenFlowRunStatus, OpenFlowRuntimeSnapshot, OpenFlowRuntimeStore,
};
use crate::state::AppStateStore;
use serde::{Deserialize, Serialize};
use std::net::{TcpListener, ToSocketAddrs};
use tauri::{Emitter, Manager, State};

const ACTIVE_STUCK_RESCUE_CYCLES: u32 = 12; // ~60s at 5s backend loop interval
const PLANNING_STUCK_RESCUE_CYCLES: u32 = 18; // ~90s at 5s backend loop interval
/// Orchestrator cycles with no progress before injecting the stuck-recovery probe.
const STUCK_PROBE_MIN_CYCLES: u32 = 10; // ~50s at 5s backend loop interval

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

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct CommLogEntry {
    pub timestamp: String,
    pub role: String,
    pub message: String,
}

#[derive(serde::Serialize)]
pub struct OrchestratorAnalysisDto {
    pub completed_roles: Vec<String>,
    pub blocked_roles: Vec<String>,
    pub assignments_count: usize,
    pub user_injections_count: usize,
}

#[derive(serde::Serialize)]
pub struct OrchestratorTriggerResult {
    pub current_phase: String,
    pub next_phase: Option<String>,
    pub analysis: OrchestratorAnalysisDto,
    pub actions_taken: Vec<String>,
    pub comm_log_offset: usize,
    pub orchestration_state: String,
    pub orchestration_detail: Option<String>,
}

fn which_tool(name: &str) -> Option<String> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_string())
        .filter(|path| !path.is_empty())
}

fn openflow_instance_id(config: &AgentConfig) -> String {
    if matches!(config.role, crate::openflow::OpenFlowRole::Orchestrator) {
        config.role.as_str().to_string()
    } else {
        format!("{}-{}", config.role.as_str(), config.agent_index)
    }
}

fn allocate_openflow_app_url() -> Result<String, String> {
    // Run-scoped preview URL for the **user project's** web app (orchestration target), not the
    // Codemux shell. Default: first bindable port in 3900–4199 so the UI shows where agents
    // should serve / you should open the preview once a dev server listens there.
    //
    // When dogfooding Codemux itself (`tauri dev`), set `CODEMUX_OPENFLOW_APP_URL=http://localhost:1420`
    // so health checks match the shell's Vite port.
    if let Ok(v) = std::env::var("CODEMUX_OPENFLOW_APP_URL") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(v.to_string());
        }
    }
    for port in 3900_u16..=4199_u16 {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            listener.set_nonblocking(true).ok();
            return Ok(format!("http://localhost:{port}"));
        }
    }
    Err("Failed to reserve an OpenFlow app port in 3900-4199".to_string())
}

fn parse_http_host_port(url: &str) -> Option<(String, u16)> {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let authority = without_scheme.split('/').next()?.trim();
    let (host, port) = authority.rsplit_once(':')?;
    Some((host.trim_matches(['[', ']']).to_string(), port.parse().ok()?))
}

fn session_working_directory(app_state: &AppStateStore, session_id: &str) -> String {
    app_state
        .snapshot()
        .terminal_sessions
        .into_iter()
        .find(|session| session.session_id.0 == session_id)
        .map(|session| session.cwd)
        .unwrap_or_else(|| ".".to_string())
}

/// Write a raw line to a PTY session. Used for the orchestrator whose wrapper
/// reads raw messages (not `opencode run "..."` commands) and re-injects context.
fn write_raw_to_session(
    pty_state: &crate::terminal::PtyState,
    session_id: &str,
    text: &str,
) -> Result<(), String> {
    let line = format!("{}\n", text);
    let mut sessions = pty_state.sessions.lock().unwrap();
    if let Some(pty_runtime) = sessions.get_mut(session_id) {
        if let Some(writer) = pty_runtime.writer.as_mut() {
            use std::io::Write;
            writer
                .write_all(line.as_bytes())
                .and_then(|_| writer.flush())
                .map_err(|error| format!("PTY write error: {error}"))
        } else {
            Err(format!("No writer for session {session_id}"))
        }
    } else {
        Err(format!("Session {session_id} not found in PtyState"))
    }
}

fn has_recent_invalid_delegation_pattern(
    entries: &[crate::openflow::orchestrator::CommLogEntry],
) -> bool {
    entries.iter().rev().take(24).any(|entry| {
        if !entry.role.eq_ignore_ascii_case("orchestrator") {
            return false;
        }

        let lower = entry.message.to_lowercase();
        lower.contains("general agent")
            || lower.contains("schema validation failure")
            || lower.contains("task failed")
            || lower.contains("must start with \"ses\"")
    })
}

fn orchestration_state_from_cycle(
    phase: &OrchestratorPhase,
    analysis: &OrchestratorAnalysis,
    actions_taken: &[String],
) -> (String, Option<String>) {
    if actions_taken.iter().any(|action| action.contains("delegation-correction")) {
        return (
            "correcting_delegation".to_string(),
            Some("Correcting invalid orchestration behavior".to_string()),
        );
    }

    if actions_taken.iter().any(|action| action.contains("stuck-run nudge")) {
        return (
            "stalled".to_string(),
            Some("Run has not advanced and was nudged".to_string()),
        );
    }

    if !analysis.blocked_roles.is_empty() {
        return (
            "blocked".to_string(),
            Some("One or more agents reported BLOCKED".to_string()),
        );
    }

    if actions_taken
        .iter()
        .any(|action| action.contains("Waiting for orchestrator response"))
        || (!analysis.user_injections.is_empty() && analysis.injections_to_forward.is_empty())
    {
        return (
            "waiting_for_response".to_string(),
            Some("Waiting for orchestrator response to user input".to_string()),
        );
    }

    match phase {
        OrchestratorPhase::Planning | OrchestratorPhase::Assigning => {
            ("active".to_string(), Some("Planning orchestration".to_string()))
        }
        OrchestratorPhase::Executing | OrchestratorPhase::Verifying | OrchestratorPhase::Reviewing => {
            ("active".to_string(), Some("Executing orchestration".to_string()))
        }
        OrchestratorPhase::WaitingApproval => {
            ("idle".to_string(), Some("Waiting for approval".to_string()))
        }
        OrchestratorPhase::Completed => (
            "idle".to_string(),
            Some("Run completed; awaiting next user action".to_string()),
        ),
        OrchestratorPhase::Blocked => (
            "error".to_string(),
            Some("Run is blocked and needs intervention".to_string()),
        ),
        OrchestratorPhase::Replanning => {
            ("active".to_string(), Some("Replanning after new information".to_string()))
        }
    }
}

fn ensure_agent_session_live(
    app: &tauri::AppHandle,
    app_state: &AppStateStore,
    agent_store: &AgentSessionStore,
    pty_state: &crate::terminal::PtyState,
    run_id: &str,
    session: &AgentSessionState,
) -> Result<bool, String> {
    let session_id = &session.session_id;
    let is_live = {
        let sessions = pty_state.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .map(|pty_runtime| pty_runtime.writer.is_some() || pty_runtime.master.is_some())
            .unwrap_or(false)
    };

    if is_live {
        return Ok(false);
    }

    let workspace_id = app_state
        .workspace_id_for_session(session_id)
        .map(|id| id.0)
        .ok_or_else(|| format!("No workspace found for session {session_id}"))?;
    let working_directory = session_working_directory(app_state, session_id);
    let goal_path = Orchestrator::goal_path(run_id).display().to_string();
    let app_url = Orchestrator::read_app_url(run_id).unwrap_or_default();
    let adapter = adapter_for_tool(&session.config.cli_tool)?;
    let comm_log_path = Orchestrator::comm_log_path(run_id).display().to_string();
    let spec = adapter.spawn_spec(
        &session.config,
        run_id,
        &comm_log_path,
        &goal_path,
        &app_url,
        &working_directory,
    );

    crate::terminal::spawn_pty_for_agent(
        app.clone(),
        session_id.clone(),
        workspace_id,
        spec.argv,
        spec.env.clone(),
        spec.execution_policy.clone(),
    );
    agent_store.update_status(session_id, AgentSessionStatus::Running);

    Ok(true)
}

fn parse_opencode_models(raw: &str) -> Vec<ModelInfo> {
    raw.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let provider = line.split('/').next().map(|segment| segment.to_string());
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
        ModelInfo {
            id: "sonnet".into(),
            name: "Claude Sonnet (latest)".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "haiku".into(),
            name: "Claude Haiku (fast/cheap)".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "opus".into(),
            name: "Claude Opus (strongest)".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "claude-haiku-4-5".into(),
            name: "Claude Haiku 4.5".into(),
            provider: Some("anthropic".into()),
        },
    ]
}

fn codex_default_models() -> Vec<ModelInfo> {
    vec![ModelInfo {
        id: "codex-mini-latest".into(),
        name: "codex-mini-latest".into(),
        provider: Some("openai".into()),
    }]
}

fn aider_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            name: "gpt-4o".into(),
            provider: Some("openai".into()),
        },
        ModelInfo {
            id: "claude-sonnet-4-5".into(),
            name: "claude-sonnet-4-5".into(),
            provider: Some("anthropic".into()),
        },
    ]
}

fn gemini_default_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gemini-2.5-pro".into(),
            name: "gemini-2.5-pro".into(),
            provider: Some("google".into()),
        },
        ModelInfo {
            id: "gemini-2.5-flash".into(),
            name: "gemini-2.5-flash".into(),
            provider: Some("google".into()),
        },
    ]
}

fn adapter_for_tool(tool_id: &str) -> Result<Box<dyn AgentAdapter>, String> {
    match tool_id {
        "opencode" => Ok(Box::new(OpenCodeAdapter)),
        "claude" => Ok(Box::new(ClaudeAdapter)),
        other => Err(format!("No adapter available for CLI tool: {other}")),
    }
}

fn analysis_dto(analysis: &OrchestratorAnalysis) -> OrchestratorAnalysisDto {
    OrchestratorAnalysisDto {
        completed_roles: analysis
            .completed_roles
            .iter()
            .map(|role| role.as_str().to_string())
            .collect(),
        blocked_roles: analysis
            .blocked_roles
            .iter()
            .map(|role| role.as_str().to_string())
            .collect(),
        assignments_count: analysis.assignments.len(),
        user_injections_count: analysis.user_injections.len(),
    }
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

#[tauri::command]
pub fn list_models_for_tool(tool_id: String) -> Result<Vec<ModelInfo>, String> {
    match tool_id.as_str() {
        "opencode" => {
            let output = std::process::Command::new("opencode")
                .arg("models")
                .output();
            match output {
                Ok(output) if output.status.success() => {
                    let text = String::from_utf8_lossy(&output.stdout);
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
    let modes = match tool_id.as_str() {
        "opencode" => vec![
            ThinkingModeInfo {
                id: "auto".into(),
                name: "Auto".into(),
                description: "Let the model decide".into(),
            },
            ThinkingModeInfo {
                id: "none".into(),
                name: "None".into(),
                description: "Disable extended thinking".into(),
            },
            ThinkingModeInfo {
                id: "low".into(),
                name: "Low".into(),
                description: "Minimal thinking budget".into(),
            },
            ThinkingModeInfo {
                id: "medium".into(),
                name: "Medium".into(),
                description: "Balanced thinking budget".into(),
            },
            ThinkingModeInfo {
                id: "high".into(),
                name: "High".into(),
                description: "Deep reasoning budget".into(),
            },
        ],
        _ => vec![],
    };
    Ok(modes)
}

#[tauri::command]
pub fn spawn_openflow_agents(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    agent_store: State<'_, AgentSessionStore>,
    runtime: State<'_, OpenFlowRuntimeStore>,
    workspace_id: String,
    run_id: String,
    goal: String,
    working_directory: String,
    agent_configs: Vec<AgentConfig>,
) -> Result<Vec<String>, String> {
    let log_path = Orchestrator::comm_log_path(&run_id);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create comm log directory: {error}"))?;
    }

    let goal_path = Orchestrator::goal_path(&run_id);
    std::fs::write(&goal_path, &goal)
        .map_err(|error| format!("Failed to write goal file: {error}"))?;
    let app_url = match Orchestrator::read_app_url(&run_id) {
        Some(existing) => existing,
        None => allocate_openflow_app_url()?,
    };
    std::fs::write(Orchestrator::app_url_path(&run_id), &app_url)
        .map_err(|error| format!("Failed to write app URL file: {error}"))?;
    let goal_path_str = goal_path.display().to_string();
    let log_path_str = log_path.display().to_string();

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let roles: Vec<String> = agent_configs
        .iter()
        .map(|config| openflow_instance_id(config).to_uppercase())
        .collect();
    let initial_message = format!(
        "[{}] [SYSTEM] GOAL: {}\n[{}] [SYSTEM] APP_URL: {}\n[{}] [SYSTEM] AGENTS: {}\n",
        timestamp,
        goal,
        timestamp,
        app_url,
        timestamp,
        roles.join(", ")
    );
    std::fs::write(&log_path, &initial_message)
        .map_err(|error| format!("Failed to write initial log: {error}"))?;

    let agents_path = Orchestrator::run_dir(&run_id).join("agents.txt");
    let agents_content = roles.join(", ");
    std::fs::write(&agents_path, &agents_content)
        .map_err(|error| format!("Failed to write agents file: {error}"))?;
    let agents_path_str = agents_path.display().to_string();

    use crate::openflow::prompts::SystemPrompts;
    SystemPrompts::ensure_prompts_exist()
        .map_err(|error| format!("Failed to create prompts directory: {error}"))?;
    SystemPrompts::ensure_wrapper_exists()
        .map_err(|error| format!("Failed to create opencode wrapper script: {error}"))?;
    SystemPrompts::ensure_claude_wrapper_exists()
        .map_err(|error| format!("Failed to create claude wrapper script: {error}"))?;

    for config in &agent_configs {
        SystemPrompts::write_prompt_for_run(
            &config.role,
            &run_id,
            &log_path_str,
            config.agent_index,
            &app_url,
        )
        .map_err(|error| format!("Failed to write prompt for {:?}: {}", config.role, error))?;
    }

    let mut session_ids = Vec::with_capacity(agent_configs.len());

    for (index, config) in agent_configs.iter().enumerate() {
        let adapter = adapter_for_tool(&config.cli_tool)?;
        let spec = adapter.spawn_spec(
            config,
            &run_id,
            &log_path_str,
            &goal_path_str,
            &app_url,
            &working_directory,
        );

        let session_id = state.add_agent_terminal_to_workspace(
            &workspace_id,
            spec.title.clone(),
            working_directory.clone(),
        )?;

        agent_store.insert(
            session_id.0.clone(),
            AgentSessionState {
                session_id: session_id.0.clone(),
                run_id: run_id.clone(),
                config: config.clone(),
                status: AgentSessionStatus::Spawning,
            },
        );

        let mut final_env = spec.env.clone();
        if matches!(config.role, crate::openflow::OpenFlowRole::Orchestrator) {
            final_env.push(("CODEMUX_OPENFLOW_AGENTS_PATH".into(), agents_path_str.clone()));
        }

        crate::terminal::spawn_pty_for_agent(
            app.clone(),
            session_id.0.clone(),
            workspace_id.clone(),
            spec.argv,
            final_env,
            spec.execution_policy.clone(),
        );

        agent_store.update_status(&session_id.0, AgentSessionStatus::Running);
        session_ids.push(session_id.0);

        if index < agent_configs.len() - 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    crate::diagnostics::openflow_breadcrumb(&format!(
        "agents_spawned run_id={} count={}",
        run_id,
        session_ids.len()
    ));

    {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            for config in &agent_configs {
                let entry = format!(
                    "[{}] [SYSTEM] Started agent: {}\n",
                    ts,
                    openflow_instance_id(config).to_uppercase()
                );
                let _ = file.write_all(entry.as_bytes());
            }
        }
    }

    crate::state::emit_app_state(&app);

    // Start the background orchestration loop for this run.
    // The loop runs continuously, driving orchestration without frontend polling.
    {
        use crate::openflow::OrchestrationLoopHandle;
        use std::sync::atomic::AtomicBool;

        let handle = OrchestrationLoopHandle {
            wake: std::sync::Arc::new(tokio::sync::Notify::new()),
            stop: std::sync::Arc::new(AtomicBool::new(false)),
            run_id: run_id.clone(),
        };
        runtime.register_loop(&run_id, handle);

        let app_clone = app.clone();
        let run_id_clone = run_id.clone();
        tauri::async_runtime::spawn(async move {
            // Initial delay to let agents start up
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            run_orchestration_loop(app_clone, run_id_clone).await;
        });

        crate::diagnostics::openflow_breadcrumb(&format!(
            "orchestration_loop_started run_id={}",
            run_id
        ));
    }

    Ok(session_ids)
}

#[tauri::command]
pub fn get_agent_sessions_for_run(
    agent_store: State<'_, AgentSessionStore>,
    run_id: String,
) -> Result<Vec<AgentSessionState>, String> {
    Ok(agent_store.for_run(&run_id))
}

#[tauri::command]
pub fn get_communication_log(
    run_id: String,
    offset: Option<usize>,
) -> Result<(Vec<CommLogEntry>, usize), String> {
    let log_path = Orchestrator::comm_log_path(&run_id);
    let metadata = std::fs::metadata(&log_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            return "No communication log yet".to_string();
        }
        format!("Failed to read comm log metadata: {error}")
    })?;

    let current_size = metadata.len() as usize;
    let start_offset = offset.unwrap_or(0);
    if start_offset >= current_size {
        return Ok((vec![], current_size));
    }

    let mut file = std::fs::File::open(&log_path)
        .map_err(|error| format!("Failed to open comm log: {error}"))?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(start_offset as u64))
        .map_err(|error| format!("Failed to seek comm log: {error}"))?;

    let mut new_content = String::new();
    file.read_to_string(&mut new_content)
        .map_err(|error| format!("Failed to read comm log: {error}"))?;

    let entries: Vec<CommLogEntry> = Orchestrator::parse_log_lines(&new_content)
        .into_iter()
        .map(|entry| CommLogEntry {
            timestamp: entry.timestamp,
            role: entry.role,
            message: entry.message,
        })
        .collect();

    Ok((entries, current_size))
}

#[tauri::command]
pub fn inject_orchestrator_message(
    run_id: String,
    message: String,
    runtime: State<'_, OpenFlowRuntimeStore>,
) -> Result<(), String> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{}] [user/inject] {}", timestamp, message);
    Orchestrator::write_to_comm_log(&run_id, &entry)
        .map_err(|error| format!("Failed to write to comm log: {error}"))?;
    // Wake the background orchestration loop immediately so it processes the injection
    runtime.wake_loop(&run_id);
    Ok(())
}

/// Core orchestration cycle logic, shared by the background loop and the manual trigger command.
async fn run_single_cycle(
    app: &tauri::AppHandle,
    runtime: &OpenFlowRuntimeStore,
    agent_store: &AgentSessionStore,
    pty_state: &crate::terminal::PtyState,
    browser_manager: &BrowserManager,
    run_id: &str,
) -> Result<OrchestratorTriggerResult, String> {
    let current_phase_str = runtime.get_run_phase(run_id)?;
    let phase = OrchestratorPhase::from_string(&current_phase_str);
    let assigned_app_url = Orchestrator::read_app_url(&run_id);

    #[cfg(debug_assertions)]
    {
        let sessions_guard = pty_state
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let total_sessions = sessions_guard.len();
        let total_pending_chunks: usize = sessions_guard
            .values()
            .map(|runtime| runtime.pending_output.len())
            .sum();

        #[cfg(target_os = "linux")]
        let rss_kb: Option<u64> =
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find(|line| line.starts_with("VmRSS:"))
                        .and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
                });
        #[cfg(not(target_os = "linux"))]
        let rss_kb: Option<u64> = None;

        let timeout_ms = 200u64;
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let mut app_url_up = false;
        let mut probe_log = String::new();
        if let Some((host, port)) = assigned_app_url.as_deref().and_then(parse_http_host_port) {
            if let Ok(addresses) = (host.as_str(), port).to_socket_addrs() {
                for address in addresses {
                    let start = std::time::Instant::now();
                    match std::net::TcpStream::connect_timeout(&address, timeout) {
                        Ok(_) => {
                            let elapsed = start.elapsed().as_millis();
                            app_url_up = true;
                            probe_log.push_str(&format!(" {} ok {}ms", address, elapsed));
                            break;
                        }
                        Err(error) => {
                            let elapsed = start.elapsed().as_millis();
                            let kind = std::io::Error::kind(&error);
                            probe_log.push_str(&format!(" {} fail {:?} {}ms", address, kind, elapsed));
                        }
                    }
                }
            }
        } else {
            probe_log.push_str("no app url assigned");
        }

        crate::diagnostics::stderr_line(&format!(
            "[DEBUG] trigger_orchestrator_cycle run_id={} phase={} sessions={} pending_chunks={} rss_kb={:?} app_url={} reachable={} probe={}",
            run_id,
            current_phase_str,
            total_sessions,
            total_pending_chunks,
            rss_kb,
            assigned_app_url.as_deref().unwrap_or("unassigned"),
            if app_url_up { "up" } else { "down" },
            probe_log.trim()
        ));
    }

    let mut entries = Orchestrator::read_communication_log(&run_id)
        .map_err(|error| format!("Failed to read comm log: {error}"))?;
    let new_offset = std::fs::metadata(Orchestrator::comm_log_path(&run_id))
        .map(|metadata| metadata.len() as usize)
        .unwrap_or(0);

    #[cfg(debug_assertions)]
    crate::diagnostics::stderr_line(&format!(
        "[DEBUG] Read {} entries from comm log",
        entries.len()
    ));

    #[cfg(debug_assertions)]
    {
        use crate::openflow::orchestrator::CommLogEntry;
        let orchestrator_msgs: Vec<&CommLogEntry> = entries
            .iter()
            .filter(|e| e.role.eq_ignore_ascii_case("orchestrator"))
            .rev()
            .take(3)
            .collect();
        if !orchestrator_msgs.is_empty() {
            let msgs_preview: Vec<String> = orchestrator_msgs
                .iter()
                .map(|e| {
                    let msg = if e.message.chars().count() > 100 {
                        format!("{}...", e.message.chars().take(100).collect::<String>())
                    } else {
                        e.message.clone()
                    };
                    format!("[{}] {}", e.timestamp, msg)
                })
                .collect();
            crate::diagnostics::stderr_line(&format!(
                "[DEBUG] Latest orchestrator messages: {}",
                msgs_preview.join(" | ")
            ));
        }

        let builder_msgs: Vec<&CommLogEntry> = entries
            .iter()
            .filter(|e| e.role.to_lowercase().starts_with("builder"))
            .rev()
            .take(3)
            .collect();
        if !builder_msgs.is_empty() {
            let msgs_preview: Vec<String> = builder_msgs
                .iter()
                .map(|e| {
                    let msg = if e.message.chars().count() > 100 {
                        format!("{}...", e.message.chars().take(100).collect::<String>())
                    } else {
                        e.message.clone()
                    };
                    format!("[{}] {}", e.timestamp, msg)
                })
                .collect();
            crate::diagnostics::stderr_line(&format!(
                "[DEBUG] Latest builder messages: {}",
                msgs_preview.join(" | ")
            ));
        }
    }

    let mut analysis = Orchestrator::analyze_comm_log(&entries);
    let mut actions_taken = Vec::new();

    if analysis.orchestrator_responded_to_pending
        && analysis.last_pending_injections > analysis.last_handled_injections
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let handled_marker = format!(
            "[{}] [SYSTEM] HANDLED_INJECTIONS: {}",
            ts, analysis.last_pending_injections
        );
        let _ = Orchestrator::write_to_comm_log(&run_id, &handled_marker);
        actions_taken.push("Recorded orchestrator response to pending user message".to_string());
        entries = Orchestrator::read_communication_log(&run_id)
            .map_err(|error| format!("Failed to refresh comm log: {error}"))?;
        analysis = Orchestrator::analyze_comm_log(&entries);
    }

    #[cfg(debug_assertions)]
    crate::diagnostics::stderr_line(&format!(
        "[DEBUG] Analysis: completed={:?} blocked={:?} assignments={} instance_assignments={} injections={}/{} forward={} pending={} responded={}",
        analysis.completed_roles,
        analysis.blocked_roles,
        analysis.assignments.len(),
        analysis.instance_assignments.len(),
        analysis.user_injections.len(),
        analysis.total_injections,
        analysis.injections_to_forward.len(),
        analysis.last_pending_injections,
        analysis.orchestrator_responded_to_pending
    ));

    // --- Auto-translator: convert internal delegation patterns to ASSIGN lines ---
    // If the orchestrator used opencode's internal "General Agent" or "Explore Agent"
    // delegation instead of literal ASSIGN lines, we detect the task descriptions and
    // automatically create ASSIGN lines for available idle workers.
    // This makes OpenFlow work reliably regardless of whether the model follows the
    // ASSIGN protocol or uses its own internal delegation system.
    // Only auto-translate if no ASSIGN lines exist at all (not just unforwarded ones).
    // Once we write synthetic ASSIGN lines, they'll be detected by normal parsing on subsequent cycles.
    let has_any_assignments = !analysis.assignments.is_empty()
        || analysis.last_handled_assignments > 0;
    if analysis.instance_assignments.is_empty() && !has_any_assignments {
        let mut auto_assignments: Vec<crate::openflow::orchestrator::InstanceAssignment> = Vec::new();
        let all_sessions = agent_store.for_run(run_id);

        // Find orchestrator entries with internal delegation patterns
        for entry in &entries {
            if !entry.role.eq_ignore_ascii_case("orchestrator") {
                continue;
            }
            // Detect patterns like "• <task> General Agent" or "• <task> Explore Agent"
            let msg = entry.message.trim();
            for line in msg.lines() {
                let line = line.trim();
                let task = if let Some(rest) = line.strip_prefix("• ") {
                    // "• Research calendar booking libraries Explore Agent"
                    let rest = rest.trim();
                    let rest = rest.strip_suffix("General Agent").or_else(|| rest.strip_suffix("Explore Agent"));
                    rest.map(|t| t.trim().to_string())
                } else {
                    None
                };

                if let Some(task_desc) = task {
                    if task_desc.is_empty() {
                        continue;
                    }
                    // Determine which role this task is for based on keywords
                    let task_lower = task_desc.to_lowercase();
                    let target_role = if task_lower.contains("research") || task_lower.contains("investigate") || task_lower.contains("find") {
                        "researcher"
                    } else if task_lower.contains("plan") || task_lower.contains("architect") || task_lower.contains("design") || task_lower.contains("structure") {
                        "planner"
                    } else if task_lower.contains("test") || task_lower.contains("verify") || task_lower.contains("check") {
                        "tester"
                    } else if task_lower.contains("review") || task_lower.contains("audit") {
                        "reviewer"
                    } else if task_lower.contains("debug") || task_lower.contains("fix") {
                        "debugger"
                    } else {
                        "builder"
                    };

                    // Find an available session for this role that hasn't been assigned yet
                    let already_assigned: std::collections::HashSet<String> = auto_assignments
                        .iter()
                        .map(|a| a.instance_id.clone())
                        .collect();

                    if let Some(session) = all_sessions.iter().find(|s| {
                        let role_str = s.config.role.as_str();
                        role_str == target_role
                            && !matches!(s.config.role, crate::openflow::OpenFlowRole::Orchestrator)
                            && !already_assigned.contains(&format!("{}-{}", role_str, s.config.agent_index))
                    }) {
                        let instance_id = format!("{}-{}", session.config.role.as_str(), session.config.agent_index);
                        auto_assignments.push(crate::openflow::orchestrator::InstanceAssignment {
                            instance_id: instance_id.clone(),
                            task: task_desc.clone(),
                        });
                    }
                }
            }
        }

        if !auto_assignments.is_empty() {
            // Write the auto-translated assignments to the comm log so they're tracked
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            for assignment in &auto_assignments {
                let assign_entry = format!(
                    "[{}] [ORCHESTRATOR] ASSIGN {}: {}",
                    ts,
                    assignment.instance_id.to_uppercase(),
                    assignment.task
                );
                let _ = Orchestrator::write_to_comm_log(run_id, &assign_entry);
            }

            #[cfg(debug_assertions)]
            crate::diagnostics::stderr_line(&format!(
                "[DEBUG] Auto-translated {} internal delegation(s) to ASSIGN lines",
                auto_assignments.len()
            ));
            actions_taken.push(format!(
                "Auto-translated {} internal delegation(s) to ASSIGN lines",
                auto_assignments.len()
            ));

            // Inject into analysis so they get forwarded to workers
            analysis.assignments.extend(auto_assignments.iter().map(|a| {
                format!("ASSIGN {}: {}", a.instance_id.to_uppercase(), a.task)
            }));
            analysis.instance_assignments = auto_assignments;

            // Re-read comm log to include the new ASSIGN entries
            entries = Orchestrator::read_communication_log(run_id)
                .map_err(|e| format!("Failed to refresh comm log: {e}"))?;
            analysis = Orchestrator::analyze_comm_log(&entries);
        }
    }

    let app_state = app.state::<AppStateStore>();

    let orchestrator_session = agent_store.for_run(run_id).into_iter().find(|session| {
        matches!(
            session.config.role,
            crate::openflow::OpenFlowRole::Orchestrator
        )
    });

    if !analysis.instance_assignments.is_empty() {
        let all_sessions = agent_store.for_run(&run_id);
        let new_total_assignments =
            analysis.last_handled_assignments + analysis.instance_assignments.len();

        // Skip re-assigning to agents that already completed or hit max turns
        let completed_set: std::collections::HashSet<String> = analysis
            .completed_instances
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        for assignment in &analysis.instance_assignments {
            if completed_set.contains(&assignment.instance_id.to_lowercase()) {
                actions_taken.push(format!(
                    "Skipped re-assignment to already-completed {}",
                    assignment.instance_id
                ));
                continue;
            }
            let target_session = all_sessions.iter().find(|session| {
                let session_instance = if matches!(
                    session.config.role,
                    crate::openflow::OpenFlowRole::Orchestrator
                ) {
                    session.config.role.as_str().to_string()
                } else {
                    format!(
                        "{}-{}",
                        session.config.role.as_str(),
                        session.config.agent_index
                    )
                };
                session_instance == assignment.instance_id
            });

            if let Some(session) = target_session {
                // Prefix the task with "ASSIGN INSTANCE-ID:" so the worker agent
                // recognizes it as an official assignment and starts working.
                // Without this prefix, workers refuse to act ("waiting for ASSIGN message").
                let prefixed_task = format!(
                    "ASSIGN {}: {}",
                    assignment.instance_id.to_uppercase(),
                    assignment.task
                );
                let session_id = session.session_id.clone();
                let write_result = write_raw_to_session(pty_state, &session_id, &prefixed_task);

                match write_result {
                    Ok(()) => {
                        #[cfg(debug_assertions)]
                        crate::diagnostics::stderr_line(&format!(
                            "[DEBUG] Forwarded task to {} (session {}): {}",
                            assignment.instance_id, session_id, assignment.task
                        ));
                        actions_taken.push(format!("Forwarded task to {}", assignment.instance_id));
                    }
                    Err(error) => {
                        actions_taken.push(format!(
                            "Failed to reach {} PTY: {}",
                            assignment.instance_id, error
                        ));
                    }
                }
            } else {
                actions_taken.push(format!(
                    "No session found for instance {}",
                    assignment.instance_id
                ));
                #[cfg(debug_assertions)]
                crate::diagnostics::stderr_line(&format!(
                    "[DEBUG] No session found for instance {} - available sessions: {:?}",
                    assignment.instance_id,
                    all_sessions.iter().map(|s| {
                        let role = s.config.role.as_str();
                        let idx = s.config.agent_index;
                        format!("{}-{}", role, idx)
                    }).collect::<Vec<_>>()
                ));
            }
        }

        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let marker = format!(
            "[{}] [SYSTEM] HANDLED_ASSIGNMENTS: {}",
            ts, new_total_assignments
        );
        let _ = Orchestrator::write_to_comm_log(&run_id, &marker);
    }

    // --- Relay worker DONE/BLOCKED messages to the orchestrator ---
    // The orchestrator can't read the comm log directly. When workers complete tasks
    // or report blocks, the backend must relay this so it can assign follow-up work.
    if let Some(ref orchestrator_session) = orchestrator_session {
        // Find the last relay marker to know how many DONE/BLOCKED we already relayed
        let last_relay_count: usize = entries
            .iter()
            .rev()
            .find_map(|e| {
                if e.role.eq_ignore_ascii_case("system") {
                    e.message
                        .strip_prefix("DONE_RELAY_COUNT: ")
                        .and_then(|n| n.trim().parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0);

        // Count ALL current DONE/BLOCKED messages
        let mut all_done_blocked: Vec<(String, String)> = Vec::new();
        for entry in &entries {
            let role_lower = entry.role.to_lowercase();
            if role_lower == "orchestrator" || role_lower == "system" {
                continue;
            }
            if entry.message.contains("DONE:") {
                let summary = entry.message.chars().take(300).collect::<String>();
                all_done_blocked.push((entry.role.to_uppercase(), summary));
            } else if entry.message.starts_with("BLOCKED:") {
                let summary = entry.message.chars().take(300).collect::<String>();
                all_done_blocked.push((entry.role.to_uppercase(), summary));
            } else if entry.message.contains("Error: Reached max turns") {
                all_done_blocked.push((
                    entry.role.to_uppercase(),
                    format!("DONE (max turns reached, work may be partial): {}", entry.role),
                ));
            }
        }

        // Only relay NEW completions (ones after the last relay count)
        if all_done_blocked.len() > last_relay_count {
            let new_completions: Vec<String> = all_done_blocked[last_relay_count..]
                .iter()
                .map(|(role, msg)| format!("{}: {}", role, msg))
                .collect();

            if !new_completions.is_empty() {
                let relay_text = format!(
                    "AGENT STATUS UPDATE: {} NEW completion(s). {}. Assign follow-up tasks. Do NOT re-assign already completed work.",
                    new_completions.len(),
                    new_completions.join(" | ")
                );

                let _ = write_raw_to_session(
                    pty_state,
                    &orchestrator_session.session_id,
                    &relay_text,
                );

                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let _ = Orchestrator::write_to_comm_log(
                    run_id,
                    &format!(
                        "[{}] [SYSTEM] DONE_RELAY_COUNT: {}",
                        ts,
                        all_done_blocked.len()
                    ),
                );

                actions_taken.push(format!(
                    "Relayed {} new DONE/BLOCKED to orchestrator (total: {})",
                    new_completions.len(),
                    all_done_blocked.len()
                ));
            }
        }
    }

    let stuck_state_for_probe = runtime.get_stuck_state(run_id);
    let invalid_delegation_detected = analysis.instance_assignments.is_empty()
        && has_recent_invalid_delegation_pattern(entries.as_slice());

    // Probe delivery: write to comm log for the record AND forward to orchestrator PTY
    // so the orchestrator actually sees it. Use simple, shell-safe probe text.
    // Only fire ONE probe per stuck cycle — probe_injected is only cleared when
    // counts_changed (new ASSIGN/DONE), not on mere log growth.

    if invalid_delegation_detected && !stuck_state_for_probe.probe_injected {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = Orchestrator::write_to_comm_log(
            run_id,
            &format!(
                "[{}] [SYSTEM] PROBE: Invalid delegation detected, sent correction to orchestrator",
                ts
            ),
        );
        // Send raw message to orchestrator PTY — the wrapper re-injects full context
        if let Some(ref orchestrator_session) = orchestrator_session {
            let _ = ensure_agent_session_live(
                app, &app_state, &agent_store, pty_state, run_id, orchestrator_session,
            );
            let _ = write_raw_to_session(
                pty_state,
                &orchestrator_session.session_id,
                "STOP: General Agent and Explore Agent do NOT work here. You must output literal ASSIGN lines. Example: ASSIGN BUILDER-3: Create the app. Do this now.",
            );
        }
        runtime.mark_probe_sent(run_id);
        actions_taken.push("Sent delegation-correction probe to orchestrator".to_string());
    }

    if stuck_state_for_probe.consecutive_no_progress_cycles >= STUCK_PROBE_MIN_CYCLES
        && !stuck_state_for_probe.probe_injected
    {
        if let Some(ref orchestrator_session) = orchestrator_session {
            if ensure_agent_session_live(
                app, &app_state, &agent_store, pty_state, run_id, orchestrator_session,
            )? {
                actions_taken.push("Respawned orchestrator for stuck-run nudge".to_string());
            }

            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = Orchestrator::write_to_comm_log(
                run_id,
                &format!(
                    "[{}] [SYSTEM] PROBE: No progress detected, sent nudge to orchestrator",
                    ts
                ),
            );
            // Send raw message — wrapper re-injects system prompt + goal + agents context
            let _ = write_raw_to_session(
                pty_state,
                &orchestrator_session.session_id,
                "No orchestration progress detected. Emit ASSIGN lines for the agent instance IDs, or write STATUS or BLOCKED with the reason.",
            );
        }
        runtime.mark_probe_sent(run_id);
        actions_taken.push("Sent stuck-run nudge to orchestrator".to_string());
    }

    let next_phase = if !analysis.user_injections.is_empty() {
        let injection_text = analysis.injections_to_forward.join(" | ");

        if let Some(ref orchestrator_session) = orchestrator_session {
            if !analysis.injections_to_forward.is_empty() {
                if ensure_agent_session_live(
                    &app,
                    &app_state,
                    &agent_store,
                    &pty_state,
                    &run_id,
                    orchestrator_session,
                )? {
                    actions_taken.push("Respawned orchestrator session for follow-up message".to_string());
                }

                // Inline goal text so OpenCode does not need to read ~/.local/share/.../goal.txt
                // (sandbox often auto-rejects external_directory access for that path).
                let goal_path = Orchestrator::goal_path(&run_id);
                let goal_excerpt = std::fs::read_to_string(&goal_path)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                const MAX_GOAL_INLINE: usize = 8000;
                let goal_excerpt = if goal_excerpt.chars().count() > MAX_GOAL_INLINE {
                    let mut t: String = goal_excerpt.chars().take(MAX_GOAL_INLINE).collect();
                    t.push_str("… (truncated)");
                    t
                } else {
                    goal_excerpt
                };
                let prompt = format!(
                    "A user message arrived. User message: \"{}\".\n\
                     Please address this message:\n\
                     - If it asks a question, answer it directly in the comm log.\n\
                     - If it requests changes, create ASSIGN messages to delegate work.\n\
                     - IMPORTANT: After answering or delegating, DO NOT exit. Wait for more user messages.\n\
                     Use the run goal below as context. Prefer the project working directory for files; avoid requiring access outside the workspace unless the user explicitly needs it.\n\
                     Current run goal:\n{}",
                    injection_text,
                    goal_excerpt
                );

                let session_id = orchestrator_session.session_id.clone();
                #[cfg(debug_assertions)]
                crate::diagnostics::stderr_line(&format!(
                    "[DEBUG] About to write to orchestrator PTY, session_id={}",
                    session_id
                ));

                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let pending_marker =
                    format!("[{}] [SYSTEM] INJECTION_PENDING: {}", ts, analysis.total_injections);
                let _ = Orchestrator::write_to_comm_log(&run_id, &pending_marker);

                // Use write_raw_to_session for orchestrator — the wrapper re-injects
                // full system prompt + goal + agents context automatically.
                let write_result = write_raw_to_session(&pty_state, &session_id, &prompt);

                match write_result {
                    Ok(()) => {
                        #[cfg(debug_assertions)]
                        crate::diagnostics::stderr_line(&format!(
                            "[DEBUG] Wrote prompt to orchestrator PTY {}",
                            session_id
                        ));
                        actions_taken.push(format!(
                            "Forwarded {} new user injection(s) to orchestrator PTY",
                            analysis.injections_to_forward.len()
                        ));
                    }
                    Err(error) => {
                        actions_taken.push(format!("Failed to reach orchestrator PTY: {}", error));
                    }
                }
            } else if analysis.last_pending_injections > analysis.last_handled_injections {
                actions_taken.push("Waiting for orchestrator response to earlier user message".to_string());
            } else {
                actions_taken.push("User message is present but there was nothing new to forward".to_string());
            }
        } else {
            actions_taken.push("No orchestrator session found; injection logged only".to_string());
        }

        if !analysis.injections_to_forward.is_empty() {
            Some(OrchestratorPhase::Replanning)
        } else {
            None
        }
    } else {
        let instance_counts: std::collections::HashMap<String, usize> = {
            let sessions = agent_store.for_run(&run_id);
            let mut counts = std::collections::HashMap::new();
            for session in &sessions {
                if !matches!(
                    session.config.role,
                    crate::openflow::OpenFlowRole::Orchestrator
                ) {
                    *counts
                        .entry(session.config.role.as_str().to_string())
                        .or_insert(0) += 1;
                }
            }
            counts
        };

        let builder_count = instance_counts.get("builder").copied().unwrap_or(0);
        let completed_builder_instances: usize = analysis
            .completed_instances
            .iter()
            .filter(|id| {
                let lower = id.to_lowercase();
                lower == "builder" || lower.starts_with("builder-")
            })
            .count();

        let builder_all_done = builder_count > 0 && completed_builder_instances >= builder_count;

        #[cfg(debug_assertions)]
        crate::diagnostics::stderr_line(&format!(
            "[DEBUG] Builder check: builder_count={}, completed_builder_instances={}, builder_all_done={}",
            builder_count, completed_builder_instances, builder_all_done
        ));

        let builder_verified_live = if matches!(phase, OrchestratorPhase::Executing) && builder_all_done {
            if let Some(app_url) = assigned_app_url.as_deref() {
                let verify_browser_id = format!("openflow-verify-{}", run_id);
                match browser_manager.spawn_browser(verify_browser_id.clone()).await {
                    Ok(_) => {
                        match browser_manager.navigate(&verify_browser_id, app_url).await {
                            Ok(_) => {
                                #[cfg(debug_assertions)]
                                crate::diagnostics::stderr_line(&format!(
                                    "[DEBUG] Browser verification succeeded: app is live at {}",
                                    app_url
                                ));
                                let _ = browser_manager.close_browser(&verify_browser_id).await;
                                true
                            }
                            Err(e) => {
                                #[cfg(debug_assertions)]
                                crate::diagnostics::stderr_line(&format!(
                                    "[DEBUG] Browser verification failed: app not accessible at {} - {}",
                                    app_url, e
                                ));
                                let _ = browser_manager.close_browser(&verify_browser_id).await;
                                false
                            }
                        }
                    }
                    Err(e) => {
                        #[cfg(debug_assertions)]
                        crate::diagnostics::stderr_line(&format!(
                            "[DEBUG] Browser verification spawn failed: {}",
                            e
                        ));
                        false
                    }
                }
            } else {
                true
            }
        } else {
            true
        };

        let last_activity_timestamp = entries.iter().rev().find_map(|e| {
            Orchestrator::parse_timestamp(&e.timestamp)
        });

        let session_count = agent_store.for_run(&run_id).len();
        let assignment_count = analysis.instance_assignments.len();
        let done_count = analysis.completed_instances.len();

        // Count meaningful entries (exclude SYSTEM entries and bash errors) for progress detection.
        // This prevents error output from broken probes from counting as "progress".
        let meaningful_entries_len = entries.iter().filter(|e| {
            let role_lower = e.role.to_lowercase();
            role_lower != "system"
                && !e.message.starts_with("bash:")
                && !e.message.contains("command not found")
        }).count();

        let (updated_stuck_state, made_progress) = runtime.update_stuck_state(
            &run_id,
            session_count,
            assignment_count,
            done_count,
            entries.len(),
            meaningful_entries_len,
            last_activity_timestamp,
        );

        #[cfg(debug_assertions)]
        crate::diagnostics::stderr_line(&format!(
            "[DEBUG] Stuck detection: cycles={}, made_progress={}, probe_sent={}, rescue_attempted={}",
            updated_stuck_state.consecutive_no_progress_cycles,
            made_progress,
            updated_stuck_state.probe_injected,
            updated_stuck_state.rescue_attempted
        ));

    let stuck_rescue_threshold = if matches!(phase, OrchestratorPhase::Planning | OrchestratorPhase::Assigning) {
        PLANNING_STUCK_RESCUE_CYCLES
    } else {
        ACTIVE_STUCK_RESCUE_CYCLES
    };

    if updated_stuck_state.consecutive_no_progress_cycles >= stuck_rescue_threshold
        && !updated_stuck_state.rescue_attempted
        && matches!(phase, OrchestratorPhase::Planning | OrchestratorPhase::Assigning | OrchestratorPhase::Executing | OrchestratorPhase::Replanning | OrchestratorPhase::Verifying | OrchestratorPhase::Reviewing)
    {
            let rescue_sessions = agent_store.for_run(&run_id);
            let mut rescued_any = false;

            for session in &rescue_sessions {
                let session_id = &session.session_id;
                let is_alive = {
                    let sessions = pty_state.sessions.lock().unwrap();
                    sessions.get(session_id).map(|pty_runtime| {
                        matches!(
                            pty_runtime.last_status.state,
                            crate::terminal::TerminalLifecycleState::Ready
                        )
                    }).unwrap_or(false)
                };

                if !is_alive {
                    if ensure_agent_session_live(
                        &app,
                        &app_state,
                        &agent_store,
                        &pty_state,
                        &run_id,
                        &session,
                    )? {
                        actions_taken.push(format!("Rescued dead agent session {}", session_id));
                        #[cfg(debug_assertions)]
                        crate::diagnostics::stderr_line(&format!(
                            "[DEBUG] Rescued dead agent session {}",
                            session_id
                        ));
                        rescued_any = true;
                    }
                }
            }

            // Notify orchestrator about dead agents so it doesn't wait for them
            if rescued_any {
                if let Some(ref orch) = orchestrator_session {
                    let dead_agents: Vec<String> = rescue_sessions
                        .iter()
                        .filter(|s| {
                            let sid = &s.session_id;
                            let sessions = pty_state.sessions.lock().unwrap();
                            !sessions.get(sid).map(|r| matches!(
                                r.last_status.state,
                                crate::terminal::TerminalLifecycleState::Ready
                            )).unwrap_or(false)
                        })
                        .filter(|s| !matches!(s.config.role, crate::openflow::OpenFlowRole::Orchestrator))
                        .map(|s| format!("{}-{}", s.config.role.as_str(), s.config.agent_index))
                        .collect();
                    if !dead_agents.is_empty() {
                        let _ = write_raw_to_session(
                            pty_state,
                            &orch.session_id,
                            &format!(
                                "AGENT TERMINATED: {} hit max turns or crashed. Do NOT wait for them. Proceed with remaining agents or declare RUN COMPLETE.",
                                dead_agents.join(", ")
                            ),
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
                runtime.mark_rescue_attempted(&run_id);
            }
        }

        let consecutive_replans = updated_stuck_state.replan_consecutive;
        let replan_start_done_count = updated_stuck_state.replan_start_done_count;

        let potential_next_phase = Orchestrator::determine_next_phase(
            &phase,
            &analysis,
            &instance_counts,
            consecutive_replans,
            replan_start_done_count,
        );

        if matches!(phase, OrchestratorPhase::Replanning) {
            if potential_next_phase == Some(OrchestratorPhase::Planning) {
                if !made_progress {
                    runtime.increment_replan_consecutive(&run_id, done_count);
                } else {
                    runtime.reset_replan_tracking(&run_id);
                }
            }
        }

        if matches!(phase, OrchestratorPhase::Executing) {
            if let Some(OrchestratorPhase::Verifying) = potential_next_phase {
                if !builder_verified_live {
                    #[cfg(debug_assertions)]
                    crate::diagnostics::stderr_line(&format!(
                        "[DEBUG] Browser verification failed but allowing transition to Verifying"
                    ));
                    actions_taken.push("Browser verification failed - continuing to Verifying anyway".to_string());
                }
            }
        }

        potential_next_phase
    };

    #[cfg(debug_assertions)]
    crate::diagnostics::stderr_line(&format!("[DEBUG] next_phase={:?}", next_phase));

    let (orchestration_state, orchestration_detail) =
        orchestration_state_from_cycle(&phase, &analysis, &actions_taken);
    let _ = runtime.set_orchestration_state(
        &run_id,
        match orchestration_state.as_str() {
            "correcting_delegation" => crate::openflow::OpenFlowOrchestrationState::CorrectingDelegation,
            "waiting_for_response" => crate::openflow::OpenFlowOrchestrationState::WaitingForResponse,
            "stalled" => crate::openflow::OpenFlowOrchestrationState::Stalled,
            "blocked" => crate::openflow::OpenFlowOrchestrationState::Blocked,
            "idle" => crate::openflow::OpenFlowOrchestrationState::Idle,
            "error" => crate::openflow::OpenFlowOrchestrationState::Error,
            _ => crate::openflow::OpenFlowOrchestrationState::Active,
        },
        orchestration_detail.clone(),
    );

    if matches!(phase, OrchestratorPhase::Blocked) {
        runtime.set_orchestration_state(
            &run_id,
            crate::openflow::OpenFlowOrchestrationState::Blocked,
            Some("Run is blocked and requires manual intervention".to_string()),
        )?;
    } else if let Some(ref new_phase) = next_phase {
        let new_status = match new_phase {
            OrchestratorPhase::Planning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Assigning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Executing => OpenFlowRunStatus::Executing,
            OrchestratorPhase::Verifying => OpenFlowRunStatus::Executing,
            OrchestratorPhase::Reviewing => OpenFlowRunStatus::Reviewing,
            OrchestratorPhase::WaitingApproval => OpenFlowRunStatus::AwaitingApproval,
            OrchestratorPhase::Replanning => OpenFlowRunStatus::Planning,
            OrchestratorPhase::Completed => OpenFlowRunStatus::Completed,
            OrchestratorPhase::Blocked => OpenFlowRunStatus::Blocked,
        };
        runtime.set_run_phase(&run_id, new_phase.as_str(), new_status)?;
    }

    Ok(OrchestratorTriggerResult {
        current_phase: phase.as_str().to_string(),
        next_phase: next_phase.as_ref().map(|phase| phase.as_str().to_string()),
        analysis: analysis_dto(&analysis),
        actions_taken,
        comm_log_offset: new_offset,
        orchestration_state,
        orchestration_detail,
    })
}

/// Tauri command wrapper for manual orchestrator cycle triggers.
/// The primary orchestration driver is now the background loop started by `spawn_openflow_agents`.
#[tauri::command]
pub async fn trigger_orchestrator_cycle(
    app: tauri::AppHandle,
    runtime: State<'_, OpenFlowRuntimeStore>,
    agent_store: State<'_, AgentSessionStore>,
    pty_state: State<'_, crate::terminal::PtyState>,
    browser_manager: State<'_, BrowserManager>,
    run_id: String,
    _offset: Option<usize>,
) -> Result<OrchestratorTriggerResult, String> {
    run_single_cycle(&app, &runtime, &agent_store, &pty_state, &browser_manager, &run_id).await
}

/// Background orchestration loop that runs continuously for a given run.
/// Replaces the frontend-driven polling model. Wakes immediately on user injection.
pub async fn run_orchestration_loop(app: tauri::AppHandle, run_id: String) {
    use std::sync::atomic::Ordering;

    let runtime = app.state::<OpenFlowRuntimeStore>();
    let agent_store = app.state::<AgentSessionStore>();
    let pty_state = app.state::<crate::terminal::PtyState>();
    let browser_manager = app.state::<BrowserManager>();

    // Get the wake/stop handles via public accessor
    let (wake, stop): (std::sync::Arc<tokio::sync::Notify>, std::sync::Arc<std::sync::atomic::AtomicBool>) = {
        match runtime.get_loop_handles(&run_id) {
            Some(handles) => handles,
            None => {
                crate::diagnostics::stderr_line(&format!(
                    "[orchestration-loop] No loop handle found for run {}, exiting",
                    run_id
                ));
                return;
            }
        }
    };

    crate::diagnostics::stderr_line(&format!(
        "[orchestration-loop] Started for run {}",
        run_id
    ));

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let result = run_single_cycle(
            &app,
            &runtime,
            &agent_store,
            &pty_state,
            &browser_manager,
            &run_id,
        )
        .await;

        match result {
            Ok(ref cycle_result) => {
                // Emit event to frontend so UI updates reactively
                let _ = app.emit("openflow-cycle", cycle_result);
            }
            Err(ref e) => {
                crate::diagnostics::stderr_line(&format!(
                    "[orchestration-loop] cycle error for {}: {}",
                    run_id, e
                ));
            }
        }

        // Determine sleep duration based on current run phase
        let sleep_duration = {
            let phase = runtime.get_run_phase(&run_id).unwrap_or_default();
            match phase.as_str() {
                "complete" | "blocked" | "awaiting_approval" => {
                    std::time::Duration::from_secs(15)
                }
                _ => std::time::Duration::from_secs(5),
            }
        };

        // Sleep with wake-on-injection capability
        tokio::select! {
            _ = tokio::time::sleep(sleep_duration) => {},
            _ = wake.notified() => {
                // Woken by user injection or explicit trigger — run cycle immediately
            },
        }
    }

    crate::diagnostics::stderr_line(&format!(
        "[orchestration-loop] Stopped for run {}",
        run_id
    ));
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
    let record = store.create_run(request);
    crate::diagnostics::openflow_breadcrumb(&format!("run_created run_id={}", record.run_id));
    Ok(record)
}

#[tauri::command]
pub fn retry_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.retry_run(&run_id)
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

    crate::diagnostics::openflow_breadcrumb(&format!(
        "run_stopped run_id={} status={:?} reason={}",
        run_id, status, reason
    ));
    // Stop the background orchestration loop before teardown
    store.stop_loop(&run_id);
    let record = store.stop_run(&run_id, status, reason)?;

    if matches!(record.status, OpenFlowRunStatus::AwaitingApproval) {
        crate::state::emit_app_state(&app);
        return Ok(record);
    }

    let agent_sessions = agent_store.for_run(&run_id);
    let session_ids: Vec<String> = agent_sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect();
    let openflow_workspace_id = session_ids
        .first()
        .and_then(|session_id| app_state.workspace_id_for_session(session_id));

    {
        let mut sessions_guard = terminal_state
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for session_id in &session_ids {
            sessions_guard.remove(session_id);
        }
    }
    app_state.remove_terminal_sessions(&session_ids);

    if let Some(workspace_id) = openflow_workspace_id {
        let _ = app_state.close_workspace(&workspace_id.0);
    }

    let _ = store.remove_run(&run_id);
    agent_store.remove_for_run(&run_id);
    crate::terminal::release_comm_log_lock(
        Orchestrator::comm_log_path(&run_id)
            .to_string_lossy()
            .as_ref(),
    );

    crate::state::emit_app_state(&app);
    Ok(record)
}
