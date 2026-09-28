# Hermes provider (experimental)

Codemux can run [Hermes](https://github.com/NousResearch/hermes-agent) profiles as chat agents over Hermes's official ACP interface. Built and tested against Hermes 0.21.3 (commit `498abb677ec39ea3ae9f8f5ed60e7def6bc47e70`). This is a bounded integration, not general Hermes compatibility; the restrictions below are deliberate.

## Setup and ownership

Configure an existing profile using Hermes. In Codemux Settings, select the official executable and profile root, refresh profiles, and check the runtime. The model picker contains Hermes → profile → native models, with “Use profile default” first. A started conversation keeps its original profile. Model IDs remain opaque and native labels are retained.

The executable must support `hermes --profile <id> acp --check`. ACP support is an optional Hermes dependency (`agent-client-protocol`); install it using the official Hermes instructions. Codemux does not install or inject it.

The adapter owns one official ACP child per canonical profile, with a stable process cwd. Each chat supplies its exact worktree cwd through native session/new/load. Foreground turns queue per profile; separate profiles run independently. A Codemux ownership lock rejects competing Codemux children but cannot control unrelated Desktop/gateway processes. Use one active interface per profile. SSH workspaces are unsupported.

Codemux's separate `hermes_bindings` table retains profile identity, installation, exact cwd, native/ACP conversation provenance, intended and effective model, edit policy and cleanup hold. Bindings survive local chat deletion. Missing/replaced profiles, missing worktrees and unusable native loads require repair: no fallback directory, replacement conversation, credential copying or native database edits.

## Compatibility restrictions

- **Reasoning:** hidden and rejected. No effort inheritance or slash-command workaround.
- **Named custom routes:** rejected at profile/configuration level, even when the current ID is bare `custom:gpt-5`. Supported bare custom endpoint control was tested with the fixture only.
- **Empty native sessions:** an ACP handle created before the first message may not have restorable native history after restart. The official load returned no usable session in the UI test; this is a visible repair error. Explicit New Chat is available, but no automatic replacement is made.
- **Compression:** native rotation provenance is stored. Reopening a chat whose native head differs from its ACP handle is blocked. Real automatic compression/recovery remains unverified; no continuation-chain repair or replacement is attempted.
- **Background work:** closing a chat cancels that chat and retains the profile child. No idle reaper or acknowledged background-drain contract exists in this revision. Close/archive keeps files and defers teardown scripts; destructive cleanup is refused, including force, while a hold remains. Holds survive local chat deletion and guard canonical paths. Existing ambiguous holds are not automatically cleared. Explicit disconnect/app exit does not certify a flush.
- **Learning:** native memory/skill tools are available. Compact success labels require a completed native result. Missing completion is explicitly unconfirmed; batch skill output without per-operation confirmation remains generic activity. No autonomous learning-quality, curator/background-review, configured MCP/plugin parity, or activity-feed certification.
- **Desktop:** later transcript visibility was checked through official Desktop backend handlers, not the rendered Desktop application. Cwd/project grouping and safe execution continuation are excluded. No import, bidirectional sync or “continue in Desktop” action.
- **Model/authentication:** real keyless `opencode-free` requests were attempted through native UI; none completed inference (the free tier refuses requests from outside OpenCode). Same-protocol model changes and offline model intent were exercised. Switching OpenCode Free models across API protocols is rejected because official ACP retains the old protocol despite confirming the new model ID. External credential lifecycle/revocation remains unverified.
- Permission IDs/scopes come from the runtime. Edit modes are not a terminal/filesystem sandbox. Images, projected Codemux skills, questionnaires and rich subagent controls are unavailable.

## Reproducible checks

Run the deterministic official-runtime harness:

```sh
python scripts/test-hermes-integration.py /absolute/path/to/hermes [isolated-ACP-dependency-directory]
```

The optional `--serve` starts the same fixture for native UI testing and writes an explicit `dev-env.json`. Use that environment for `npm run tauri:dev -- --no-watch`; it isolates HOME and every XDG data/config/cache/state/runtime location. `--root` only accepts disposable `codemux-hermes-integration-*` directories under the system temporary directory; `--port` permits fixture restarts on the same loopback endpoint. The fixture creates synthetic profile config/personas and records synthetic model requests. It has no configured MCP servers and disables background review/compression, so it cannot certify those features.


Focused checks:

```sh
npm run test -- src/lib/agent-chat/hermes-outcomes.test.ts src/lib/agent-chat/hermes-learning.test.ts src/components/chat/HermesPermissionOptions.test.tsx src/components/chat/pickers/HermesProfileModels.test.tsx src/stores/hermes-store.test.ts
cargo test -j 2 --manifest-path src-tauri/Cargo.toml --lib hermes_ -- --test-threads=2
cargo test -j 2 --manifest-path src-tauri/Cargo.toml --lib agent_provider::acp -- --test-threads=2
```

The Rust regression cases cover atomic offline intent, actual approval behavior after restart, config/alias compatibility gates, pre-initialization failure/retry/deletion, genuine cleanup holds and forced deletion, startup/deletion serialization, same-thread duplicate startup, profile independence, queued cancellation, and archive-history reassociation. `/bin/false` is used only as a deliberately failing transport in a unit test, never as a Hermes substitute. The integration test always executes the official Hermes binary.
