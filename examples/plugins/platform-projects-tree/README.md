# Projects Tree

The original T3 Projects Tree logic packaged for Codemux isolated API v1. Install this directory and grant settings, thread reads, workflow queries, native trees and change signals for selected projects. Choose the Projects tree in the native sidebar. No dependency installation or build is needed.

The backend pages `projects.list` and `threads.list` safe metadata, with independent call/byte/project/thread bounds. It never scans transcripts. Repeated/malformed cursors stop the scan and return a bounded partial tree. Rows carry native project/thread IDs, preserving the host's real row behavior. Stable hashed node IDs, cycle/depth checks and complete ancestor paths prevent malformed graph output. Pinned/live work leads, snoozed/settled groups fold, archived rows are omitted, and quiet projects can fold after the configured interval. Metadata not exposed by a native provider is not guessed into state.

Some grouping hints in T3 are absent or null on existing Codemux history. The tree consumes the available native metadata conservatively; missing hints do not trigger transcript reads or invented identity. Native fork lineage and verified plugin launches can nest within the same project/section. All output stays below 1,800 nodes and 120,000 UTF-8 bytes; unplaced threads remain available in the normal host sidebar.

```sh
node --test examples/plugins/platform-projects-tree/test/*.test.mjs
```

Tests include real worker pagination and deterministic graph/bounds cases adapted from the reference T3 example.
