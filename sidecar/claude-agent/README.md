# claude-agent sidecar

A tiny TypeScript JSON-RPC server that Codemux's Rust side spawns as a
subprocess. Compiled to a standalone binary with Bun (`bun build
--compile`), shipped alongside Codemux as a Tauri "external bin", and
used as the execution home for any runtime that's easier to express in
TypeScript than in Rust.

## Current scope

Hosts Anthropic's Claude Agent SDK and exposes it over JSON-RPC. The
Rust side spawns the sidecar, starts a session per chat thread, sends
turns, and forwards every SDK message back to the UI. Tool-permission
callbacks from the SDK are bridged to the Rust side as
`request-opened` notifications; the Rust side answers with
`respond-to-request`.

## Requirements

- [Bun](https://bun.sh) ≥ 1.3. Not Node / npm / pnpm: Bun is the
  toolchain.

## Build

For the host platform:

```sh
bun install
bun run build
```

For every supported platform at once:

```sh
bun install
bun run build:all
```

Per-target scripts (`build:linux-x64`, `build:linux-arm64`,
`build:darwin-x64`, `build:darwin-arm64`, `build:windows-x64`) exist
for CI. Binaries land in `dist/codemux-claude-sidecar-<target>` (with
`.exe` on Windows).

## Run manually

For debugging the sidecar by hand, skip the compile step:

```sh
bun run src/main.ts
```

Then feed it JSON-RPC lines on stdin:

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"ping","params":{"hello":"world"}}' \
  | bun run src/main.ts
```

## Test

```sh
bun test
```

Tests spawn `bun run src/main.ts` as a subprocess and exercise the
protocol surface end-to-end.

## Protocol

Newline-delimited JSON-RPC 2.0 over stdin/stdout. Every line is one
complete JSON envelope. Log output goes to stderr only; stdout is
reserved for the protocol channel. See `src/rpc.ts` for the envelope
types and `src/main.ts` for the dispatch loop.

## Exposed methods

| Method | Purpose |
|---|---|
| `start-session` | Spawn a new ClaudeSession. |
| `send-turn` | Queue a user message onto a session. |
| `interrupt` | Halt the current turn. |
| `set-model` | Swap the session default model. |
| `set-permission-mode` | Change permission mode. |
| `respond-to-request` | Resolve a pending tool approval. |
| `respond-to-user-input` | Answer an AskUserQuestion prompt. |
| `initialization-result` | Read the SDK's cached init payload. |
| `stop-session` | Close a session (idempotent). |
| `probe-installed` | Check `<binary> --version`. |
| `probe-authenticated` | Check `<binary> auth status`. |
| `ping` | Liveness probe. |

## Notifications

Emitted to stdout as JSON-RPC notifications whenever the session does
something the client should know about:

| Method | When |
|---|---|
| `session-configured` | Session created. |
| `sdk-message` | Every SDKMessage from `query()`, passed through raw. |
| `session-ended` | Iteration completed; `reason`: `"iteration-complete"` / `"interrupted"`. |
| `session-error` | Stream threw a non-abort error. |
| `request-opened` | SDK called `canUseTool`. |
| `request-resolved` | The parked approval was resolved. |
| `plan-proposed` | Assistant emitted `ExitPlanMode`. |
| `user-input-requested` | Assistant emitted `AskUserQuestion`. |

## ToS boundary

The sidecar is the only place in Codemux that integrates with Claude
at all, so a static check enforces that we stay inside Anthropic's
officially supported SDK:

1. No reads of `.claude.json` or `~/.anthropic/`.
2. No references to `api.anthropic.com` or `anthropic.com`.
3. No spawning the `claude` binary outside `src/auth-probe.ts` (which
   is allow-listed for `--version` and `auth status` probes).
4. No reads of `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`.

Run the check:

```sh
bun run check-tos
```

The check runs automatically as part of `bun test`. CI has it as a
separate explicit step too.

## Layout

```
sidecar/claude-agent/
  package.json          Bun project manifest
  bunfig.toml           Bun install config
  tsconfig.json         Strict TypeScript config
  src/
    main.ts             Entry point, stdin loop, dispatch
    rpc.ts              JSON-RPC envelope types + parse/format
    logger.ts           Stderr-only structured logger
    methods/
      ping.ts           The only method so far
  test/
    ping.test.ts        End-to-end subprocess tests
  dist/                 Compiled binaries (git-ignored)
```
