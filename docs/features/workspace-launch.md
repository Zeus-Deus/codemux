# Launch separate workspaces from chat

Ask a Full-access Claude or Codex chat to start separate workspaces, for example:
“Start two separate workspaces: one to fix the flaky sidebar test, one to fix the
terminal resize bug.” The agent calls `workspace_launch` once per workspace.
Each gets a new git worktree and branch, its own sidebar title, and an agent chat
that begins working. Your current workspace stays selected.

The new agent receives only `message`, so each brief must include the goal,
relevant files, constraints, verification, and whether to open a PR. Results stay
in the new workspace. Use `delegate_task` for tasks whose reports should return
to the original chat.

`title` and `message` are required. `branch` defaults to a unique kebab-case name
from the title; an explicit branch must be new. `base` defaults to the calling
workspace's current HEAD, including when it is itself a worktree. `provider`
defaults to the caller and may select an installed Claude, Codex, OpenCode,
Cursor, or Grok agent. Set `model` and `effort` only when explicitly requested;
they must match the provider's advertised capabilities. New chats use that
provider's Full-access mode.

The tool is available only to main Claude and Codex chats in Full access inside
local git workspaces. Remote workspaces, imported snapshots, and running
delegated tasks cannot launch workspaces. This is independent of the
cross-provider delegation setting. Outside MCP clients still cannot launch
worktrees through `thread_launch`.

Up to six launches may be starting per calling chat. Calls return `starting`
immediately; failures appear in Codemux notifications. If chat startup fails
after checkout creation, the workspace is kept for inspection because setup
scripts may already have written files.

For a synthetic demo, run `npm run dev` and open
`http://localhost:1420/?mockScenario=workspace-launch`.
