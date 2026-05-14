# Phase 1.5 — vexis-agent ↔ Codemux gap analysis

- Purpose: Identify the MCP gaps that block a Telegram-driven brain from materializing real work in Codemux (worktree + workspace + agent + prompt → working agent visible in sidebar).
- Authority: Research notes; not a build plan. Read after `docs/plans/vexis-agent-integration.md`.

## 1. workspace_create reality check

The MCP tool at `mcp_server.rs:338-347` accepts an optional `path` argument and dispatches to the `create_workspace` socket command via `mcp_server.rs:809-816`:

```rust
"workspace_create" => {
    let path = arguments.get("path").and_then(Value::as_str);
    let params = match path { Some(p) => json!({ "path": p }), None => json!({}) };
    call_socket("create_workspace", params).await
}
```

But the socket arm at `control.rs:573-578` **discards `params` entirely**:

```rust
"create_workspace" => {
    let state: State<'_, AppStateStore> = app.state();
    let db: State<'_, crate::database::DatabaseStore> = app.state();
    crate::commands::workspace::create_workspace_impl(app.clone(), &state, &db, None)
        .map(|workspace_id| serde_json::json!({ "workspace_id": workspace_id }))
}
```

The fourth arg to `create_workspace_impl` is hardcoded `None` — never reads `request.params.get("path")`. `create_workspace_impl` (`commands/workspace.rs:52-86`) takes that `None` and falls back to `crate::project::current_project_root()` (line 66).

**End-to-end test against the dev Codemux:**

```
BEFORE: workspaces: ['workspace-11','workspace-21','workspace-22','workspace-31','workspace-49']
CALL workspace_create with path "/tmp/recon-test-path"
RESPONSE: {"workspace_id": "workspace-59"}
AFTER: workspace-59 | Workspace 6 | /home/zeus/.codemux/worktrees/codemux/researchvexis-codemux-integration
```

The workspace IS created. `workspace-59` has surfaces (surface-60), an active terminal pane (pane-62, session-61), populated git info (`git_branch: researchvexis-codemux-integration`), and `project_root: /home/zeus/projects/codemux`. `emit_app_state` fires (line 84). The sidebar would display it.

**Verdict: workspace_create is half-broken.** It returns a real workspace ID and creates a usable workspace, but the `path` argument advertised in its schema is silently dropped — the workspace always lands at `current_project_root()`. The brain's "no such workspace" claim is most likely the brain checking whether a workspace exists *at the path it requested* and not finding one (because the cwd was overridden). The Phase 1 verification claim "all 5 tools verified working" is intact — workspace_create is one of the original 31 from before Phase 1, not part of the 5 new tools. This bug pre-dates Phase 1.

**One-line fix:** read `request.params.get("path")` in the socket arm and pass it through. ~3 LOC.

## 2. apply_preset machinery

Socket arm: `control.rs:591-610`. Inputs: `workspace_id`, `preset_id`, `override_mode` (optional, one of `new_tab` / `split_pane` / `current_terminal` / `existing_panes`), `initial_prompt` (optional). Delegates to `crate::commands::presets::apply_preset` (`commands/presets.rs:236-433`).

What it does (`commands/presets.rs:246-432`): looks up the preset, bounces `ChatAgent` kinds back with an explicit error (those go through `agentChatStart…` Tauri commands — line 262-267), validates every command binary is on PATH (line 270-275), resolves the effective `LaunchMode`, injects agent context env via `agent_context::inject_agent_context` (line 296), then branches on launch mode: `current_terminal` writes to the active session, `split_pane` splits per command, `existing_panes` chains commands into every existing terminal, `new_tab` creates a tab per command via `state.create_tab(workspace_id, TabKind::Terminal)`. Emits app state on the way out (line 431). The frontend "+" button invokes the same function — `apply_preset` is both a `#[tauri::command]` and a socket arm pointing at `commands::presets::apply_preset`, so the surfaces are byte-identical.

**MCP wrap needed:** thin. The socket command already exists. An MCP tool would parse `workspace_id`/`preset_id`/`override_mode`/`initial_prompt` from arguments, forward via `call_socket("apply_preset", …)`. ~25 LOC including schema. Brain still needs preset IDs — see §5.

## 3. Initial prompt injection

Lives at `branch_name.rs:175-201`:

```rust
pub fn prepare_agent_command(
    preset_id: &str,
    base_command: &str,
    initial_prompt: Option<&str>,
) -> (String, bool) {
```

Mechanism is preset-aware:

- **`builtin-claude`**: prompt appended as ANSI-C-quoted positional arg → `claude --dangerously-skip-permissions $'<prompt>'`. Single PTY write. (Line 188-191.)
- **`builtin-codex`**: same shape, positional arg. (Line 192-195.)
- **Everything else (Gemini, OpenCode, custom)**: returns `(base_command, true)` — `needs_pty_injection=true`. The caller starts the agent first, waits ~1500ms for the TUI to initialize, then writes the prompt via PTY as a second `write_command_when_ready` (e.g. `commands/presets.rs:328-334` and `commands/workspace.rs:422-431`).

Reuse is straightforward — `prepare_agent_command` is pure (no Tauri State) and is already called from three sites: `apply_preset` (line 318), `create_worktree_workspace` (line 405), and four call sites in `branch_name.rs` tests (line 307+). It's not coupled to issue-creation context. Any new MCP entry point that takes a `preset_id + initial_prompt` pair can call it and get the same Claude/Codex shortcut.

## 4. Worktree-and-workspace creation flow

Frontend: `branch-picker.tsx::handlePrimaryAction` (line 130-171) dispatches to one of `onOpenWorkspace`, `onImportWorktree`, `onCreateOnCurrent`, `onSelectBase`, or `onOpenExisting`. The "Fork →" callback ultimately invokes the Tauri command `create_worktree_workspace` via `src/tauri/commands.ts:390-409`:

```ts
export const createWorktreeWorkspace = (
  repoPath, branch, newBranch, layout,
  base?, initialPrompt?, agentPresetId?, prNumber?
) => invoke<string>("create_worktree_workspace", { ... });
```

Backend: `commands/workspace.rs:277-439`. **One atomic call does everything:** `git_create_worktree` under `~/.codemux/worktrees/<repo>/<sanitized-branch>` (line 313-315, see `git.rs:1464`); `state.create_workspace_with_layout(worktree_path, layout)` with single/pair/quad/six/eight/shell_browser/empty (line 316); `set_workspace_worktree` + `set_workspace_project_root` + `populate_git_info` (line 318-321); optional PR association (line 323-325); PTY spawn per terminal session (line 327-337); setup scripts in background (line 340); `.mcp.json` autoconfig (line 343-345); and if `agent_preset_id` is set, preset launch reusing the default tab for the first command, calling `prepare_agent_command(preset, command, initial_prompt)` and firing `write_command_when_ready` at 120ms for the agent launch and 1500ms for the prompt-via-PTY case (line 347-434). Closes with `emit_app_state` (line 437).

**Not on the control socket.** `grep '"create_worktree_workspace"' src-tauri/src/control.rs` returns nothing. Currently only the frontend can call it.

**Natural MCP shape: one tool.** The end-to-end behavior the target prompt needs ("worktree + workspace + agent + initial prompt + visible in sidebar") is exactly what `create_worktree_workspace` already does in a single call. Splitting it would mean the brain has to orchestrate worktree → workspace → preset, racing against state hydration and `emit_app_state` debouncing. The Tauri command already handles ordering; the MCP tool should mirror it.

## 5. Recommended Phase 1.5 MCP additions

Minimum set for the target use case:

| MCP tool | New socket command? | Wraps | Inputs | ~LOC |
|---|---|---|---|---|
| **`workspace_create` fix** | no | existing arm | adds `path` param read at `control.rs:573` | ~3 |
| **`worktree_create`** | YES | `create_worktree_workspace` | `repo_path`, `branch`, `new_branch` (default true), `base` (default `"main"`), `layout` (default `"single"`), `initial_prompt`, `agent_preset_id`, `pr_number` | ~80 (40 MCP + 40 socket arm calling `create_worktree_workspace`-equivalent code) |
| **`preset_apply`** | no | existing `apply_preset` arm | `workspace_id`, `preset_id`, `override_mode?`, `initial_prompt?` | ~25 |
| **`preset_list`** | YES | `get_presets` | none | ~30 (15 MCP + 15 socket) |

Total estimate: ~140 LOC of code + tests + tool-count test bump (36 → 39 if we skip the workspace_create fix as its own tool, or 36 → 39 with three new tools).

**The target prompt would resolve in two MCP calls:**

```
preset_list()  →  [{id: "builtin-claude", ...}, ...]
worktree_create(
  repo_path: "/home/zeus/projects/openclaw",
  branch: "computer-use-research",
  new_branch: true,
  base: "main",
  layout: "single",
  agent_preset_id: "builtin-claude",
  initial_prompt: "research how they do computer use"
)  →  "workspace-NN"
```

After the second call returns, the worktree exists on disk, the workspace is in state with PTY ready, Claude Code is launched with the prompt as a positional ANSI-C-quoted arg, and the sidebar emits via `emit_app_state`. The brain then confirms back via `app_status` or `workspace_info`.

**Optional ergonomic tools** (defer unless field tests show they're needed):
- `workspace_close` — wraps the close path so the brain can clean up after a Telegram session ends.
- `workspace_get_layout` — what panes/tabs exist; the brain currently inspects via `get_app_state` which is verbose.

## 6. Known issues uncovered during recon

- `workspace_create` MCP tool silently drops `path` argument (§1). Pre-existing; not a Phase 1 regression.
- `create_worktree_workspace`, `create_workspace_with_preset`, `get_presets` are all Tauri-only — the brain cannot discover or invoke them via MCP. This is the primary Phase 1.5 gap.
- `preset.commands` are validated against `command_binary_exists` (`commands/presets.rs:270-275`) before any tab/pane is created. A brain asking for `builtin-codex` on a host without `codex` on PATH gets a clean error string back. Good — no half-created workspaces.
