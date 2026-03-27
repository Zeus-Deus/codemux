# Setup Scripts

Codemux can automatically run commands when workspaces are created or deleted. This is especially useful for worktree workflows where each worktree needs its own dependencies, environment files, or services.

## Configuration

### Via Settings UI

Open **Settings > Projects** to configure scripts for the active project:

- **Setup** — Commands that run when a new workspace is created (one per line)
- **Teardown** — Commands that run when a workspace is deleted (one per line)
- **Run** — A command to start your dev server, triggered via `Ctrl+Shift+G`

Changes are saved automatically when you leave the field.

### Via Config File

Create a `.codemux/config.json` file in your project root:

```json
{
  "setup": ["npm install", "cp .env.example .env"],
  "teardown": ["docker compose down"],
  "run": "npm run dev"
}
```

Commit this file to share setup automation with your team.

**Precedence**: File-based config (`.codemux/config.json`) always takes precedence over Settings UI configuration. When a config file is detected, the Settings UI shows a notice.

## Environment Variables

All scripts have access to these environment variables:

| Variable | Value |
|----------|-------|
| `$CODEMUX_ROOT_PATH` | Git repository root directory |
| `$CODEMUX_WORKSPACE_PATH` | Workspace/worktree directory |
| `$CODEMUX_WORKSPACE_NAME` | Workspace title |
| `$CODEMUX_WORKSPACE_ID` | Workspace unique ID |
| `$COMPOSE_PROJECT_NAME` | Project folder name (auto-set) |

### Docker Compose Gotcha

When using Docker Compose with worktrees, each worktree has a different directory name. Docker Compose uses the directory name as the default project name, which means each worktree would create separate containers and volumes.

Codemux automatically sets `COMPOSE_PROJECT_NAME` to the main project folder name **in setup and teardown scripts only**. This means `docker compose` commands in those scripts will correctly share containers across worktrees.

However, `COMPOSE_PROJECT_NAME` is **not** available in regular terminal sessions (e.g., when you open a terminal tab and run `docker compose` manually). In a regular terminal, Docker Compose falls back to using the directory name, which differs per worktree.

**To make it work everywhere**, add this line to your setup script so it writes the value into the project's `.env` file:

```bash
echo "COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME" >> .env
```

Docker Compose automatically reads `.env` from the working directory, so any terminal session in that worktree will pick up the correct project name — no extra environment variables needed.

## Run Command

The **Run** command starts your dev server in a dedicated terminal tab:

- Press `Ctrl+Shift+G` or use the command palette ("Run Dev Command")
- A new tab named "Workspace Run" is created with your dev server running
- Pressing `Ctrl+Shift+G` again restarts the command in the same tab (sends Ctrl+C first)
- Switch back to your working terminal anytime

## Examples

### Node.js
```json
{
  "setup": ["npm install"],
  "teardown": [],
  "run": "npm run dev"
}
```

### Docker Compose
```json
{
  "setup": [
    "echo COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME >> .env",
    "docker compose up -d",
    "npm install",
    "npm run db:migrate"
  ],
  "teardown": ["docker compose down -v"],
  "run": "npm run dev"
}
```

### Python
```json
{
  "setup": ["python -m venv .venv", ".venv/bin/pip install -r requirements.txt"],
  "teardown": [],
  "run": ".venv/bin/python manage.py runserver"
}
```

### Rust
```json
{
  "setup": ["cargo build"],
  "teardown": [],
  "run": "cargo run"
}
```

### Full-Stack (complex setup)
```json
{
  "setup": [".codemux/setup.sh"],
  "teardown": [".codemux/teardown.sh"],
  "run": "npm run dev"
}
```

For complex setups, use shell scripts and reference them from the config.

## Tips

- Keep setup commands fast — they run on every workspace creation
- Setup failure does not prevent workspace use
- Use `force delete` to skip teardown if a teardown command hangs
- The setup banner in the sidebar prompts you to configure scripts when you create your first worktree
