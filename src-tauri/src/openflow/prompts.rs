use crate::openflow::OpenFlowRole;
use std::path::PathBuf;

const BASE_CONTEXT: &str = r#"You are an agent inside an OpenFlow orchestration run.

Environment:
- Your role: {role}
- Your instance ID: {instance_id}
- Run ID: {run_id}
- Working directory: the project folder selected by the user
- Assigned app URL for this run: {app_url}

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

Assignment format — always use the INSTANCE ID (role + index), NOT the bare role name:
  ASSIGN BUILDER-0: <detailed task description>
  ASSIGN BUILDER-1: <different task description>
  ASSIGN RESEARCHER-0: <question to answer>
  ASSIGN TESTER-0: <what to test>
  ASSIGN REVIEWER-0: <what to review>
  ASSIGN DEBUGGER-0: <bug to fix>
  ASSIGN PLANNER-0: <planning task>

PARALLEL EXECUTION — this is critical for speed:
- You will be told which agent instances are available (e.g. BUILDER-0, BUILDER-1, BUILDER-2).
- Use the exact instance IDs from the AGENTS line. Do NOT invent new IDs or renumber them.
- Example: if the AGENTS line says `BUILDER-3, BUILDER-9, BUILDER-15`, those are the only valid builder targets.
- Assign DIFFERENT tasks to ALL available instances of a role simultaneously.
- Do NOT wait for one builder to finish before assigning to another builder.
- Only wait for a specific instance (e.g. BUILDER-0) if you need ITS output before the next step.
- If you have 3 builders, give all 3 independent tasks at the same time.

Tooling constraints:
- Do NOT use internal General Agent / Task delegation for repo work.
- Do NOT describe assignments in prose.
- Do NOT use "echo" or any shell command to output ASSIGN lines.
- Write ASSIGN lines DIRECTLY to your output in the exact format:
  ASSIGN <INSTANCE-ID>: <task description>
  Example: ASSIGN BUILDER-0: Create a hello world file
  Example: ASSIGN TESTER-0: Write tests for the API
  Example: ASSIGN REVIEWER-0: Review the login implementation
- The ASSIGN line must appear as a standalone line in your response — NOT inside an echo, printf, or any other command.

Example of good parallel assignment:
  ASSIGN BUILDER-0: Create the backend API routes in src/routes/
  ASSIGN BUILDER-1: Create the database schema and migrations in src/db/
  ASSIGN BUILDER-2: Create the frontend React components in src/components/

Your responsibilities:
- Read the user's goal and produce a plan
- Identify which agent instances are available from the AGENTS line in the comm log and use those exact IDs
- Assign tasks in parallel to all available instances of each role
- Monitor for DONE <INSTANCE-ID>: and BLOCKED <INSTANCE-ID>: messages
- When an instance is DONE, assign it the next available task immediately
- When an instance is BLOCKED, decide: reassign, adjust scope, or replan
- When a live preview is needed, keep everyone on the assigned app URL from context: {app_url}
- After major milestones, output STATUS update
- When ALL tasks are complete, say: RUN COMPLETE: <summary>

Phase loop: Plan → Assign (in parallel) → Execute → Verify → Review → RUN COMPLETE
"#;

const PLANNER_PROMPT: &str = r#"You are a Planner agent. Your job is to break down the user's goal into a structured task plan.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.

When assigned:
- Analyze the goal and break into phases and concrete tasks
- Output a task graph
- Say DONE: <brief summary> when complete
- Say BLOCKED: <reason> if you cannot proceed
"#;

const REVIEWER_PROMPT: &str = r#"You are a Reviewer agent. Your job is code quality checking.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.

When assigned:
- Read the diff or files mentioned
- Check for bugs, edge cases, security issues
- Output review report
- Say DONE: <brief summary> when complete
- Say BLOCKED: <reason> if you cannot proceed
"#;

const TESTER_PROMPT: &str = r#"You are a Tester agent. Your job is to verify implemented features work.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.

You have access to Codemux browser:
- `codemux browser open <url>` - open URL
- `codemux browser snapshot` - get page structure
- `codemux browser click <selector>` - click element
- `codemux browser fill <selector> <text>` - fill input
- `codemux browser screenshot` - take screenshot
- `codemux browser console-logs` - get JS console

When assigned:
- Use the assigned app URL from context (`{app_url}`) unless the orchestrator gives an explicitly different approved URL
- Run tests or verify in browser
- Say DONE: <brief summary> when complete
- Say BLOCKED: <reason> if you cannot proceed
"#;

const DEBUGGER_PROMPT: &str = r#"You are a Debugger agent. Called when something is broken.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.

When assigned:
- Investigate the bug
- Implement minimal fix
- Say DONE: FIX APPLIED: <description> when complete
- Say BLOCKED: <reason> if you cannot proceed
"#;

const RESEARCHER_PROMPT: &str = r#"You are a Researcher agent. You gather context and answer questions.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.

When assigned:
- Research the question/topic
- Output findings report
- Say DONE: <brief summary> when complete
- Say BLOCKED: <reason> if you cannot proceed
"#;

const BUILDER_PROMPT: &str = r#"You are a Builder agent. Your ONLY job is to write code.
Your instance ID is {instance_id}. Always sign your DONE/BLOCKED messages with it.

CRITICAL: Wait for an ASSIGN {instance_id}: message from the Orchestrator BEFORE doing anything.
If you receive no ASSIGN message addressed to your instance ID, do nothing and wait.

When you receive ASSIGN {instance_id}: <task>:
1. Implement exactly what is described
2. Write clean, working code
3. Only start a dev server if your assignment actually requires a live preview or browser verification
4. If the task involves running a dev server or web app:
   a. Keep the preview on the assigned app URL from context: `{app_url}`
   b. Prefer a strict fixed-port startup command. For Vite, use a command like:
      - `setsid npm run dev -- --host 127.0.0.1 --port <assigned-port> --strictPort`
      - adapt the command for the framework, but keep the same assigned port
   c. If the assigned port is already busy because of an earlier process you started, reuse or replace that process; do NOT silently switch ports
   d. Use `codemux browser open {app_url}` to verify the app is accessible there
   e. If verification fails, fix the issue and retry the same assigned URL
   f. Only say DONE if the app is actually accessible at the assigned app URL

IMPORTANT: You MUST include the word "DONE:" in your output when finished. Format: DONE: <brief summary of what you built>
Example: DONE: Created hello.py with print statement
Example: DONE: Implemented user authentication in auth.js
Example: DONE: Built REST API endpoints for /users

CRITICAL: Your message must contain the word "DONE:" followed by what you completed. This is how the system knows you finished.

5. Say BLOCKED: <reason> if you cannot proceed

Never run commands or edit files without being assigned first.
Never work on tasks assigned to other instance IDs.
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
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir)?;

        let wrapper_content = r#"#!/bin/bash
# OpenCode wrapper - keeps the PTY alive and executes opencode commands on demand

set -uo pipefail

PROMPT_PATH="${CODEMUX_SYSTEM_PROMPT_PATH:-}"
GOAL_PATH="${CODEMUX_GOAL_PATH:-}"
AGENTS_PATH="${CODEMUX_OPENFLOW_AGENTS_PATH:-}"
MODEL="${OPENCODE_MODEL:-}"
WORKING_DIR="${CODEMUX_WORKING_DIR:-}"
ROLE="${CODEMUX_AGENT_ROLE:-}"
INSTANCE_ID="${CODEMUX_AGENT_INSTANCE_ID:-${ROLE:-agent}}"
AUTO_START="${CODEMUX_OPENFLOW_AUTO_START:-0}"
APP_PORT="${CODEMUX_OPENFLOW_APP_PORT:-}"

# Change to the working directory if set
if [ -n "$WORKING_DIR" ] && [ -d "$WORKING_DIR" ]; then
    cd "$WORKING_DIR" || exit 1
fi

if [ -n "$APP_PORT" ]; then
    export PORT="$APP_PORT"
fi

PROMPT=""
GOAL=""
AGENTS=""

if [ -n "$PROMPT_PATH" ] && [ -f "$PROMPT_PATH" ]; then
    PROMPT=$(cat "$PROMPT_PATH")
fi

if [ -n "$GOAL_PATH" ] && [ -f "$GOAL_PATH" ]; then
    GOAL=$(cat "$GOAL_PATH")
fi

if [ -n "$AGENTS_PATH" ] && [ -f "$AGENTS_PATH" ]; then
    AGENTS=$(cat "$AGENTS_PATH")
fi

run_opencode() {
    local message="$1"

    if [ -n "$MODEL" ]; then
        if [ -n "$message" ]; then
            opencode run "$message" --model "$MODEL"
        else
            opencode --model "$MODEL"
        fi
    else
        if [ -n "$message" ]; then
            opencode run "$message"
        else
            opencode
        fi
    fi
}

build_initial_message() {
    if [ "$AUTO_START" != "1" ]; then
        return
    fi

    local initial_message="$PROMPT"

    if [ -n "$GOAL" ]; then
        initial_message="${initial_message}

---

TOP-LEVEL GOAL:
${GOAL}
"
    fi

    if [ -n "$AGENTS" ]; then
        initial_message="${initial_message}

AVAILABLE AGENTS (use these exact IDs when assigning work):
${AGENTS}
"
    fi

    if [ -n "$GOAL" ]; then
        initial_message="${initial_message}
Start coordinating this run now. Delegate repo work to the other agents instead of doing it yourself.
"
    fi

    printf '%s' "$initial_message"
}

INITIAL_MSG="$(build_initial_message || true)"

if [ -n "$INITIAL_MSG" ]; then
    run_opencode "$INITIAL_MSG"
else
    printf '[wrapper] %s waiting for assignment\n' "$INSTANCE_ID"
fi

while IFS= read -r line; do
    if [ -z "$line" ]; then
        continue
    fi

    bash -lc "$line"
done
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

    /// Path to the shared role prompt (used as a fallback / base).
    pub fn prompt_path_for_role(role: &OpenFlowRole) -> PathBuf {
        let mut path = Self::prompts_dir();
        path.push(format!("{}.md", role.as_str()));
        path
    }

    /// Path to the per-instance prompt file.  The orchestrator (always one instance) uses
    /// the bare role name; all other roles get `{role}-{index}.md` so each parallel agent
    /// instance starts with its own identity baked in.
    pub fn prompt_path_for_instance(role: &OpenFlowRole, agent_index: usize) -> PathBuf {
        let mut path = Self::prompts_dir();
        if matches!(role, OpenFlowRole::Orchestrator) {
            path.push(format!("{}.md", role.as_str()));
        } else {
            path.push(format!("{}-{}.md", role.as_str(), agent_index));
        }
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
                let content = Self::build_prompt_for_role(&role, "", "", 0, "");
                std::fs::write(&path, content)?;
            }
        }

        Ok(())
    }

    /// Write an instance-specific prompt for the given role and agent index.
    pub fn write_prompt_for_run(
        role: &OpenFlowRole,
        run_id: &str,
        comm_log_path: &str,
        agent_index: usize,
        app_url: &str,
    ) -> std::io::Result<PathBuf> {
        let path = Self::prompt_path_for_instance(role, agent_index);
        let content =
            Self::build_prompt_for_role(role, run_id, comm_log_path, agent_index, app_url);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, content)?;
        Ok(path)
    }

    fn build_prompt_for_role(
        role: &OpenFlowRole,
        run_id: &str,
        comm_log_path: &str,
        agent_index: usize,
        app_url: &str,
    ) -> String {
        let role_str = role.as_str();
        let role_upper = role_str.to_uppercase();

        // For non-orchestrator roles, embed the instance ID so the agent knows its own identity
        // in the communication log (e.g. "You are BUILDER-0").
        let instance_label = if matches!(role, OpenFlowRole::Orchestrator) {
            role_upper.clone()
        } else {
            format!("{}-{}", role_upper, agent_index)
        };

        let base = BASE_CONTEXT
            .replace("{role}", role_str)
            .replace("{instance_id}", &instance_label)
            .replace("{run_id}", run_id)
            .replace("{app_url}", app_url)
            .replace("{comm_log_path}", comm_log_path)
            .replace("YOUR_ROLE", &role_upper);

        let role_specific_raw = match role {
            OpenFlowRole::Orchestrator => ORCHESTRATOR_PROMPT,
            OpenFlowRole::Planner => PLANNER_PROMPT,
            OpenFlowRole::Builder => BUILDER_PROMPT,
            OpenFlowRole::Reviewer => REVIEWER_PROMPT,
            OpenFlowRole::Tester => TESTER_PROMPT,
            OpenFlowRole::Debugger => DEBUGGER_PROMPT,
            OpenFlowRole::Researcher => RESEARCHER_PROMPT,
        };
        // Apply the same substitutions to the role-specific section so that
        // placeholders like {instance_id} are resolved there too.
        let role_specific = role_specific_raw
            .replace("{instance_id}", &instance_label)
            .replace("{app_url}", app_url);

        format!("{}{}", base, role_specific)
    }
}
