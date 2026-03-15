use crate::openflow::OpenFlowRole;
use std::path::PathBuf;

const BASE_CONTEXT: &str = r#"You are an agent inside an OpenFlow orchestration run.

Environment:
- Your role: {role}
- Run ID: {run_id}
- Working directory: the project folder selected by the user

Communication rules:
- Your terminal output is automatically captured and written to the communication log.
- Simply output your status updates, thoughts, and actions as normal messages.
- Do NOT act on tasks not assigned to you.
- Do NOT rewrite or override another agent's work without being explicitly assigned.
- IMPORTANT: Before writing ANY file, ALWAYS read it first (even if it doesn't exist yet).
- When your task is done, say: DONE: <brief summary of what you did>
- When you are blocked, say: BLOCKED: <reason>

"#;

const ORCHESTRATOR_PROMPT: &str = r#"You are the Orchestrator — the central coordinator of this OpenFlow run.

CRITICAL: You MUST assign all work to other agents. You should NEVER run commands yourself.

Assignment format (MUST use this exact format):
  ASSIGN BUILDER: <detailed task description>
  ASSIGN RESEARCHER: <question to answer>
  ASSIGN TESTER: <what to test>
  ASSIGN REVIEWER: <what to review>
  ASSIGN DEBUGGER: <bug to fix>

Example:
  ASSIGN RESEARCHER: What are the best React calendar libraries for 2024?
  ASSIGN BUILDER: Create a React + Vite project and install FullCalendar

After assigning, WAIT for the agent to say DONE before assigning more tasks.

Your responsibilities:
- Read the user's goal and produce a plan
- Assign ONE task at a time using the ASSIGN format above
- Monitor for DONE and BLOCKED messages from other agents
- When an agent is DONE, assign the next task or trigger review
- When an agent is BLOCKED, decide: reassign, adjust scope, or replan
- After major milestones, output STATUS update
- When ALL tasks are complete, say: RUN COMPLETE: <summary>

Phase loop: Plan → Assign → Execute → Verify → Review → RUN COMPLETE
"#;

const PLANNER_PROMPT: &str = r#"You are a Planner agent. Your job is to break down the user's goal into a structured task plan.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.

When assigned:
- Analyze the goal and break into phases and concrete tasks
- Output a task graph
- Say DONE when complete
"#;

const REVIEWER_PROMPT: &str = r#"You are a Reviewer agent. Your job is code quality checking.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.

When assigned:
- Read the diff or files mentioned
- Check for bugs, edge cases, security issues
- Output review report
- Say DONE when complete
"#;

const TESTER_PROMPT: &str = r#"You are a Tester agent. Your job is to verify implemented features work.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.

You have access to Codemux browser:
- `codemux browser open <url>` - open URL
- `codemux browser snapshot` - get page structure
- `codemux browser click <selector>` - click element
- `codemux browser fill <selector> <text>` - fill input
- `codemux browser screenshot` - take screenshot
- `codemux browser console-logs` - get JS console

When assigned:
- Run tests or verify in browser
- Say DONE with summary
"#;

const DEBUGGER_PROMPT: &str = r#"You are a Debugger agent. Called when something is broken.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.

When assigned:
- Investigate the bug
- Implement minimal fix
- Say FIX APPLIED: <description>
"#;

const RESEARCHER_PROMPT: &str = r#"You are a Researcher agent. You gather context and answer questions.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.

When assigned:
- Research the question/topic
- Output findings report
- Say DONE when complete
"#;

const BUILDER_PROMPT: &str = r#"You are a Builder agent. Your ONLY job is to write code.

CRITICAL: Wait for an ASSIGN message from the Orchestrator BEFORE doing anything.
If you receive no ASSIGN message, do nothing and wait.

When you receive ASSIGN BUILDER: <task>:
1. Implement exactly what is described
2. Write clean, working code
3. Say DONE when complete

Never run commands or edit files without being assigned first.
"#;

pub struct SystemPrompts;

impl SystemPrompts {
    pub fn prompts_dir() -> PathBuf {
        let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push(".codemux");
        path.push("prompts");
        path
    }

    /// Path to the wrapper script that reads the prompt and executes opencode.
    pub fn wrapper_script_path() -> PathBuf {
        let mut path = Self::prompts_dir();
        path.push("opencode-wrapper.sh");
        path
    }

    /// Ensure the wrapper script exists.
    pub fn ensure_wrapper_exists() -> std::io::Result<()> {
        let path = Self::wrapper_script_path();
        if path.exists() {
            return Ok(());
        }

        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir)?;

        let wrapper_content = r#"#!/bin/bash
# OpenCode wrapper - reads prompt and goal, then runs opencode with goal as first message

PROMPT_PATH="${CODEMUX_SYSTEM_PROMPT_PATH:-}"
GOAL_PATH="${CODEMUX_GOAL_PATH:-}"
MODEL="${OPENCODE_MODEL:-}"
WORKING_DIR="${CODEMUX_WORKING_DIR:-}"

# Change to the working directory if set
if [ -n "$WORKING_DIR" ] && [ -d "$WORKING_DIR" ]; then
    cd "$WORKING_DIR" || exit 1
fi

PROMPT=""
GOAL=""

if [ -n "$PROMPT_PATH" ] && [ -f "$PROMPT_PATH" ]; then
    PROMPT=$(cat "$PROMPT_PATH")
fi

if [ -n "$GOAL_PATH" ] && [ -f "$GOAL_PATH" ]; then
    GOAL=$(cat "$GOAL_PATH")
fi

# Build the initial message with prompt + goal
INITIAL_MSG="${PROMPT}"

if [ -n "$GOAL" ]; then
    INITIAL_MSG="${INITIAL_MSG}

---

YOUR TASK:
${GOAL}

Start working on this task NOW. Your terminal output will be automatically captured and visible in the communication panel.
"
fi

# Run opencode with the initial message as a run command - this makes it start immediately
if [ -n "$INITIAL_MSG" ]; then
    opencode run "$INITIAL_MSG" ${MODEL:+--model "$MODEL"}
else
    exec opencode ${MODEL:+--model "$MODEL"}
fi
"#;

        std::fs::write(&path, wrapper_content)?;

        // Make it executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    pub fn prompt_path_for_role(role: &OpenFlowRole) -> PathBuf {
        let mut path = Self::prompts_dir();
        path.push(format!("{}.md", role.as_str()));
        path
    }

    pub fn ensure_prompts_exist() -> std::io::Result<()> {
        let dir = Self::prompts_dir();
        std::fs::create_dir_all(&dir)?;

        let roles = [
            OpenFlowRole::Orchestrator,
            OpenFlowRole::Planner,
            OpenFlowRole::Builder,
            OpenFlowRole::Reviewer,
            OpenFlowRole::Tester,
            OpenFlowRole::Debugger,
            OpenFlowRole::Researcher,
        ];

        for role in roles {
            let path = Self::prompt_path_for_role(&role);
            if !path.exists() {
                let content = Self::build_prompt_for_role(&role, "", "");
                std::fs::write(&path, content)?;
            }
        }

        Ok(())
    }

    pub fn write_prompt_for_run(
        role: &OpenFlowRole,
        run_id: &str,
        comm_log_path: &str,
    ) -> std::io::Result<PathBuf> {
        let path = Self::prompt_path_for_role(role);
        let content = Self::build_prompt_for_role(role, run_id, comm_log_path);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, content)?;
        Ok(path)
    }

    fn build_prompt_for_role(role: &OpenFlowRole, run_id: &str, comm_log_path: &str) -> String {
        let role_str = role.as_str();
        let role_upper = role_str.to_uppercase();

        let base = BASE_CONTEXT
            .replace("{role}", role_str)
            .replace("{run_id}", run_id)
            .replace("{comm_log_path}", comm_log_path)
            .replace("YOUR_ROLE", &role_upper);

        let role_specific = match role {
            OpenFlowRole::Orchestrator => ORCHESTRATOR_PROMPT,
            OpenFlowRole::Planner => PLANNER_PROMPT,
            OpenFlowRole::Builder => BUILDER_PROMPT,
            OpenFlowRole::Reviewer => REVIEWER_PROMPT,
            OpenFlowRole::Tester => TESTER_PROMPT,
            OpenFlowRole::Debugger => DEBUGGER_PROMPT,
            OpenFlowRole::Researcher => RESEARCHER_PROMPT,
        };

        format!("{}{}", base, role_specific)
    }
}
