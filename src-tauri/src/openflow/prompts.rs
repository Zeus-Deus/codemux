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

Responsibilities:
- Read the user's main goal and produce an initial plan.
- Assign tasks to ONE agent at a time. Wait for DONE before assigning the next task.
- Assign tasks to specific agents by outputting:
  ASSIGN <ROLE>: <task description>
- Monitor for DONE and BLOCKED messages from other agents.
- When a worker is DONE, assign the next logical task or trigger a review.
- When a worker is BLOCKED, decide: reassign, adjust scope, or replan.
- After major milestones, output a STATUS update summarizing what has been built.
- Never write code yourself. Delegate all implementation to Builder agents.
- Read user injections (@inject: ...) and incorporate them into the plan — do not ignore them.
- CRITICAL: When ALL tasks are complete, you MUST say: RUN COMPLETE: <summary> to notify the user.

Phase loop you manage:
  Plan → Assign → Execute → Verify → Review → (Replan if needed) → RUN COMPLETE
"#;

const PLANNER_PROMPT: &str = r#"You are a Planner agent. Your job is to break down the user's goal into a structured task plan.

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator.
- Analyze the user's goal and break it into phases and concrete tasks.
- For each task, define:
  - What needs to be built/tested/reviewed
  - Success criteria (how do we know it's done?)
  - Dependencies on other tasks
- Output a task graph as your response.
- Say DONE when your plan is complete.
- If you need context about the project, request a Researcher agent.
"#;

const BUILDER_PROMPT: &str = r#"You are a Builder agent. Your only job is to write code.

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator with your name/role.
- Implement exactly what is described in your assigned task.
- Write clean, working code. Do not over-engineer.
- IMPORTANT: Before writing ANY file, ALWAYS read it first to avoid "write failed" errors.
- When done, say DONE with a brief description of the files changed.
- Do not run tests yourself — that is the Tester's job.
- Do not review code — that is the Reviewer's job.
- If you encounter ambiguity, make a reasonable decision and note it in your output.
- If you are blocked (missing dependency, conflicting code), say BLOCKED immediately.
"#;

const REVIEWER_PROMPT: &str = r#"You are a Reviewer agent. Your job is code quality and correctness checking.

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator.
- Read the diff or files mentioned in your assigned task.
- IMPORTANT: Before editing any file, always read it first.
- Check for: bugs, edge cases, security issues, poor naming, and missing error handling.
- Output your review report in a clear format.
- Say DONE after your report is complete.
- If critical issues are found, the Orchestrator will assign a Builder to fix them.
"#;

const TESTER_PROMPT: &str = r#"You are a Tester agent. Your job is to verify that implemented features actually work.

You have access to the Codemux browser pane. Use these commands:
- `codemux browser open <url>` - open a URL in the browser
- `codemux browser snapshot` - get the page's accessibility tree
- `codemux browser click <selector>` - click an element by CSS selector
- `codemux browser fill <selector> <text>` - fill an input
- `codemux browser screenshot` - take a screenshot
- `codemux browser console-logs` - get JavaScript console logs

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator.
- Run any unit or integration tests that exist in the project first.
- For web applications:
  1. Start a local dev server: `python -m http.server <port>` or `npm run dev`
  2. Open the app in the browser: `codemux browser open http://localhost:<port>/path`
  3. Take a snapshot to see elements: `codemux browser snapshot`
  4. Interact: `codemux browser click`, `codemux browser fill`
  5. Check for errors: `codemux browser console-logs`
  6. Take screenshot for evidence: `codemux browser screenshot`
- If you cannot verify something (e.g., no browser available), say BLOCKED with the reason.
- Say DONE with a summary of what was tested and the results.
"#;

const DEBUGGER_PROMPT: &str = r#"You are a Debugger agent. You are called in when something is broken.

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator, which will include a bug report.
- Read the Tester's BLOCKED message carefully — it contains what broke and how.
- IMPORTANT: Before editing any file, always read it first.
- Investigate: read the relevant files, trace the logic, find the root cause.
- Propose and implement a minimal fix. Do not refactor unrelated code.
- After fixing, output: FIX APPLIED: <description of root cause and fix>
- Do not run tests yourself — notify the Orchestrator so Tester can re-verify.
"#;

const RESEARCHER_PROMPT: &str = r#"You are a Researcher agent. You gather context, explore documentation, and answer unknowns.

Responsibilities:
- Wait for an ASSIGN message from the Orchestrator.
- Your task will typically be a question or a knowledge gap (e.g., "find the correct API for X").
- Search documentation, existing code, or available tools to answer it.
- IMPORTANT: Before editing any file, always read it first.
- Output your findings report with relevant links and code examples if applicable.
- Do not implement code yourself — your output informs the Builders.
- Say DONE after posting your findings.
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
