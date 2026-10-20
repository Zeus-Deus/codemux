# Live orchestration harness

Drives a real, isolated Codemux instance with real providers, headlessly, so
orchestration changes can be proven end to end: a debug `codemux serve` in a
bwrap sandbox, a headless browser paired to it over the web remote, and
read-only assertions against its store.

```
up.sh               isolated root, sandbox, isolation pre-flight, serve, workspace, pairing → <root>/env
down.sh             stop only the recorded sandbox (graceful, or --hard), then check the real credentials
drive.mjs           headless browser driver (library + CLI)
assert.sh           read-only sqlite + `codemux json conversation_read` helpers
run.mjs             run scenarios/*.mjs → <root>/report.md, report.json, shots/
lib.sh              shared env, sandbox, stop and credential-check helpers
isolation-check.sh  runs inside the sandbox during up.sh; asserts what must be out of reach
tests/              harness self-tests (no providers, no build): teardown-test.sh
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

Each scenario has a 10-minute budget (`ORCH_LIVE_SCENARIO_TIMEOUT_MS`). When
it runs out, `ctx.signal` aborts: browser, CLI and assert calls in flight are
killed and every `waitFor` throws. A scenario that is still running 30 s
later ends the whole run. A failure takes a `<name>-failure` screenshot.
Assert on what the model produced, never on text the prompt already contains
(the smoke scenario asks for a computed token and looks for it only in
`[data-testid="assistant-message"]`). Prompts should be imperative and
name the exact tool and arguments, so runs are close to deterministic. Inside
agent tasks, wait with `python3 -c 'import time; time.sleep(N)'`; Claude's
harness blocks a standalone `sleep`.

## Cost

Default to cheap models: `claude-haiku-4-5` for Claude and `gpt-6-luna` with
low effort for Codex (`ORCH_LIVE_CLAUDE_MODEL` / `ORCH_LIVE_CODEX_MODEL`
override the smoke scenario). `up.sh` spends one Haiku call on its credential
pre-flight (`ORCH_LIVE_SKIP_CLI_PREFLIGHT=1` skips it). The smoke scenario is
two one-line turns (`ORCH_LIVE_SMOKE_PROVIDERS=claude` or `=codex` runs one;
unknown or empty values are an error). Run only the scenarios you need.

## Safety rules

- **Never copy credential files.** Refreshing a copied OAuth token rotates the
  refresh token and can sign the user out of every real session. The sandbox
  bind-mounts the real files instead, and exposes nothing else of the profile
  directories:
  - **Claude.** The sandbox's `~/.claude` is private (`<root>/home/.claude`):
    transcripts, sessions, settings, history and plugins are all run-local.
    Credentials come from the real `~/.claude`, bound at
    `/run/orch-live/claude-auth` and selected with
    `CLAUDE_SECURESTORAGE_CONFIG_DIR` (the CLI reads `.credentials.json` and
    takes its refresh lock in that directory; checked in the 2.1.299
    binary). The directory itself is bound because Claude persists a
    refreshed token with temp file + rename, which fails with `EBUSY` on a
    file bind-mount (the new refresh token would be lost). Every existing
    entry there except `.credentials.json` and the refresh lock is covered
    by a read-only empty file or directory, so the only things the sandbox
    can create in the real `~/.claude` are new top-level entries, which is
    what Claude's credential writes need. `down.sh` lists any new top-level
    entry other than `.credentials.json`, `.credentials.json.tmp.*` and
    `.oauth_refresh.lock*`, or a changed symlink, and fails.
  - **Codex** rewrites `auth.json` in place, so only that file is bound into
    an otherwise private `~/.codex`.
  - `down.sh` checks that both real files are still non-empty JSON objects
    and fails loudly if not, or if a file that existed when `up.sh` ran
    (recorded in `<root>/credentials.present`) is gone. It never restores
    them: re-login instead.
  - Hermes is not wired yet. It writes `auth.json` with temp + replace, so it
    needs the Claude treatment (its storage dir bound, everything else in it
    covered). Add that before any Hermes scenario.
  - Only `claude`, `codex` and `node`/`npm`/`npx` are linked into
    `<root>/bin` (the sandbox PATH). Other provider CLIs need a link there,
    a `--ro-bind` if they live outside `~/.local/share/mise` or `/usr`, and
    their own credential treatment.
  - `up.sh` proves both are the host's files (same device and inode) and that
    a rename in Claude's storage dir reaches the host, then runs `claude -p`
    and `codex login status` inside the sandbox.
- **Concurrent refresh with the host (residual risk).** The user's own Claude
  and Codex sessions keep running while a live run uses the same login.
  Claude serialises refreshes across processes with a lock directory
  (`.oauth_refresh.lock`) in the shared storage dir and re-reads the file
  after taking it, so the sandbox and the host don't both spend the same
  refresh token; the lock's dead-holder takeover refuses holders in another
  PID namespace, so a crashed sandbox refresh can delay a host refresh by up
  to the lock's 60 s staleness window. Codex (0.162.1) has no cross-process
  lock for `auth.json`; on a 401 it reloads `auth.json` and uses tokens
  another process already refreshed, but two Codex processes refreshing in
  the same instant can still both spend the refresh token, and the loser
  reports "Your access token could not be refreshed. Please log out and sign
  in again." Codex refreshes rarely (tokens last days), so the window is
  small but real; keep live runs short.
- **Shutdown never SIGKILLs a provider.** Codex writes `auth.json` by
  truncate + write, so a kill mid-refresh can leave it empty. `down.sh`
  stops serve (SIGTERM; with `--hard`, SIGKILL to the serve process alone),
  lets provider CLIs exit on stdin EOF, sends SIGTERM to anything still
  running after 30 s, and otherwise fails and leaves the survivors for a
  human. It waits on the sandbox's PID-namespace init (bwrap's `--info-fd`
  `child-pid`), not bwrap's outer process: the outer process exits as soon
  as serve does, while the init keeps reaping orphaned provider processes
  and exits only once all of them have exited on their own, so namespace
  teardown never kills a live provider. (Codemux's own provider shutdown is
  out of the harness's hands.) `tests/teardown-test.sh` checks this, and the
  credential checks, with a stand-in serve and a fake HOME; it needs only
  bwrap, jq and python3.
- **Isolation.** The sandbox mounts the host read-only and gives the run
  private `/dev`, `/proc` (own PID namespace), `/tmp`, `/var/tmp`, `/dev/shm`
  and `/run` (only systemd-resolved's directory is re-exposed when
  `/etc/resolv.conf` points there). HOME is `<root>/home` mounted at the real
  HOME path; XDG dirs, including `XDG_RUNTIME_DIR`, point into the root. The
  environment is an allow-list: no `DISPLAY`, `WAYLAND_DISPLAY`,
  `DBUS_SESSION_BUS_ADDRESS`, `CODEMUX_*` or `CLAUDE*` from the caller. The
  only writable host paths are the root, the private HOME and the two
  credential binds. `up.sh` runs `isolation-check.sh` inside the sandbox and
  refuses to continue unless the real Codemux sockets
  (`/run/user/<uid>/codemux{,-dev}.sock`), the session bus, keyring and X
  sockets are unreachable, the repo and its git dir are read-only or
  invisible, every writable mount is an expected one, and probes written to
  the sandbox's `/tmp` and `/var/tmp` don't appear on the host. The network
  namespace is shared (providers need the internet), so abstract Unix
  sockets such as X's `@/tmp/.X11-unix/X0` remain reachable in principle;
  nothing in the sandbox is told about them. Release builds are refused
  (they use the real store and socket names).
- **No X server, ever.** The browser is headless Chromium via
  `agent-browser`. Never start Xvfb or Xwayland on the host.
- **Kill only your own processes.** `down.sh` signals only the sandbox
  recorded in `<root>/env` after checking its PID, its start time
  (`/proc/<pid>/stat` field 22) and the run's `ORCH_LIVE_RUN_ID` marker in its
  environment, and only processes below it. `up.sh` records that metadata
  right after launch, and stops the sandbox again if startup fails or is
  interrupted. Never `pkill`, and never `pgrep -f` a pattern.
- **Ports.** `up.sh` picks a free port, then verifies serve's own status and
  that the listener belongs to its serve process; a lost race retries on
  another port (`ORCH_LIVE_PORT` pins one, or a comma list to try in order).
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
