# Cross-provider delegation

A Full-access Claude or Codex chat can hand a task to another installed agent
when you ask for it: *"have Codex add a slugify helper with tests, then review
it"*, or *"ask Claude on opus at max effort to review this diff"*. The chat calls
the `delegate_task` tool, Codemux opens the other agent as an ordinary chat in a
tab beside it, and the agent's final report is posted back into the chat. The
tool does nothing unless the model is asked to use another agent.

It is on by default. **Settings → Agent → Delegate to other agents** turns it
off: Claude chats lose the tool right away, Codex chats from their next new chat.

## What you see

- A card where the hand-off happened: provider, title, model and effort, status,
  timer, one line of activity, **Stop** and **Open**. Contiguous hand-offs share
  one card.
- The child tab next to the chat. Focus stays where it was. The child is a
  normal chat: watch it, answer it, or take it over.
- Above the composer while the chat is idle: *Continues when Codex finishes*,
  with Open, Stop and Stop all.
- When the round is done, one **Delegated results** divider. Expanding it shows
  exactly what the chat received. The chat then checks the work and answers.

## How it behaves

| Situation | Result |
| --- | --- |
| Several hand-offs in one request | One results message once all of them finish; a failure is posted at once |
| The chat is busy, rate-limited or has no live session | The report waits; nothing is pushed into a running turn |
| The child asks a question or needs approval | Card reads *Waiting for your answer in its tab*; answer there |
| The child hits its usage limit | *Paused* until auto-resume runs; without auto-resume it fails with the reset time |
| The child fails to start or errors | Card turns **Failed** with the reason; the chat is told and asked not to retry on its own |
| Stop, close or New Chat in the parent | Its children stop too; nothing is posted |
| Stop, close or a restart in a child tab | That task stops quietly and is noted in the round's report |
| Codemux quits mid-task | On the next launch the card shows *Stopped*; nothing starts by itself |

Rules that keep it predictable:

- Only Full-access chats delegate (checked on the chat's live mode, so the
  Plan and Ask pills refuse). Children run with full access too.
- One level deep: a child is never offered the tool.
- The child receives only the task text plus a short request for a final report.
  It does not see the parent conversation; its own CLI loads `AGENTS.md`.
- At most 3 running hand-offs per chat and 6 per user message. Failed tasks are
  never re-run automatically. There are no time limits.
- Children share the parent's working directory. Give parallel tasks separate
  files.

## Coverage

| Role | Providers |
| --- | --- |
| Can delegate | Claude Code; Codex (chats started in Full access) |
| Can receive a task | Claude Code, Codex, OpenCode, Cursor, Grok Build (CLI on `PATH`) |

Hermes is not supported yet in either role. In Supervised or Auto-accept-edits
Claude chats the call needs approval and is then refused with a pointer to Full
access.
