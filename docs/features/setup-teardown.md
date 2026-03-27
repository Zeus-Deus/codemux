# Setup and Teardown Scripts

- Purpose: Describe automatic command execution on workspace create/delete and the run dev command.
- Audience: Anyone working with worktree workspaces or project onboarding.
- Authority: Canonical feature-level reality doc.
- Update when: Config format, execution behavior, or environment variables change.
- Read next: `docs/features/browser.md`, `docs/reference/CONTROL.md`

## What This Feature Is

Automatic command execution when workspaces are created or deleted, plus a "run" command for starting dev servers. Essential for worktree workflows where each worktree needs its own dependency installation, environment files, or service setup.

## Current Model

### Configuration Sources (precedence order)

1. `.codemux/config.json` in workspace directory (file, highest priority)
2. `.codemux/config.json` in git repo root (file, fallback)
3. Settings > Projects UI (stored in SQLite via `project.scripts:<path>` key)

File-based config always takes precedence over UI-configured scripts.

### Lifecycle

- **Setup commands** run sequentially after workspace/worktree creation (in a background thread so creation is not blocked)
- **Teardown commands** run synchronously before workspace deletion
- **Run command** opens a dedicated "Workspace Run" terminal tab, triggered via `Ctrl+Shift+G`

### Configuration Examples

File-based (`.codemux/config.json`):

```json
{
    "setup": ["npm install"],
    "teardown": [],
    "run": "npm run dev"
}
```

Docker project:
```json
{
    "setup": ["docker-compose up -d", "npm run db:migrate"],
    "teardown": ["docker-compose down -v"],
    "run": "npm run dev"
}
```

Python project:
```json
{
    "setup": ["python -m venv .venv", "source .venv/bin/activate && pip install -r requirements.txt"],
    "teardown": [],
    "run": "source .venv/bin/activate && python manage.py runserver"
}
```

UI-based: Settings > Projects section provides textareas for setup, teardown, and run. Auto-saves on blur.

### Environment Variables

Commands have access to:

| Variable | Value |
|----------|-------|
| `CODEMUX_ROOT_PATH` | Git repo root (or workspace path if not a git repo) |
| `CODEMUX_WORKSPACE_PATH` | Workspace/worktree directory path |
| `CODEMUX_WORKSPACE_NAME` | Workspace title |
| `CODEMUX_WORKSPACE_ID` | Workspace ID |
| `COMPOSE_PROJECT_NAME` | Auto-set to project folder name (prevents Docker container collisions across worktrees) |

### Run Command Behavior

- `Ctrl+Shift+G` or command palette "Run Dev Command"
- Creates a new terminal tab named "Workspace Run"
- If a "Workspace Run" tab already exists, reuses it (sends Ctrl+C to stop the previous process, then runs the command again)
- Errors if no run command is configured

### Setup Banner

A sidebar banner appears when:
- The active project has no setup scripts configured (neither file nor DB)
- The project has at least one worktree workspace
- The banner has not been dismissed

The "Configure" button opens Settings > Projects. Dismiss persists per-project.

## What Works Today

- Sequential setup command execution after workspace creation (all types: standard, preset, worktree)
- Sequential teardown command execution before workspace deletion (standard and worktree)
- Config fallback from worktree directory to main repo root, then to DB-stored scripts
- Setup progress indicator (spinner) in sidebar WorkspaceRow
- Toast notification on setup failure
- Force-delete option when teardown fails
- Teardown warning in worktree delete confirmation dialog
- Manual setup re-run via `run_workspace_setup` Tauri command
- Config reading via `get_workspace_config` Tauri command
- `run` field for dev server commands with dedicated terminal tab
- Settings > Projects UI for configuring scripts without editing JSON
- Sidebar setup banner prompting users to configure scripts
- `Ctrl+Shift+G` keyboard shortcut and command palette entry for run command
- `CODEMUX_WORKSPACE_PATH` and `COMPOSE_PROJECT_NAME` environment variables
- DB-stored project scripts via `get_project_scripts` / `set_project_scripts` Tauri commands
- Unit tests for config reading, git root resolution, worktree fallback, and DB roundtrip

## Current Constraints

- Sequential execution only (no parallel command execution)
- No config merging (workspace-level config fully overrides repo-level config)
- No timeout on individual commands (a hanging command blocks the setup thread)
- No socket/CLI command to trigger setup or teardown externally
- Setup runs once on creation; no automatic re-run on config file changes

## Important Touch Points

- `src-tauri/src/config/workspace_config.rs` — config struct, reader, git root resolver, `read_effective_config`
- `src-tauri/src/database.rs` — `ProjectScripts` struct, DB get/set methods
- `src-tauri/src/scripts.rs` — setup/teardown/run execution with events
- `src-tauri/src/commands/workspace.rs` — lifecycle hooks, `run_project_dev_command`, Tauri commands
- `src-tauri/src/commands/database.rs` — `get_project_scripts`, `set_project_scripts` commands
- `src/components/settings/settings-view.tsx` — Projects section UI
- `src/components/layout/sidebar-setup-banner.tsx` — setup prompt banner
- `src/hooks/use-keyboard-shortcuts.ts` — Ctrl+Shift+G handler
- `src/tauri/commands.ts` — frontend command wrappers

## Tips

- Keep setup fast — it runs on every workspace creation
- Commit `.codemux/config.json` to share setup with your team
- Use shell scripts for complex logic: `{"setup": [".codemux/setup.sh"]}`
- Setup failure does not prevent workspace use — the workspace is fully created, PTYs are running
- `COMPOSE_PROJECT_NAME` prevents Docker container/volume collisions across worktrees
- Use the Settings UI for personal scripts, `.codemux/config.json` for team-shared scripts
