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
    Blocked,
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
            "blocked" => Self::Blocked,
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
            Self::Blocked => "blocked",
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
    pub fn run_dir(run_id: &str) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codemux")
            .join("runs")
            .join(run_id)
    }

    pub fn comm_log_path(run_id: &str) -> PathBuf {
        Self::run_dir(run_id).join("communication.log")
    }

    pub fn goal_path(run_id: &str) -> PathBuf {
        Self::run_dir(run_id).join("goal.txt")
    }

    pub fn app_url_path(run_id: &str) -> PathBuf {
        Self::run_dir(run_id).join("app_url.txt")
    }

    pub fn read_app_url(run_id: &str) -> Option<String> {
        std::fs::read_to_string(Self::app_url_path(run_id))
            .ok()
            .map(|content| content.trim().to_string())
            .filter(|value| !value.is_empty())
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

    /// Read only new entries since last read - for incremental processing.
    /// Returns (entries, file_offset) where file_offset can be passed to next call.
    pub fn read_communication_log_incremental(
        run_id: &str,
        last_offset: usize,
    ) -> std::io::Result<(Vec<CommLogEntry>, usize)> {
        let path = Self::comm_log_path(run_id);
        if !path.exists() {
            return Ok((vec![], 0));
        }

        let metadata = std::fs::metadata(&path)?;
        let current_size = metadata.len() as usize;

        // If offset is past end of file, file was likely rotated - reset to beginning
        let effective_offset = if last_offset > current_size {
            0
        } else {
            last_offset
        };

        // No new content since last read
        if effective_offset >= current_size {
            return Ok((vec![], current_size));
        }

        // Read only new content from last_offset to end
        let mut file = std::fs::File::open(&path)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(effective_offset as u64))?;

        let mut new_content = String::new();
        std::io::Read::read_to_string(&mut file, &mut new_content)?;

        let entries: Vec<CommLogEntry> = new_content
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| Self::parse_log_line(line))
            .collect();

        Ok((entries, current_size))
    }

    pub fn parse_log_line(line: &str) -> Option<CommLogEntry> {
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

    pub fn parse_timestamp(timestamp_str: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_str(timestamp_str, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }

    /// Parse an "ASSIGN ROLE-N: task" line and extract the instance id + task.
    /// Accepts both "ASSIGN BUILDER-0: task" and legacy "ASSIGN BUILDER: task".
    /// Returns None if the message is not an ASSIGN directive.
    pub fn parse_assign_message(msg: &str) -> Option<InstanceAssignment> {
        // Find the ASSIGN keyword (case-insensitive)
        let upper = msg.to_uppercase();
        let assign_pos = upper.find("ASSIGN ")?;
        let rest = &msg[assign_pos + 7..].trim_start();

        // Split on ':' to get "BUILDER-0" and "task description"
        let colon = rest.find(':')?;
        let raw_target = rest[..colon].trim().to_string();
        let task = rest[colon + 1..].trim().to_string();

        if raw_target.is_empty() || task.is_empty() {
            return None;
        }

        Some(InstanceAssignment {
            instance_id: raw_target.to_lowercase(),
            task,
        })
    }

    pub fn analyze_comm_log(entries: &[CommLogEntry]) -> OrchestratorAnalysis {
        let mut completed = Vec::new();
        let mut completed_instances: Vec<String> = Vec::new();
        let mut blocked = Vec::new();
        let mut blocked_instances: Vec<String> = Vec::new();
        let mut assignments = Vec::new();
        let mut all_instance_assignments: Vec<InstanceAssignment> = Vec::new();
        let mut status_updates = Vec::new();
        let mut all_injections = Vec::new();
        let mut last_handled_count: usize = 0;
        let mut last_handled_assignments: usize = 0;
        let mut last_pending_injections: usize = 0;
        let mut last_pending_index: Option<usize> = None;

        for (index, entry) in entries.iter().enumerate() {
            let role_lower = entry.role.to_lowercase();

            if entry.message.contains("DONE:") {
                // Track both the bare role (for phase transitions) and the full instance id
                if let Some(role) = OpenFlowRole::from_str(Self::base_role(&role_lower)) {
                    completed.push(role);
                }
                completed_instances.push(role_lower.clone());
            } else if entry.message.contains("BLOCKED:") {
                if let Some(role) = OpenFlowRole::from_str(Self::base_role(&role_lower)) {
                    blocked.push(role);
                }
                blocked_instances.push(role_lower.clone());
            } else if entry.message.to_lowercase().contains("assign ")
                || entry.message.to_lowercase().contains("assign:")
            {
                assignments.push(entry.message.clone());
                // Also parse as an instance-level assignment when it originates from orchestrator
                if role_lower == "orchestrator" {
                    if let Some(ia) = Self::parse_assign_message(&entry.message) {
                        all_instance_assignments.push(ia);
                    }
                }
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
                // Track the highest HANDLED_ASSIGNMENTS marker seen so far
                if let Some(rest) = entry.message.strip_prefix("HANDLED_ASSIGNMENTS: ") {
                    if let Ok(n) = rest.trim().parse::<usize>() {
                        if n > last_handled_assignments {
                            last_handled_assignments = n;
                        }
                    }
                }
                if let Some(rest) = entry.message.strip_prefix("INJECTION_PENDING: ") {
                    if let Ok(n) = rest.trim().parse::<usize>() {
                        if n >= last_pending_injections {
                            last_pending_injections = n;
                            last_pending_index = Some(index);
                        }
                    }
                }
            }
        }

        let orchestrator_responded_to_pending = last_pending_index
            .map(|index| {
                entries
                    .iter()
                    .skip(index + 1)
                    .any(|entry| entry.role.eq_ignore_ascii_case("orchestrator"))
            })
            .unwrap_or(false);

        // Only return injections that have not yet been handled
        let unhandled_injections = if last_handled_count < all_injections.len() {
            all_injections[last_handled_count..].to_vec()
        } else {
            vec![]
        };

        let forwarded_injections = last_handled_count.max(last_pending_injections);
        let injections_to_forward = if forwarded_injections < all_injections.len() {
            all_injections[forwarded_injections..].to_vec()
        } else {
            vec![]
        };

        // Only return instance assignments that have not yet been forwarded
        let unforwarded_assignments = if last_handled_assignments < all_instance_assignments.len() {
            all_instance_assignments[last_handled_assignments..].to_vec()
        } else {
            vec![]
        };

        OrchestratorAnalysis {
            completed_roles: completed,
            completed_instances,
            blocked_roles: blocked,
            blocked_instances,
            assignments,
            instance_assignments: unforwarded_assignments,
            status_updates,
            user_injections: unhandled_injections,
            injections_to_forward,
            total_injections: all_injections.len(),
            last_handled_assignments,
            last_pending_injections,
            last_handled_injections: last_handled_count,
            orchestrator_responded_to_pending,
        }
    }

    /// Extract the base role name from an instance ID.
    /// "builder-0" → "builder", "orchestrator" → "orchestrator".
    fn base_role(instance_id: &str) -> &str {
        if let Some(dash) = instance_id.rfind('-') {
            // Only strip suffix if it's all digits (i.e. it IS an instance index)
            if instance_id[dash + 1..].chars().all(|c| c.is_ascii_digit()) {
                return &instance_id[..dash];
            }
        }
        instance_id
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
        let path_str = path.to_string_lossy().to_string();
        // Use the same file lock used by agent PTY writers so that all
        // communication log writes are serialized through a single mutex.
        let log_lock = crate::terminal::get_comm_log_lock(&path_str);
        {
            use std::io::Write;
            let mut file = log_lock.lock().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "Comm log lock poisoned")
            })?;
            file.write_all(entry.as_bytes())?;
            file.flush()?;
        }

        // Rotate after writing to keep the file bounded.  We do a best-effort
        // rotation — if it fails we silently continue rather than failing the write.
        // Increased from 500 to 5000 for better scalability with 20+ agents.
        let _ = Self::rotate_comm_log_if_needed(&path, 5000);

        Ok(())
    }

    /// If the log has grown beyond `max_lines`, atomically rotate by writing to a temp file
    /// then renaming. This prevents data loss if readers have offsets into the original file.
    /// Keeps the newest half so the orchestrator retains recent context.
    fn rotate_comm_log_if_needed(path: &std::path::Path, max_lines: usize) -> std::io::Result<()> {
        let content = std::fs::read_to_string(path)?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        if lines.len() <= max_lines {
            return Ok(());
        }

        let keep_from = lines.len() - (max_lines / 2);
        let header_lines: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| {
                line.contains("[SYSTEM] GOAL:")
                    || line.contains("[SYSTEM] APP_URL:")
                    || line.contains("[SYSTEM] AGENTS:")
                    || line.contains("[SYSTEM] Started agent:")
            })
            .collect();

        let mut kept_lines: Vec<&str> = Vec::with_capacity(header_lines.len() + (max_lines / 2));
        for line in header_lines {
            if !kept_lines.contains(&line) {
                kept_lines.push(line);
            }
        }
        for line in &lines[keep_from..] {
            if !kept_lines.contains(line) {
                kept_lines.push(*line);
            }
        }

        let trimmed = kept_lines.join("\n") + "\n";

        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, trimmed)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }

    /// `instance_counts`: number of running instances per role (excluding orchestrator).
    /// When a role has N instances, the phase only advances when all N have reported DONE.
    /// `consecutive_replans`: number of consecutive replanning cycles without new DONE markers.
    /// `previous_done_count`: DONE count when we entered the current Replanning cycle.
    pub fn determine_next_phase(
        current_phase: &OrchestratorPhase,
        analysis: &OrchestratorAnalysis,
        instance_counts: &HashMap<String, usize>,
        consecutive_replans: u32,
        previous_done_count: usize,
    ) -> Option<OrchestratorPhase> {
        let run_complete = analysis
            .status_updates
            .iter()
            .any(|s| s.to_lowercase().contains("run complete"));

        // User injections always take priority in any active phase.
        let has_unhandled_injection = !analysis.user_injections.is_empty();

        // Returns true when all (or at least one if count = 0) instances of a role are done.
        let all_done = |role: &OpenFlowRole| -> bool {
            let count = instance_counts.get(role.as_str()).copied().unwrap_or(0);
            if count == 0 {
                // Legacy: just check if any completion of this role exists
                return analysis.completed_roles.contains(role);
            }
            // Count how many distinct instances of this role reported DONE
            let role_str = role.as_str();
            let completed_count = analysis
                .completed_instances
                .iter()
                .filter(|id| {
                    let lower = id.to_lowercase();
                    // matches "builder-0", "builder-1", or bare "builder"
                    lower == role_str || lower.starts_with(&format!("{}-", role_str))
                })
                .count();
            completed_count >= count
        };

        match current_phase {
            OrchestratorPhase::Planning => {
                if run_complete {
                    Some(OrchestratorPhase::Completed)
                } else if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else if analysis.assignments.is_empty() {
                    if analysis.total_injections > 0
                        && analysis.total_injections == analysis.last_handled_injections
                    {
                        Some(OrchestratorPhase::Completed)
                    } else {
                        None
                    }
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
                } else if all_done(&OpenFlowRole::Builder) {
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
                } else if all_done(&OpenFlowRole::Tester) || all_done(&OpenFlowRole::Reviewer) {
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
                    Some(OrchestratorPhase::Completed)
                }
            }
            OrchestratorPhase::Replanning => {
                let current_done_count = analysis.completed_instances.len();
                let made_progress = current_done_count > previous_done_count;
                const MAX_CONSECUTIVE_REPLANS: u32 = 3;
                if !made_progress && consecutive_replans >= MAX_CONSECUTIVE_REPLANS {
                    Some(OrchestratorPhase::Blocked)
                } else {
                    Some(OrchestratorPhase::Planning)
                }
            }
            OrchestratorPhase::Completed => {
                if has_unhandled_injection {
                    Some(OrchestratorPhase::Replanning)
                } else {
                    None
                }
            }
            OrchestratorPhase::Blocked => None,
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

/// A parsed ASSIGN directive targeting a specific agent instance.
#[derive(Debug, Clone)]
pub struct InstanceAssignment {
    /// Target instance ID, e.g. `"builder-0"`.
    pub instance_id: String,
    /// The task description to forward to that agent's PTY.
    pub task: String,
}

#[derive(Debug, Clone)]
pub struct OrchestratorAnalysis {
    pub completed_roles: Vec<OpenFlowRole>,
    /// Per-instance completions: `"builder-0"`, `"tester-1"`, etc.
    pub completed_instances: Vec<String>,
    pub blocked_roles: Vec<OpenFlowRole>,
    /// Per-instance blocks.
    pub blocked_instances: Vec<String>,
    pub assignments: Vec<String>,
    /// Parsed instance-level assignments from the orchestrator (e.g. "ASSIGN BUILDER-0: task").
    pub instance_assignments: Vec<InstanceAssignment>,
    pub status_updates: Vec<String>,
    /// Injections that have NOT yet been handled (i.e. count > last HANDLED_INJECTIONS marker).
    pub user_injections: Vec<String>,
    /// New injections that have not yet been forwarded to the orchestrator PTY.
    pub injections_to_forward: Vec<String>,
    /// Total number of injections ever written to the log (including already-handled ones).
    pub total_injections: usize,
    /// Number of instance assignments already forwarded (for HANDLED_ASSIGNMENTS marker).
    pub last_handled_assignments: usize,
    /// Number of injections currently forwarded to the orchestrator but not yet marked handled.
    pub last_pending_injections: usize,
    /// Number of injections already handled (for HANDLED_INJECTIONS marker).
    pub last_handled_injections: usize,
    /// True when the orchestrator has written at least one log line after the latest pending marker.
    pub orchestrator_responded_to_pending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: &str, message: &str) -> CommLogEntry {
        CommLogEntry {
            timestamp: "2026-03-18 00:00:00".to_string(),
            role: role.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn analyze_comm_log_only_returns_unhandled_injections_after_marker() {
        let entries = vec![
            entry("user/inject", "first question"),
            entry("system", "HANDLED_INJECTIONS: 1"),
            entry("user/inject", "second question"),
        ];

        let analysis = Orchestrator::analyze_comm_log(&entries);

        assert_eq!(analysis.total_injections, 2);
        assert_eq!(analysis.last_handled_injections, 1);
        assert_eq!(
            analysis.user_injections,
            vec!["second question".to_string()]
        );
    }

    #[test]
    fn analyze_comm_log_tracks_pending_injections_until_orchestrator_replies() {
        let entries = vec![
            entry("user/inject", "is it live?"),
            entry("system", "INJECTION_PENDING: 1"),
            entry("orchestrator", "STATUS: checking the app URL now"),
        ];

        let analysis = Orchestrator::analyze_comm_log(&entries);

        assert_eq!(analysis.user_injections, vec!["is it live?".to_string()]);
        assert!(analysis.injections_to_forward.is_empty());
        assert_eq!(analysis.last_pending_injections, 1);
        assert!(analysis.orchestrator_responded_to_pending);
    }

    #[test]
    fn completed_phase_replans_when_new_user_message_arrives() {
        let analysis = OrchestratorAnalysis {
            completed_roles: vec![],
            completed_instances: vec![],
            blocked_roles: vec![],
            blocked_instances: vec![],
            assignments: vec![],
            instance_assignments: vec![],
            status_updates: vec![],
            user_injections: vec!["can you keep going?".to_string()],
            injections_to_forward: vec!["can you keep going?".to_string()],
            total_injections: 1,
            last_handled_assignments: 0,
            last_pending_injections: 0,
            last_handled_injections: 0,
            orchestrator_responded_to_pending: false,
        };

        let next_phase = Orchestrator::determine_next_phase(
            &OrchestratorPhase::Completed,
            &analysis,
            &HashMap::new(),
            0,
            0,
        );

        assert_eq!(next_phase, Some(OrchestratorPhase::Replanning));
    }
}
