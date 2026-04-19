# claude-agent sidecar

A tiny TypeScript JSON-RPC server that Codemux's Rust side spawns as a
subprocess. Compiled to a standalone binary with Bun (`bun build
--compile`), shipped alongside Codemux as a Tauri "external bin", and
used as the execution home for any runtime that's easier to express in
TypeScript than in Rust.

## Current scope

Scaffold only. One method — `ping` — that echoes its params back with a
server timestamp. Real methods land in future commits.

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
