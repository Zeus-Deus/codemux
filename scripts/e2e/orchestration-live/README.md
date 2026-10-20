# Live orchestration harness

Drives a real, isolated Codemux instance with real providers, headlessly, so
orchestration changes can be proven end to end: a debug `codemux serve` in a
bwrap sandbox, a headless browser paired to it over the web remote, and
read-only assertions against its store.

```
up.sh        isolated root, sandbox, serve, workspace, pairing → <root>/env
down.sh      stop only the recorded sandbox (graceful, or --hard)
drive.mjs    headless browser driver (library + CLI)
assert.sh    read-only sqlite + `codemux json conversation_read` helpers
run.mjs      run scenarios/*.mjs → <root>/report.md, report.json, shots/
lib.sh       shared env, sandbox and process helpers
```

## Run it

Build once per change (from the repo root):

```sh
~/.cache/codemux-agents/cargo-slot cargo build -j 2 --manifest-path src-tauri/Cargo.toml
npm ci            # if node_modules is missing
npm run build     # debug serve loads <repo>/dist
(cd sidecar/claude-agent && bun install && bun run build)
```

A fresh worktree also needs placeholder files for every
`src-tauri/binaries/*` name in the main checkout (including
`codemux-managed-workflow-sidecar-<triple>`), or `tauri-build` panics.

Then:

```sh
scripts/e2e/orchestration-live/up.sh          # prints ROOT=… and PAIR_URL=…
node scripts/e2e/orchestration-live/run.mjs   # all scenarios, or name them: run.mjs smoke
scripts/e2e/orchestration-live/down.sh
```

Every script acts on `$ORCH_LIVE_ROOT`, an explicit root argument, or the
newest root under `~/.cache/codemux-agents/orch-v2/live/`. A root keeps its
store, logs, provider transcripts and screenshots after `down.sh`;
`up.sh <root>` restarts on the same state (restart scenarios), and
`down.sh --hard` simulates a crash.

Ad-hoc driving while a root is up:

```sh
node drive.mjs pair                                   # fresh pairing, wait for the app
node drive.mjs invoke get_app_state
node drive.mjs click composer-submit                  # [data-testid=composer-submit]
node drive.mjs wait 'document.title !== ""' 30000
node drive.mjs shot my-state                          # shots/my-state-{desktop,mobile}.png
./assert.sh sql "SELECT thread_id, provider FROM agent_chat_sessions"
./assert.sh conversation <thread_id>
./assert.sh reply <thread_id> 'PONG'
./assert.sh no-text 'secret-value'
```

## Write a scenario

Add `scenarios/<name>.mjs`:

```js
export const name = "s4-chat-to-chat";
export const description = "Chat B sends to idle chat A with thread_send";

export async function run(ctx) {
  const a = await ctx.startChat({ provider: "codex", model: "gpt-6-luna", effort: "low", text: "Reply READY" });
  await ctx.waitFor(() => ctx.assert.reply(a.threadId, "READY").catch(() => null), { timeout: 300_000 });
  ctx.observe("chat A", a.threadId);
  // …drive chat B, then assert on A
  const turns = await ctx.assert.sql(`SELECT COUNT(*) AS n FROM agent_chat_search WHERE thread_id = '${a.threadId}' AND role = 'user'`);
  ctx.check("A got a second turn", turns[0].n === 2, turns);
  await ctx.openTab(a.tabId);
  await ctx.shot("s4-chat-a");
}
```

`ctx` gives you:

| Member | Use |
|---|---|
| `startChat({provider, model, effort, text, permissionMode})` | Create a chat pane bound to a fresh thread, start the session and send the first turn, all through the paired browser's remote transport (the same calls the composer makes). Returns `{threadId, paneId, tabId}`. |
| `driver.invoke(cmd, args)` | Any backend command through the remote transport, so the web-remote allow-list applies exactly as it does for a paired phone. |
| `driver.click(testid)`, `driver.clickSelector(css)` | Real clicks. User actions (Stop, Open question, Merge back, Save securely) must be clicks, not invokes. |
| `openTab(tabId)` | Click a tab in the tab strip. |
| `waitFor(jsExpr \| asyncFn, {timeout, interval, label})` | Poll until truthy. Never a bare sleep. |
| `assert.sql(q)`, `assert.conversation(id)`, `assert.reply(id, regex)`, `assert.noText(text)` | Read-only store and transcript checks. |
| `codemux(cmd, args)` | Control-socket call (`codemux json`), e.g. `get_app_state`. |
| `check(label, ok, evidence)` | Record a pass/fail line; a failure ends the scenario. |
| `observe(key, value)` | Record ids for the report. |
| `shot(name, {phone})` | Screenshot at 1440×900 and 390×844. Below the phone breakpoint the app shows the phone shell; `phone(driver)` navigates it first. |

Each scenario has a 10-minute budget (`ORCH_LIVE_SCENARIO_TIMEOUT_MS`). A
failure takes a `<name>-failure` screenshot. Prompts should be imperative and
name the exact tool and arguments, so runs are close to deterministic. Inside
agent tasks, wait with `python3 -c 'import time; time.sleep(N)'`; Claude's
harness blocks a standalone `sleep`.

## Cost

Default to cheap models: `claude-haiku-4-5` for Claude and `gpt-6-luna` with
low effort for Codex (`ORCH_LIVE_CLAUDE_MODEL` / `ORCH_LIVE_CODEX_MODEL`
override the smoke scenario). `up.sh` spends one Haiku call on its credential
pre-flight (`ORCH_LIVE_SKIP_CLI_PREFLIGHT=1` skips it). The smoke scenario is
two one-line turns. Run only the scenarios you need.

## Safety rules

- **Never copy credential files.** Refreshing a copied OAuth token rotates the
  refresh token and can sign the user out of every real session. The sandbox
  bind-mounts them read-write:
  - Claude persists a refreshed token with a temp file + rename, and a rename
    onto a file bind-mount fails with `EBUSY` (the new token would be lost).
    So the real `~/.claude` directory is bound, and run-local directories are
    mounted over its `projects sessions todos file-history session-env
    shell-snapshots tasks backups plugins skills`; `settings.json`,
    `settings.local.json` and `CLAUDE.md` are replaced by empty files.
  - Codex rewrites `auth.json` in place, so only that file is bound into an
    otherwise private `~/.codex`.
  - Hermes is not wired yet. It also writes `auth.json` with temp + replace,
    so it needs the Claude treatment (bind the profile directory, overlay
    its run state), not a file bind. Add that before any Hermes scenario.
  - Only `claude`, `codex` and `node`/`npm`/`npx` are linked into
    `<root>/bin` (the sandbox PATH). Other provider CLIs need a link there,
    a `--ro-bind` if they live outside `~/.local/share/mise` or `/usr`, and
    their own credential treatment.
  - `up.sh` proves both are the host's files (same device and inode) and that
    a rename inside `~/.claude` reaches the host, then runs `claude -p` and
    `codex login status` inside the sandbox.
- **No X server, ever.** The browser is headless Chromium via `agent-browser`.
  The sandbox environment is an allow-list without `DISPLAY` or
  `WAYLAND_DISPLAY`. Never start Xvfb or Xwayland on the host.
- **Kill only your own processes.** `down.sh` signals only the PID in
  `<root>/serve.pid` (checked to still be our `bwrap` and its process-group
  leader) and its descendants. The sandbox has its own PID namespace, so
  provider CLIs die with it. Never `pkill`, and never `pgrep -f` a pattern.
- **Isolation.** The sandbox's HOME is `<root>/home` mounted at the real HOME
  path; XDG dirs point into the root; the caller's `CODEMUX_*` and `CLAUDE*`
  variables never reach it. Release builds are refused (they use the real
  store and socket names).
- **Roots live under `~/.cache`, never `/tmp`.** `/tmp` has a per-user quota
  shared by every agent, and SQLite there has failed with "disk I/O error".

## Local mode on the paired browser

A paired browser normally stops at the sign-in screen unless the desktop is
signed in to a Codemux account. `up.sh` starts serve with
`CODEMUX_E2E_LOCAL_REMOTE=1`; the browser reads it through the
`web_remote_e2e_local_mode` command and enters local mode. The flag only
works in debug builds (`cfg!(debug_assertions)`); release builds always
answer `false`. No source patch is needed.

## Web remote policy

Fork, merge and remote-delegation commands are blocked over the web remote
(`src-tauri/src/web_remote/policy.rs`). Scenarios that need them either run
the action as the agent (MCP, desktop side) or use allow-list entries the
feature PR adds. Never patch the allow-list just for a test.
