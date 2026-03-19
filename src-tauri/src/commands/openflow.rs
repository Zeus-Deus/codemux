use crate::browser::BrowserManager;
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
use tauri::{Manager, State};

const ACTIVE_STUCK_RESCUE_CYCLES: u32 = 2;
const PLANNING_STUCK_RESCUE_CYCLES: u32 = 3;

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

fn write_prompt_to_session(
    pty_state: &crate::terminal::PtyState,
    session_id: &str,
    prompt: &str,
) -> Result<(), String> {
    let escaped = prompt
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\'', "'\\''");
    let command = format!("opencode run \"{}\"\n", escaped);
    let mut sessions = pty_state.sessions.lock().unwrap();
    if let Some(pty_runtime) = sessions.get_mut(session_id) {
        if let Some(writer) = pty_runtime.writer.as_mut() {
            use std::io::Write;
            writer
                .write_all(command.as_bytes())
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
            id: "claude-opus-4-5".into(),
            name: "claude-opus-4-5".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "claude-sonnet-4-5".into(),
            name: "claude-sonnet-4-5".into(),
            provider: Some("anthropic".into()),
        },
        ModelInfo {
            id: "claude-haiku-3-5".into(),
            name: "claude-haiku-3-5".into(),
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
        .map_err(|error| format!("Failed to create wrapper script: {error}"))?;

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

    let entries = new_content
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(Orchestrator::parse_log_line)
        .map(|entry| CommLogEntry {
            timestamp: entry.timestamp,
            role: entry.role,
            message: entry.message,
        })
        .collect();

    Ok((entries, current_size))
}

#[tauri::command]
pub fn inject_orchestrator_message(run_id: String, message: String) -> Result<(), String> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{}] [user/inject] {}", timestamp, message);
    Orchestrator::write_to_comm_log(&run_id, &entry)
        .map_err(|error| format!("Failed to write to comm log: {error}"))
}

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
    let current_phase_str = runtime.get_run_phase(&run_id)?;
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
                    let msg = if e.message.len() > 100 {
                        format!("{}...", &e.message[..100])
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
                    let msg = if e.message.len() > 100 {
                        format!("{}...", &e.message[..100])
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

    let app_state = app.state::<AppStateStore>();

    let orchestrator_session = agent_store.for_run(&run_id).into_iter().find(|session| {
        matches!(
            session.config.role,
            crate::openflow::OpenFlowRole::Orchestrator
        )
    });

    if !analysis.instance_assignments.is_empty() {
        let all_sessions = agent_store.for_run(&run_id);
        let new_total_assignments =
            analysis.last_handled_assignments + analysis.instance_assignments.len();

        for assignment in &analysis.instance_assignments {
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
                let command = format!(
                    "opencode run \"{}\"\n",
                    assignment.task.replace('"', "\\\"")
                );
                let session_id = session.session_id.clone();
                let write_result = {
                    let mut sessions = pty_state.sessions.lock().unwrap();
                    if let Some(pty_runtime) = sessions.get_mut(&session_id) {
                        if let Some(writer) = pty_runtime.writer.as_mut() {
                            use std::io::Write;
                            writer
                                .write_all(command.as_bytes())
                                .and_then(|_| writer.flush())
                                .map_err(|error| format!("PTY write error: {error}"))
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

    let stuck_state_for_probe = runtime.get_stuck_state(&run_id);
    let invalid_delegation_detected = analysis.instance_assignments.is_empty()
        && has_recent_invalid_delegation_pattern(entries.as_slice());

    if invalid_delegation_detected && !stuck_state_for_probe.probe_injected {
        if let Some(ref orchestrator_session) = orchestrator_session {
            if ensure_agent_session_live(
                &app,
                &app_state,
                &agent_store,
                &pty_state,
                &run_id,
                orchestrator_session,
            )? {
                actions_taken.push("Respawned orchestrator session for delegation correction".to_string());
            }

            let correction_prompt = "Your last attempt used invalid internal delegation. Stop using General Agent / Task delegation. Coordinate only through literal standalone lines of the form `ASSIGN <INSTANCE-ID>: <task>` using the exact IDs from the AGENTS line. If you cannot delegate yet, write `STATUS:` or `BLOCKED:` with the exact reason. Do this now.";

            match write_prompt_to_session(&pty_state, &orchestrator_session.session_id, correction_prompt) {
                Ok(()) => {
                    runtime.mark_probe_sent(&run_id);
                    actions_taken.push("Sent delegation-correction prompt to orchestrator".to_string());
                }
                Err(error) => {
                    actions_taken.push(format!("Failed to send delegation-correction prompt: {}", error));
                }
            }
        }
    }

    if stuck_state_for_probe.consecutive_no_progress_cycles >= 2
        && !stuck_state_for_probe.probe_injected
    {
        if let Some(ref orchestrator_session) = orchestrator_session {
            if ensure_agent_session_live(
                &app,
                &app_state,
                &agent_store,
                &pty_state,
                &run_id,
                orchestrator_session,
            )? {
                actions_taken.push("Respawned orchestrator session for stuck-run nudge".to_string());
            }

            let probe_prompt = "OpenFlow appears stuck. Do one of these now:\n\
                 - emit literal standalone ASSIGN lines for the real OpenFlow agent instance IDs from the AGENTS line, or\n\
                 - write STATUS: with what you are waiting on, or\n\
                 - write BLOCKED: with the exact reason.\n\
                 Do NOT use internal General Agent or Task delegation. Keep this generic to the user's project type; only mention an app URL if the task actually involves a live preview.";

            match write_prompt_to_session(&pty_state, &orchestrator_session.session_id, &probe_prompt) {
                Ok(()) => {
                    runtime.mark_probe_sent(&run_id);
                    actions_taken.push("Sent stuck-run nudge to orchestrator".to_string());
                }
                Err(error) => {
                    actions_taken.push(format!("Failed to send stuck-run nudge: {}", error));
                }
            }
        }
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

                let goal_path = Orchestrator::goal_path(&run_id).display().to_string();
                let prompt = format!(
                    "A user message arrived. User message: \"{}\".\n\
                     Please address this message:\n\
                     - If it asks a question, answer it directly in the comm log.\n\
                    - If it requests changes, create ASSIGN messages to delegate work.\n\
                    - IMPORTANT: After answering or delegating, DO NOT exit. Wait for more user messages.\n\
                    Just respond to this message and wait. The goal file is at: {}",
                    injection_text,
                    goal_path
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

                let write_result = write_prompt_to_session(&pty_state, &session_id, &prompt);

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

        let (updated_stuck_state, made_progress) = runtime.update_stuck_state(
            &run_id,
            session_count,
            assignment_count,
            done_count,
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
            let all_sessions = agent_store.for_run(&run_id);
            let mut rescued_any = false;

            for session in all_sessions {
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

            if rescued_any {
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

    crate::diagnostics::openflow_breadcrumb(&format!(
        "run_stopped run_id={} status={:?} reason={}",
        run_id, status, reason
    ));
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
