# Vexis `codemux-delegation` skill — proposed additions (hand to user)

The skill on `pandora` (`~/vexis-workspace/skills/software-dev/codemux-delegation/SKILL.md`)
assumes the project already exists as a git repo and tells the agent to call
`mcp__codemux__worktree_create` — which does NOT exist on the remote
`codemux-remote mcp` surface. Two additions fix the gaps. (Not applied on
pandora — paste/adapt as you see fit.)

## 1. New-project bootstrap (add as a step BEFORE "Standard flow (new work)")

> ### Bootstrapping a brand-new project
>
> If the project does not exist yet as a git repository, create it first —
> `worktree_create`/`worktree add` need a repo with at least one commit on a
> base branch:
>
> 1. Choose a normal project root: `~/projects/<name>` (NOT under
>    `~/.codemux/worktrees/`, which is reserved for per-branch worktrees).
> 2. Via `terminal_spawn` + `terminal_write` (or your shell tools):
>    ```sh
>    mkdir -p ~/projects/<name> && cd ~/projects/<name>
>    git init --initial-branch=main
>    git config user.email "<you>" && git config user.name "<you>"   # if not global
>    # ...scaffold initial files...
>    git add -A && git commit -m "init"
>    # If there are no files to commit yet, still make an empty first
>    # commit so `main` is forkable: git commit --allow-empty -m "init"
>    ```
> 3. Only after `main` has a commit do you create worktrees/workspaces.
>
> A repo with NO initial commit cannot be forked into a worktree — always
> make the first commit on `main`.

## 2. Correct the remote tool surface (replace the `worktree_create` bullet in "Standard flow")

> 3. **Create worktree + workspace.**
>    - On a **desktop** Codemux MCP: call `mcp__codemux__worktree_create`
>      (atomic: worktree + workspace + agent launch).
>    - On a **remote host** (`codemux-remote mcp`): call
>      `mcp__codemux__worktree_create` as well — it now exists on the
>      headless surface (Codemux ≥ next release) and runs `git worktree add`
>      under `~/.codemux/worktrees/<repo>/<branch>`, then registers the
>      workspace. Params: `repo_path` (absolute repo root), `branch`
>      (kebab-case), `new_branch: true`, `base: "main"`, optional `name`.
>    - Fallback for older `codemux-remote` daemons without `worktree_create`:
>      run `git worktree add ~/.codemux/worktrees/<repo>/<branch> -b <branch> main`
>      in a terminal, THEN `workspace_create` with `path` = that worktree dir
>      and `project_root` = the repo root (so it gets the shared project
>      identity + `worktree` kind). Do NOT register a worktree at a path you
>      never actually `git worktree add`-ed.

## Why

- The old skill presumed `main` already exists; a from-scratch project then
  gets a worktree branched off an empty/absent base.
- It named a desktop-only tool; on the remote the agent had to improvise,
  and improvised paths (sibling dirs) don't match what the desktop pull
  expects — contributing to the empty-pull symptom.
