# Task Launcher

An external isolated v1 package adapted from the original T3 Task Launcher's immutable proposals, verified source ownership, bounded versioned storage, per-plan serialization and conservative dispatch journal. All dependencies and the HTML view are shipped in this directory; no install hooks or build are needed.

Install this directory through Codemux plugin settings. Approve storage, thread reads, workflow queries, thread creation and UI/card/change capabilities for a selected project. Grant optional `worktrees.create` only when needed. The native `plugin_list` tool exposes the agent-callable `propose` action. A host-verified source agent can invoke it with:

```json
{"title":"Review plan","tasks":[{"title":"Inspect tests","prompt":"Review the affected tests and report missing coverage."}]}
```

The result contains a plain-text historical summary and a small card reference `{viewId:"launcher", data:{planId}}`. The native host stamps installation/generation/data owner; the author cannot forge these fields. Opening the card or thread status reads the stored plan under current verified thread/project authority. An agent without a verified source or with a different target is rejected even if input claims another caller.

A person loads the actual native model catalog, selects its advertised options and permission mode, selects normal/plan intent, then chooses the project folder or a new worktree with base ref, branch and explicit origin-fetch choice. Source model options are retained when selecting that same model. Only a `client` caller can start tasks. Neither an agent nor a background event can use the start controls. The backend also supports `startAll` with a bounded choices array; each task has an independent durable launch journal.

Before calling `threads.create`, the package persists the immutable request with its stable `launch-<taskId>` effect key and host operation ID. The native operation receipt is stored without converting accepted/running into completed work. An accepted native command is labeled **accepted**; agent work may still be running. Pending receipts can be refreshed through the host's installation/principal-scoped `workflow.query({query:"operation",operationId})` read. Missing receipt access never causes redispatch. A response lost after native admission remains **uncertain** across restarts and updates, even if a later click uses a new operation ID or session. A person can explicitly retire such a proposal as abandoned; this does not cancel/delete any native thread or make the task startable again.

Private `plans.v1` storage retains up to 12 plans of six tasks, protecting all unresolved launches from eviction. Unknown/damaged versions fail closed and retain data. Capability revocation, unavailable models/policies, invalid options and worktree preparation errors are reported without guessing a replacement. Known native-only admission recovery belongs to the host receipt ledger; this UI never replays a journal to test whether it worked.

```sh
node --test examples/plugins/platform-task-launcher/test/*.test.mjs
```

The tests spawn real source/compiled workers with deterministic durable domain fixtures, exercise lost native replies and restarts, and assert one observable create. They do not start a real provider or perform real worktree operations. Native integration coverage is owned by the platform service tests.
