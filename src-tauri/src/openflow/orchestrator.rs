use crate::openflow::OpenFlowRole;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OrchestratorState {
    pub run_id: String,
    pub goal: String,
    pub current_phase: OrchestratorPhase,
    pub assigned_tasks: HashMap<String, TaskAssignment>,
    pub completed_agents: Vec<OpenFlowRole>,
    pub blocked_agents: Vec<OpenFlowRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorPhase {
    Planning,
    Assigning,
    Executing,
    Verifying,
    Reviewing,
    WaitingApproval,
    Completed,
    Replanning,
}

impl OrchestratorPhase {
    pub fn from_string(s: &str) -> Self {
        match s {
            "plan" => Self::Planning,
            "assign" => Self::Assigning,
            "execute" => Self::Executing,
            "verify" => Self::Verifying,
            "review" => Self::Reviewing,
            "awaiting_approval" | "approval" => Self::WaitingApproval,
            "complete" | "completed" => Self::Completed,
            "replan" => Self::Replanning,
            _ => Self::Planning,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planning => "plan",
            Self::Assigning => "assign",
            Self::Executing => "execute",
            Self::Verifying => "verify",
            Self::Reviewing => "review",
            Self::WaitingApproval => "awaiting_approval",
            Self::Completed => "complete",
            Self::Replanning => "replan",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskAssignment {
    pub task_id: String,
    pub assigned_to: OpenFlowRole,
    pub description: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Assigned,
    InProgress,
    Done,
    Blocked,
}

pub struct Orchestrator;

impl Orchestrator {
    pub fn comm_log_path(run_id: &str) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codemux")
            .join("runs")
            .join(run_id)
            .join("communication.log")
    }

    pub fn read_communication_log(run_id: &str) -> std::io::Result<Vec<CommLogEntry>> {
        let path = Self::comm_log_path(run_id);
        if !path.exists() {
            return Ok(vec![]);
        }

        let content = std::fs::read_to_string(&path)?;
        let entries: Vec<CommLogEntry> = content
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| Self::parse_log_line(line))
            .collect();

        Ok(entries)
    }

    fn parse_log_line(line: &str) -> Option<CommLogEntry> {
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

    pub fn analyze_comm_log(entries: &[CommLogEntry]) -> OrchestratorAnalysis {
        let mut completed = Vec::new();
        let mut blocked = Vec::new();
        let mut assignments = Vec::new();
        let mut status_updates = Vec::new();
        let mut all_injections = Vec::new();
        let mut last_handled_count: usize = 0;

        for entry in entries {
            let role_lower = entry.role.to_lowercase();

            if entry.message.contains("DONE:") {
                if let Some(role) = OpenFlowRole::from_str(&role_lower) {
                    completed.push(role);
                }
            } else if entry.message.contains("BLOCKED:") {
                if let Some(role) = OpenFlowRole::from_str(&role_lower) {
                    blocked.push(role);
                }
            } else if entry.message.to_lowercase().contains("assign ")
                || entry.message.to_lowercase().contains("assign:")
            {
                assignments.push(entry.message.clone());
            } else if entry.message.to_lowercase().contains("run complete") {
                status_updates.push(entry.message.clone());
            } else if role_lower.contains("status") || role_lower.contains("phase") {
                status_updates.push(entry.message.clone());
            } else if role_lower.contains("user/inject") || entry.message.starts_with("@instruct") {
                all_injections.push(entry.message.clone());
            } else if role_lower == "system" {
                // Track the highest HANDLED_INJECTIONS marker seen so far
                if let Some(rest) = entry.message.strip_prefix("HANDLED_INJECTIONS: ") {
                    if let Ok(n) = rest.trim().parse::<usize>() {
                        if n > last_handled_count {
                            last_handled_count = n;
                        }
                    }
                }
            }
        }

        // Only return injections that have not yet been handled
        let unhandled_injections = if last_handled_count < all_injections.len() {
            all_injections[last_handled_count..].to_vec()
        } else {
            vec![]
        };

        OrchestratorAnalysis {
            completed_roles: completed,
            blocked_roles: blocked,
            assignments,
            status_updates,
            user_injections: unhandled_injections,
            total_injections: all_injections.len(),
        }
    }

    pub fn generate_assign_message(role: OpenFlowRole, task: &str) -> String {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        format!(
            "[{}] [ORCHESTRATOR] ASSIGN {}: {}",
            timestamp,
            role.as_str().to_uppercase(),
            task
        )
    }

    pub fn generate_status_message(summary: &str) -> String {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        format!("[{}] [ORCHESTRATOR] STATUS: {}", timestamp, summary)
    }

    pub fn generate_complete_message(summary: &str) -> String {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        format!("[{}] [ORCHESTRATOR] RUN COMPLETE: {}", timestamp, summary)
    }

    pub fn write_to_comm_log(run_id: &str, message: &str) -> std::io::Result<()> {
        let path = Self::comm_log_path(run_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let entry = format!("{}\n", message);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, entry.as_bytes()))?;

        // Rotate after writing to keep the file bounded.  We do a best-effort
        // rotation — if it fails we silently continue rather than failing the write.
        let _ = Self::rotate_comm_log_if_needed(&path, 500);

        Ok(())
    }

    /// If the log has grown beyond `max_lines`, drop the oldest half and rewrite the file.
    /// This keeps recent context intact while preventing unbounded memory allocation when
    /// the full file is read on every orchestration cycle.
    fn rotate_comm_log_if_needed(path: &std::path::Path, max_lines: usize) -> std::io::Result<()> {
        let content = std::fs::read_to_string(path)?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        if lines.len() <= max_lines {
            return Ok(());
        }
        // Keep the newest half so the orchestrator retains recent context.
        let keep_from = lines.len() - (max_lines / 2);
        let trimmed = lines[keep_from..].join("\n") + "\n";
        std::fs::write(path, trimmed)?;
        Ok(())
    }

    pub fn determine_next_phase(
        current_phase: &OrchestratorPhase,
        analysis: &OrchestratorAnalysis,
    ) -> Option<OrchestratorPhase> {
        let run_complete = analysis
            .status_updates
            .iter()
            .any(|s| s.to_lowercase().contains("run complete"));

        // User injections always take priority in any active phase.
        let has_unhandled_injection = !analysis.user_injections.is_empty();

        match current_phase {
            OrchestratorPhase::Planning => {
                if run_complete {
                    Some(OrchestratorPhase::Completed)
                } else if has_unhandled_injection {
                    // Re-enter planning so the orchestrator AI picks up the new context
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.assignments.is_empty() {
                    None
                } else {
                    Some(OrchestratorPhase::Executing)
                }
            }
            OrchestratorPhase::Executing => {
                if run_complete {
                    Some(OrchestratorPhase::Completed)
                } else if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else if !analysis.blocked_roles.is_empty() {
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.completed_roles.contains(&OpenFlowRole::Builder) {
                    Some(OrchestratorPhase::Verifying)
                } else {
                    None
                }
            }
            OrchestratorPhase::Verifying => {
                if run_complete {
                    Some(OrchestratorPhase::Completed)
                } else if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else if !analysis.blocked_roles.is_empty() {
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.completed_roles.contains(&OpenFlowRole::Tester)
                    || analysis.completed_roles.contains(&OpenFlowRole::Reviewer)
                {
                    Some(OrchestratorPhase::Reviewing)
                } else {
                    None
                }
            }
            OrchestratorPhase::Reviewing => {
                if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else {
                    Some(OrchestratorPhase::WaitingApproval)
                }
            }
            OrchestratorPhase::WaitingApproval => {
                if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else {
                    None
                }
            }
            OrchestratorPhase::Replanning => Some(OrchestratorPhase::Planning),
            OrchestratorPhase::Completed => {
                if has_unhandled_injection {
                    Some(OrchestratorPhase::Planning)
                } else {
                    None
                }
            }
            OrchestratorPhase::Assigning => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommLogEntry {
    pub timestamp: String,
    pub role: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct OrchestratorAnalysis {
    pub completed_roles: Vec<OpenFlowRole>,
    pub blocked_roles: Vec<OpenFlowRole>,
    pub assignments: Vec<String>,
    pub status_updates: Vec<String>,
    /// Injections that have NOT yet been handled (i.e. count > last HANDLED_INJECTIONS marker).
    pub user_injections: Vec<String>,
    /// Total number of injections ever written to the log (including already-handled ones).
    pub total_injections: usize,
}
