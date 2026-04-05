# Worktree Environment Bootstrapping

- Purpose: Describe how Codemux bootstraps worktree workspaces with gitignored files, environment variables, and setup scripts.
- Audience: Users configuring worktree workflows, especially with Docker, secrets, or build caches.
- Authority: Canonical feature-level reality doc for worktree bootstrapping.
- Update when: .codemuxinclude behavior, env vars, or setup order changes.
- Read next: `docs/features/setup-teardown.md`

## What This Feature Is

When Codemux creates a git worktree workspace, gitignored files like `.env`, build caches, and secrets are missing. The worktree bootstrapping system copies these files from the main worktree and provides environment variables so setup scripts can reference the original project root.

## .codemuxinclude

A `.codemuxinclude` file in the project root (committed to git) lists gitignored files to copy from the main worktree into new worktrees. Uses gitignore-style patterns.

```
.env
.env.*
config/master.key
```

Files are copied (not symlinked), preserving directory structure. Runs before setup scripts so copied files are available during setup.

## Environment Variables

Setup and teardown scripts receive:

| Variable | Value |
|----------|-------|
| `CODEMUX_ROOT_PATH` | Main git repo root (the original checkout, not the worktree) |
| `CODEMUX_WORKSPACE_PATH` | Worktree directory path |
| `CODEMUX_WORKSPACE_NAME` | Workspace title |
| `CODEMUX_WORKSPACE_ID` | Workspace ID |
| `CODEMUX_BRANCH` | Git branch name |
| `CODEMUX_PORT` | Stable base port (hash-derived, 10-port range per workspace) |

## Docker Compose Pattern

Docker Compose auto-reads `.env` from the working directory. Combined with `.codemuxinclude`, this gives a clean workflow:

### Shared containers (recommended default)

All worktrees share the same Docker containers and volumes.

1. Add `COMPOSE_PROJECT_NAME=my-project` to your `.env`
2. Create `.codemuxinclude` listing `.env`
3. `docker compose` from any worktree targets the same containers

```
# .env (gitignored, in main worktree)
COMPOSE_PROJECT_NAME=my-project
DATABASE_URL=postgres://localhost:5432/mydb
SECRET_KEY=dev-secret-123
```

```
# .codemuxinclude (committed)
.env
```

When an agent runs `docker compose up` from a worktree, Docker reads the copied `.env`, sees `COMPOSE_PROJECT_NAME=my-project`, and connects to the same stack.

### Isolated containers per worktree

Each worktree gets its own Docker stack with unique ports.

1. Omit `COMPOSE_PROJECT_NAME` from `.env` (Docker defaults to the directory name, which differs per worktree)
2. Use `CODEMUX_PORT` in a setup script for unique port mappings

```json
{
  "setup": [
    "sed -i \"s/HOST_PORT=.*/HOST_PORT=$CODEMUX_PORT/\" .env",
    "docker compose up -d"
  ],
  "teardown": ["docker compose down -v"]
}
```

## Re-run Setup

The full bootstrap pipeline (`.codemuxinclude` copy + setup scripts) can be re-triggered on existing workspaces:

- **Context menu**: Right-click workspace > "Re-run Setup"
- **Socket API**: `{"command": "rerun_setup", "workspace_id": "..."}`
- **CLI**: `codemux workspace rerun-setup [workspace-id]`

Common use case: update `.env` in the main worktree, then re-run setup to push changes to existing workspaces.

## Order of Operations

1. `git worktree add` creates the worktree
2. `.codemuxinclude` files copied from main worktree
3. Setup scripts run with environment variables set

## Important Touch Points

- `src-tauri/src/scripts.rs` — `process_codemuxinclude()`, `script_env()`, `allocate_workspace_port()`
- `src-tauri/src/commands/workspace.rs` — `spawn_setup_scripts()`, `run_workspace_setup()`
- `src-tauri/src/config/workspace_config.rs` — `find_git_root()` (resolves worktree to main repo)
- `src-tauri/src/control.rs` — `rerun_setup` socket command
- `src-tauri/src/cli.rs` — `codemux workspace rerun-setup`
- `docs/features/setup-teardown.md` — full setup/teardown script reference
