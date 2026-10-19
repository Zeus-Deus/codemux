# MCP tools fixture

`plugin.js` is a plain ESM disk plugin. Change `fixturePath` to this checkout's
`src-tauri/tests/fixtures/plugin_mcp.py`, then copy the folder as `mcp-tools`
under an **isolated test instance's** plugins directory.

The Python fixture supports `--empty --state /tmp/fixture-state`. With no state
file it exposes zero tools. Writing `ready` to that file emits
`notifications/tools/list_changed` and exposes four tools in two pages. `new_tool`
returns `ORCHID-742`, `image` returns a small PNG and a text tree with identifiers
available only in `structuredContent`, and `slow` waits 120 seconds for
cancellation tests. `--loop-cursor` tests invalid pagination.

Calling `image` with `{"includeStructuredText": true}` also includes the server's
JSON text copy, for checking that adapters do not duplicate structured data.

See the plugin guide for provider behavior and the supported host API.
