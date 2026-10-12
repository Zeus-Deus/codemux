# Native harness workflow boundaries

Research checked on 2026-10-09. An ordinary chat transport does not prove that a
provider can safely execute a managed workflow attempt. The required boundary is
a fresh native harness, a host-owned tool catalog, captured callback authority,
disabled independent fanout/configuration, and verified process-group shutdown.
Provider authentication must remain owned by the configured native harness.

## Cursor: official SDK, separate authentication admission

The [official TypeScript SDK](https://cursor.com/docs/sdk/typescript) runs Cursor's
native agent locally and accepts in-process custom tools. The adapter pins
[`@cursor/sdk` 1.0.37](https://registry.npmjs.org/@cursor/sdk/1.0.37), including its
platform packages. Its published declarations and bundled implementation were
inspected, rather than assuming that ACP and SDK have identical configuration.

`tools: ["mcp"]` enables custom tools while excluding built-in read/edit/shell and
other native tools. `disallowedTools: ["task"]` explicitly prohibits native
subagents. Empty `mcpServers`, `agents`, and `local.settingSources` exclude ambient
project/user/team/MDM/plugin MCP, hooks, skills, and rules. Empty `tools` would
also disable custom MCP tools; it is not the correct configuration. Restrictions
are not persisted across resume, so the adapter always creates a fresh agent.

The SDK checks tool names and sends the resolved allowlist/exclusions on every
model iteration. There is no public pre-sampling effective-catalog API. Readiness
therefore attests the pinned, source-reviewed options and the exact registered
callback catalog; it does not pretend to inspect a catalog returned by a sampled
model. SDK callback session/tool-call IDs never choose host authority.

Authentication is `CURSOR_API_KEY` or an already-existing SDK login owned by
`Cursor.auth` (`~/.cursor/sdk/auth.json`). The SDK documents its own saved-login
flow. It does **not** document importing ordinary CLI OAuth credentials, and this
integration neither copies CLI credentials nor triggers login. Ordinary
[Cursor ACP](https://cursor.com/docs/cli/acp) sessions keep their current behavior.
An ordinary CLI login alone does not establish managed SDK readiness.

The SDK's token totals are not an exact bill: plan inclusions, discounts and fees
affect charges, and billed cost can arrive later. Unreported cost remains unknown.
The flat Bun binary must ship official native platform assets beside itself;
there is no installation on the user's machine at workflow runtime.

## OpenCode: isolate configuration and check effective permissions

Primary references: [configuration](https://opencode.ai/docs/config/),
[tools](https://opencode.ai/docs/tools/),
[configuration implementation](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/src/config/config.ts),
[permission implementation](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/src/permission/index.ts),
and [tool registration](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/src/tool/registry.ts).

`OPENCODE_CONFIG_CONTENT="{}"` is an overlay, not a way to erase user/project
configuration. Source layers merge before inline content and managed organization
configuration can merge afterward. `OPENCODE_DISABLE_PROJECT_CONFIG` suppresses
project discovery; `--pure` suppresses external plugins. These do not by themselves
prove an empty tool surface: tool-directory discovery is a separate registry path.

The implemented adapter pins native 1.18.35 and starts a dedicated authenticated
loopback server inside the owned Linux process group. It gives that process
private XDG configuration/data/cache/state and temporary directories, redirects
home discovery with the version-reviewed `OPENCODE_TEST_HOME`, clears inherited
OpenCode overlays and instrumentation, excludes project/Claude/skill discovery,
and disables external/default plugins. The owned empty global configuration
directory is read-only: the native [package installer](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/core/src/npm.ts)
checks writability and skips installing dependencies there. Actual readiness
confirmed the directory stayed empty, avoiding per-attempt package installation.
Existing administrator configuration causes
admission to fail; it is never bypassed.

Only a private authenticated MCP endpoint exposes the captured host tool names.
The actual running server must confirm its version, effective config, selected
agent, private MCP connection and exact final **session** rules before receiving a
prompt. Session deny-all rules take precedence over the native agent's automatically
added tool-output directory exception. The exact tagged
[prompt preparation](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/src/session/llm/request.ts)
filters denied tools before sampling; checking globally registered IDs alone would
not prove this. Unexpected native child sessions fail the attempt.

This managed path currently supports explicit built-in `provider/model` routes
with environment API-key authentication. It keeps all native credential stores
untouched and does not import OAuth/custom-plugin routing into the clean process.
The ordinary shared-server chat integration remains available separately. A real
native readiness check confirmed private MCP plus final session permissions using
a dummy key and **zero model turns**. Fake HTTP tests cover admission failures,
scoped callbacks, token normalization, unknown billing, native-child rejection and request bounds.
Native `info.cost` is computed from catalogue token rates, as the tagged
[usage calculation](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/src/session/session.ts)
shows; it is not treated as a provider invoice. Observed tokens are recorded and
unreported billing stays unknown. The [official HTTP server API](https://opencode.ai/docs/server/) provides the
configuration/session endpoints. Prepared mise installations are resolved with
read-only `mise which`, disabled auto-install/environment/hooks; shell wrappers
that run `mise use -g` are not executed. Explicit `CODEMUX_OPENCODE_BINARY` can
select the prepared binary. [Mise settings](https://mise.jdx.dev/configuration/settings.html)
document its auto-install suppression. Compatibility with other native versions remains
fail-closed until reviewed and tested.

## Hermes: ordinary ACP needs a separate controlled wrapper

Primary references: [ACP](https://hermes-agent.nousresearch.com/docs/user-guide/features/acp),
[toolsets](https://hermes-agent.nousresearch.com/docs/reference/toolsets-reference),
[plugins](https://hermes-agent.nousresearch.com/docs/user-guide/features/plugins),
[configuration](https://hermes-agent.nousresearch.com/docs/user-guide/configuration),
and the [official ACP server source](https://github.com/NousResearch/hermes-agent/blob/main/acp_adapter/server.py).
The local source reviewed was `8b66a51036c1e20920a17cdd049fdf55c968d683`.

ACP defaults include terminal/files, delegation, execution, memory and skills.
`platform_toolsets.acp` and disabled-toolsets options alter the initial set, but
configured MCP and enabled plugins can add tools. ACP session MCP registration
refreshes definitions and can inject memory-provider tools afterward. Ordinary ACP
does not acknowledge a verified effective catalog before a prompt.

The existing CodeMux profile runtime shares one process; closing a routed session
does not prove that process or its work has stopped. A managed path must create one
isolated process per attempt, resolve the selected native profile/provider through
Hermes' own runtime resolver, and strictly register only host tools before entering
the agent loop. An explicitly empty enabled-toolsets list is different from `None`
(all tools). Plugin, hook, memory, skill, delegation and execution injection must
remain disabled at both definition and dispatch boundaries. A wrapper must reject
silent provider fallback and remain version/API gated; exposing a toolset toggle
alone does not meet the workflow boundary.

The implemented managed wrapper uses the official
[Python library](https://hermes-agent.nousresearch.com/docs/guides/python-library/)
and [tool registry](https://hermes-agent.nousresearch.com/docs/developer-guide/tools-runtime/)
in a fresh prepared process. Native routing config and API-key dotenv fields are
read without interpolation; only allowlisted route fields enter an owned clean
home. Hermes' resolver runs there, because its ordinary resolver can write config
backups, sanitize dotenv files and refresh authentication. Public audit hooks
reject subprocess/installer launch and personal/source file mutations. Exact
registered schemas are checked before inference and again at dispatch.

Explicit prepared source and Python 3.14 venv paths are required; CodeMux performs
no installation or profile repair. Direct supported API-key/local routes are
accepted; OAuth, pools, named providers, plugins, fallback models and external
process APIs are rejected. Token-free fake-library/RPC regressions pass. The local
installed Python 3.11 runtime was correctly refused, so **no native Hermes model
turn is claimed**. See [setup requirements](../hermes-provider.md).

## Grok: CLI flags do not establish stdio tool isolation

Primary references: [CLI reference](https://docs.x.ai/build/cli/reference),
[settings](https://docs.x.ai/build/settings),
[permissions](https://docs.x.ai/build/features/permissions),
[MCP](https://docs.x.ai/build/features/mcp-servers), and
[official source](https://github.com/xai-org/grok-build).
Relevant source is the
[CLI dispatcher](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager-bin/src/main.rs),
[agent builder](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-agent/src/builder.rs),
[agent configuration](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-agent/src/config.rs),
and [configuration guide](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/05-configuration.md).
The installed native binary reported 1.0.50.

Root CLI flags include tools/exclusions, no-subagents/no-plan/no-memory and web
search disabling. The stdio-agent dispatcher does not forward the root tool and
subagent selections. A safe stdio path therefore needs a reviewed agent profile,
`--no-leader`, owned `GROK_HOME`, disabled native workflows, and explicit native
authentication; placing root flags before `agent stdio` is insufficient.

Further traps in the official builder:

- Empty profile `tools` inherits the full toolset.
- Unknown allowlist names can fall back to the full toolset; inventing a managed
  tool name must never be treated as an allowlist success.
- Native MCP entries expand to search/use-tool helpers, requiring separate proof
  that discovery and dispatch expose only the private host MCP server.
- Hosted web/x-search tools are distinct from the function catalog.
- `GROK_CONFIG` overlays only allowed soft settings; it cannot erase arbitrary
  ambient plugin/MCP/hook discovery. Project and compatibility sources require
  separate controls, and enterprise configuration must be respected.
- ACP `_meta.toolOverrides` acknowledges search policy, not a complete catalog.
  `tool_definitions.json` is written at model sampling and does not establish a
  pre-prompt catalog, nor include hosted tools.

No official local Grok Build agent SDK was found during this review. The official
raw xAI inference SDK is not the configured Grok Build harness. Until a native
adapter proves these catalog/discovery/dispatch boundaries, Grok managed execution
must remain unavailable; replacing the provider or weakening controls would be a
false compatibility claim.

## Claude and Codex: retain their native managed boundaries

Claude uses the official Agent SDK with a private captured tool catalog, empty
ambient settings, disabled native tools/fanout, and verified Linux shutdown. Its
small paid read-only canary exercised the production coordinator and two children;
it does not establish paid large-scale or mixed-provider performance.

Codex uses its native app-server, checks acknowledged startup restrictions before
inference and rejects unsupported environment/policy contracts. The
[official app-server reference](https://developers.openai.com/codex/app-server/)
distinguishes stable and experimental protocol surfaces. Version-specific native
readiness and deterministic RPC fixtures provide stronger evidence than assuming
a setting name has identical semantics in every release. Native utility tools
can remain; file, command and independent delegation authority stay restricted.

Across adapters, the host captures attempt identity before spawning, rejects
pre-readiness tool calls, treats lost events and missing spend as unknown, and
requires verified process-group plus host-tool quiescence before releasing a slot.
Passing a fake provider test proves host coordination; it does not certify a
provider's native paid runtime. Authentication and SDK/version setup must be
checked independently before inference.
