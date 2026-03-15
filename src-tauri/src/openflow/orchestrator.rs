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
        let mut user_injections = Vec::new();

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
            } else if role_lower.contains("assign") {
                assignments.push(entry.message.clone());
            } else if role_lower.contains("status") || role_lower.contains("phase") {
                status_updates.push(entry.message.clone());
            } else if role_lower.contains("user/inject") || entry.message.starts_with("@instruct") {
                user_injections.push(entry.message.clone());
            }
        }

        OrchestratorAnalysis {
            completed_roles: completed,
            blocked_roles: blocked,
            assignments,
            status_updates,
            user_injections,
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

        Ok(())
    }

    pub fn determine_next_phase(
        current_phase: &OrchestratorPhase,
        analysis: &OrchestratorAnalysis,
    ) -> Option<OrchestratorPhase> {
        match current_phase {
            OrchestratorPhase::Planning => {
                if analysis.assignments.is_empty() {
                    None
                } else {
                    Some(OrchestratorPhase::Executing)
                }
            }
            OrchestratorPhase::Executing => {
                if !analysis.blocked_roles.is_empty() {
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.completed_roles.contains(&OpenFlowRole::Builder) {
                    Some(OrchestratorPhase::Verifying)
                } else {
                    None
                }
            }
            OrchestratorPhase::Verifying => {
                if !analysis.blocked_roles.is_empty() {
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.completed_roles.contains(&OpenFlowRole::Tester)
                    || analysis.completed_roles.contains(&OpenFlowRole::Reviewer)
                {
                    Some(OrchestratorPhase::Reviewing)
                } else {
                    None
                }
            }
            OrchestratorPhase::Reviewing => Some(OrchestratorPhase::WaitingApproval),
            OrchestratorPhase::WaitingApproval => None,
            OrchestratorPhase::Replanning => Some(OrchestratorPhase::Planning),
            OrchestratorPhase::Completed | OrchestratorPhase::Assigning => None,
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
    pub user_injections: Vec<String>,
}
