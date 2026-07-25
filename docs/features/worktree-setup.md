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

The same bootstrap (includes copy + setup scripts + env vars below) also runs when a worktree is created on a remote host through the headless daemon's `worktree_create` MCP tool — see `docs/features/setup-teardown.md` § "Headless Daemon Parity".

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


## Where worktree-include patterns come from

`process_worktree_includes()` (`src-tauri/src/scripts.rs`) resolves patterns from
the **first** of three sources — this is the single most surprising behavior in
this area:

1. a `.codemuxinclude` file in the project root (`IncludeSource::File`)
2. otherwise, the `worktree_includes` project setting (Settings → Projects, also
   valid as a `worktree_includes` key in `.codemux/config.json`) (`IncludeSource::Setting`)
3. otherwise, the hardcoded `DEFAULT_WORKTREE_INCLUDES = [".env", ".env.*", ".env.local"]`
   (`IncludeSource::Defaults`)

**Consequence: every new worktree gets your `.env` files copied in by default,
even with no `.codemuxinclude` file and no configured setting.** That is
usually what you want (a worktree that can actually run), but it means secrets
propagate into every worktree automatically. To opt out, create a
`.codemuxinclude` that does not match them — an empty file is enough, since an
existing file wins over the defaults.

Whichever source is used is emitted as a `worktree-includes-applied` event
(`{workspace_id, source, copied}`) and surfaced as a toast by
`src/hooks/use-worktree-include-toast.ts`, so the UI tells you which of the
three fired.

**Same-path guard:** when the workspace root and the worktree path resolve to the
same directory (i.e. a workspace opened at the repo root, not a worktree), the
whole include step short-circuits — otherwise `fs::copy(src, src)` would truncate
the file to zero bytes. So the copy step is silently a no-op for non-worktree
workspaces, including on an explicit "re-run setup".

## Important Touch Points

- `src-tauri/src/scripts.rs` — `process_worktree_includes()`, `script_env()`, `allocate_workspace_port()`
- `src-tauri/src/commands/workspace.rs` — `spawn_setup_scripts()`, `run_workspace_setup()`
- `src-tauri/src/config/workspace_config.rs` — `find_git_root()` (resolves worktree to main repo)
- `src-tauri/src/control.rs` — `rerun_setup` socket command
- `src-tauri/src/cli.rs` — `codemux workspace rerun-setup`
- `docs/features/setup-teardown.md` — full setup/teardown script reference
