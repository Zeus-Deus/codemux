# Suggested follow-up tasks

A Claude or Codex chat that notices work outside the current request (a related
bug, missing tests, a refactor) suggests it with the `suggest_tasks` tool
instead of listing it in prose. Nothing starts on its own: the chat shows the
tasks as a card and carries on.

## What you see

- A card, *N suggested tasks*, with **Start all**. Each task has its title, the
  brief (click to expand), an agent picker (Claude, Codex and any other
  installed agent that can take a task), **Do it here** and **Start**.
- **Start** creates a new workspace on its own git worktree, on a new branch
  named after the task and made from this workspace's latest commit.
  Uncommitted edits here are not carried over. The chosen agent gets the brief
  and is asked to commit its work on that branch.
- A started task becomes a status row: Starting, Working, Waiting for your
  answer, Done (branch, `+adds −dels`), Failed or Stopped, with **Open**.
- The new workspace is a normal card in the sidebar with a link back to this
  one, and nests under it in the Projects view.
- When started tasks finish, this chat gets one **Delegated results** message
  (the same one cross-provider delegation posts) and reviews it.
- **Merge all** merges each finished branch into this workspace's branch with
  `git merge --no-ff`, oldest first. Nothing is archived afterwards.
- **Do it here** sends this chat an ordinary message asking it to do the task
  now.

## Rules

| Situation | Result |
| --- | --- |
| This workspace has uncommitted changes | Merge all refuses and says so |
| A merge conflicts | That merge is aborted, merging stops, and the card names the task, branch and files |
| A finished task left uncommitted files in its worktree | It is reported as not mergeable and skipped |
| A task is still running, failed or stopped | Merge all skips it with the reason |
| Stop, close or New Chat in this chat | Started tasks keep running in their own workspaces |
| Stop in a started task, or closing / archiving its workspace | That task stops and is noted in the results |
| Codemux quits mid-task | On the next launch the row shows *Stopped* |

- Offered to Claude and Codex chats in every permission mode; it only
  suggests. A started task's own chat is never offered the tool.
- At most 5 tasks per call. Started tasks are not limited by delegation's
  running caps: the user started each one.
- Needs a git repository with at least one commit; workspaces on another
  device cannot start tasks.
