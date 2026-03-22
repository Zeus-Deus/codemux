# Setup and Teardown Scripts

- Purpose: Describe automatic command execution on workspace create/delete.
- Audience: Anyone working with worktree workspaces or project onboarding.
- Authority: Canonical feature-level reality doc.
- Update when: Config format, execution behavior, or environment variables change.
- Read next: `docs/features/browser.md`, `docs/reference/CONTROL.md`

## What This Feature Is

Automatic command execution when workspaces are created or deleted. Essential for worktree workflows where each worktree needs its own dependency installation, environment files, or service setup.

## Current Model

Place a `.codemux/config.json` file in your project root (or workspace directory). Codemux reads it automatically during workspace lifecycle events:

- **Setup commands** run sequentially after workspace/worktree creation (in a background thread so creation is not blocked)
- **Teardown commands** run synchronously before workspace deletion

Config lookup order:
1. `{workspace_directory}/.codemux/config.json`
2. `{git_repo_root}/.codemux/config.json` (fallback, including for worktrees pointing back to the main repo)

### Configuration Examples

Node.js project:
```json
{
    "setup": ["npm install"],
    "teardown": []
}
```

Node.js with environment:
```json
{
    "setup": ["npm install", "cp .env.example .env"],
    "teardown": []
}
```

Docker project:
```json
{
    "setup": ["docker-compose up -d", "npm run db:migrate"],
    "teardown": ["docker-compose down -v"]
}
```

Python project:
```json
{
    "setup": ["python -m venv .venv", "source .venv/bin/activate && pip install -r requirements.txt"],
    "teardown": []
}
```

Rust project:
```json
{
    "setup": ["cargo build"],
    "teardown": []
}
```

### Environment Variables

Commands have access to:

| Variable | Value |
|----------|-------|
| `CODEMUX_ROOT_PATH` | Git repo root (or workspace path if not a git repo) |
| `CODEMUX_WORKSPACE_NAME` | Workspace title |
| `CODEMUX_WORKSPACE_ID` | Workspace ID |

## What Works Today

- Sequential setup command execution after workspace creation (all types: standard, preset, worktree)
- Sequential teardown command execution before workspace deletion (standard and worktree)
- Config fallback from worktree directory to main repo root
- Setup progress indicator (spinner) in sidebar WorkspaceRow
- Toast notification on setup failure
- Force-delete option when teardown fails
- Teardown warning in worktree delete confirmation dialog
- Manual setup re-run via `run_workspace_setup` Tauri command
- Config reading via `get_workspace_config` Tauri command
- Environment variable injection for all commands
- Unit tests for config reading, git root resolution, and worktree fallback

## Current Constraints

- Sequential execution only (no parallel command execution)
- No config merging (workspace-level config fully overrides repo-level config)
- No user-level override config (`~/.config/codemux/projects/<id>/config.json` not yet implemented)
- No timeout on individual commands (a hanging command blocks the setup thread)
- No socket/CLI command to trigger setup or teardown externally
- Setup runs once on creation; no automatic re-run on config file changes

## Important Touch Points

- `src-tauri/src/config/workspace_config.rs` — config struct, reader, git root resolver
- `src-tauri/src/scripts.rs` — setup/teardown execution with events
- `src-tauri/src/commands/workspace.rs` — lifecycle hooks and Tauri commands
- `src/components/sidebar/WorkspaceRow.svelte` — setup progress spinner
- `src/components/sidebar/Sidebar.svelte` — teardown error dialog, force-delete
- `src/stores/workspace.ts` — frontend store functions

## Tips

- Keep setup fast — it runs on every workspace creation
- Commit `.codemux/config.json` to share setup with your team
- Use shell scripts for complex logic: `{"setup": [".codemux/setup.sh"]}`
- Setup failure does not prevent workspace use — the workspace is fully created, PTYs are running
