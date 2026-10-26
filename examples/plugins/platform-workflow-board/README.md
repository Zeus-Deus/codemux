# Workflow Board

Adapted from T3's Workflow Board, this package records committed finalized-run events and selected notes in durable plugin storage. Install this directory; no dependencies, install hooks or compilation are needed. Grant storage, settings, thread reads, events and UI for explicit projects. The board appears as a native page and exposes selection-saving actions.

Observations are keyed by the host's `deliveryId` before any automatic effect. At-least-once replay across worker restart does not duplicate a note, including after the visible note has been removed. The last 100 notes are retained in a paged view, with byte bounds below worker result limits; the separate delivery journal retains up to 2,000 identities. The combined state also has an 850,000-byte bound, leaving room below IPC limits. Filling it pauses observation with an actionable error rather than evicting deduplication evidence silently. Unknown stored versions are retained and rejected for repair.

Completion reviews are off by default and also require the optional `threads.send` grant. Turn completion alone never authorizes automation: the event must carry `effectsAllowed:true` from committed input provenance. The host excludes plugin-generated runs and caps effects per delivery. A follow-up request is frozen in storage before its first send. Repeated delivery after a lost send response uses the same event operation/principal and stable `completion-review` key, so the host reconciles the native receipt without a second dispatch. The saved receipt reports native command acceptance, not finished review work. Optional change signals are discovered; Refresh remains usable if they are absent.

```sh
node --test examples/plugins/platform-workflow-board/test/*.test.mjs
```

Tests run real workers with filesystem-backed host fixtures, replay after restart, opt-in/capability/provenance denials and a reply lost after native admission. Committed SQLite capture and native authorization are verified separately by platform domain tests.
