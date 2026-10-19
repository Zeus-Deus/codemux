# MCP tools fixture

`plugin.js` is a plain ESM disk plugin. Change `fixturePath` to this checkout's
`src-tauri/tests/fixtures/plugin_mcp.py`, then copy the folder as `mcp-tools`
under an **isolated test instance's** plugins directory.

The Python fixture supports `--empty --state /tmp/fixture-state`. With no state
file it exposes zero tools. Writing `ready` to that file emits
`notifications/tools/list_changed` and exposes four tools in two pages. `new_tool`
returns `ORCHID-742`, `image` returns a small PNG, and `slow` waits 120 seconds for
cancellation tests. `--loop-cursor` tests invalid pagination.

`codex-probe.py` launches the real installed `codex app-server` with an isolated
`CODEX_HOME`. It serves local MCP and Responses HTTP endpoints. The Responses
endpoint records tool names then returns a deliberate HTTP 400; this probe tests
the real app-server protocol and catalog behavior, **not** model reasoning.

Observed on Linux with `codex --version` → `codex-cli 0.162.1`:

```sh
codex app-server generate-json-schema --experimental --out /tmp/codemux-plugin-codex-schema
python docs/plugins/examples/mcp-tools/codex-probe.py
```

The generated schemas exposed `dynamicTools` on `ThreadStartParams`, absent from
`ThreadResumeParams` and `TurnStartParams`. The RPC probe's first inference
included `dynamic_before`. Supplying `dynamic_after` on `thread/resume` did not
replace it: the next inference still included `dynamic_before`.

With a CLI-configured HTTP MCP endpoint, SSE initialization and a session-id
header, the client opened `GET /mcp`. Its initial `tools/list` returned zero tools.
Sending `notifications/tools/list_changed` did not produce another `tools/list`.
`config/mcpServer/reload` returned `{}` but the next inference still lacked
`probe_new_tool`. The probe logs a summary and saves structured evidence under
`/tmp/codemux-plugin-probe/`. No provider configuration files are changed.

Separate live model/Codemux evidence is recorded when that verification runs.
Windows and macOS have not been exercised on this Linux machine.
