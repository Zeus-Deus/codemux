# Agent Output Styling Plan

- Purpose: Turn Agent Chat into a calm, final-answer-first transcript while
  preserving full process detail on demand.
- Audience: Anyone changing transcript grouping, agent activity presentation,
  chat markdown, or right-panel file previews.
- Authority: Shipped implementation record; canonical behavior lives in the
  Agent Chat feature doc and design-system reference.
- Update when: Only when correcting the historical record.
- Read next: `docs/features/agent-chat.md`,
  `docs/reference/DESIGN-SYSTEM.md`, `docs/core/STATUS.md`
- Status: SHIPPED

## Goal

Make a CodeMux agent turn feel lightweight while it is running and polished
when it settles: show only the newest mechanical work by default during a run,
fold completed process output behind one quiet turn disclosure, keep the
terminal assistant answer visible, and turn source references into file-aware
links that open the existing right-panel editor at the referenced location.

This is a presentation and navigation change. The raw transcript remains
durable and recoverable, provider behavior remains intact, and approvals,
questions, errors, plans, subagents, and workflow surfaces remain visible when
they require attention.

## Comparison Baseline

The comparison used the supplied screenshots plus the current CodeMux sources
at `07ae3bb6` and a locally inspected reference implementation.

| Area | CodeMux today | Reference behavior | Direction |
| --- | --- | --- | --- |
| Running work | Each contiguous reasoning/tool run becomes a bordered, tinted `ActivityBlock`; a settled turn can leave several large cards and commentary rows behind. | Mechanical work is a flat 12px row; only the newest work-log entry stays visible and older entries sit behind `+N previous tool calls`. | Replace card-level emphasis with a compact live work log and progressive disclosure. |
| Settled turn | A working activity card resets to a collapsed settled card, but the turn is not reduced to its final answer. | Commentary and tool work fold behind one hairline `Worked for …` row; the terminal assistant message remains visible. | Add turn-level settlement and make the final answer the dominant result. |
| Assistant framing | A 29px avatar gutter and 13–20px row gaps frame every assistant run. | Assistant prose uses the reading column directly, with restrained vertical rhythm and no repeated avatar gutter. | Remove the gutter from mechanical rows and evaluate a single, quieter attribution point for the final answer. |
| Source references | Fenced code has language/file icons and syntax highlighting, but inline paths and tool-body paths are plain text or generic inline code. | Markdown links and inline-code paths become compact file links with language icons; click opens a syntax-highlighted right-panel preview and reveals the cited line. | Add workspace-aware file-reference parsing and wire it to CodeMux's existing right-panel doc panes. |
| External URLs | Absolute web links already get safe-link handling and favicons. | Favicons are paired with a more distinctive link treatment and a full-URL tooltip. | Keep the existing safe-link/favicons pipeline; polish it after the transcript hierarchy and file links. |
| Code blocks | Streamdown + Shiki, terminal-theme colors, file-aware headers, copy, and wrap are already present. | Comparable highlighted code-block treatment. | Preserve this implementation; only make a resolvable file title openable if useful. |
| File preview foundation | The right-panel deck already supports dynamic `doc:<absolute path>` panes backed by `EditorPane`/CodeMirror, including syntax colors and rendered Markdown/images. Chat does not open it, and it has no line-reveal request. | The right panel owns a thread-scoped file surface with line reveal. | Extract a shared open action and add line/column reveal rather than building another previewer. |

## Product Decisions

1. A settled turn defaults to one `Worked…` disclosure plus its terminal
   assistant answer. Expansion restores the original chronological process;
   nothing is deleted.
2. An active turn shows assistant commentary normally, but only the newest
   routine work entry by default. Older routine entries are reachable through
   one `+N previous …` control.
3. Pending approvals, AskUserQuestion, plan decisions, active workflows,
   active subagents, errors, and user-facing runtime notices never disappear
   behind an automatic fold while actionable.
4. CodeMux keeps its single 20px `AgentOrb` as the only live-turn glyph, but
   removes the surrounding card, bold status label, shimmer, counters, and
   competing avatar treatment from the compact work row.
5. A recognized source reference opens the file in the right-panel deck by
   default and reveals its line/column when supplied. The final renderer must
   remain honest when a path is missing, outside the workspace, or ambiguous.
6. External URL and code-block systems are refinements, not rewrites. The main
   quality gain comes from turn settlement, density, and source navigation.

## Delivered Priorities

### 1. Preserve turn identity and timing

- Give every assistant-side transcript item reliable turn attribution. Stamp
  tool calls and other presently untagged mechanical items from their provider
  event instead of inferring ownership later from adjacency alone.
- Retain lightweight turn lifecycle metadata: user/start boundary, turn ID,
  completion state, completion boundary, and interrupted/error state.
- Expose the existing `agent_chat_messages.created_at` value in history rows
  and an equivalent timestamp on live persisted envelopes. Rebuild the same
  turn metadata during cold hydrate and cursor-tail hydrate.
- Use the dispatched user row as a queued turn's start boundary. If old data
  cannot support an honest duration, render `Worked` rather than manufacturing
  a time.
- Keep success completion visually silent at the item level while retaining it
  as presentation metadata.

**Acceptance:** live, resumed, queued, interrupted, and completed turns group
identically for Claude, Codex, and OpenCode; their duration never changes merely
because the pane remounted.

### 2. Derive a two-level timeline

- Replace the current single-purpose activity grouping with a pure timeline
  derivation that first groups items by turn, identifies the terminal assistant
  message, and then derives compact work rows inside the active/expanded turn.
- Add stable row shapes for `turn-fold`, `work`, `work-toggle`, and ordinary
  transcript items. Key fold state by turn ID so virtualization does not erase
  expansion.
- For a settled turn, hide all foldable commentary/reasoning/tool rows except
  the terminal assistant answer. Insert one disclosure at the first hidden row:
  `Worked for …`, `You stopped after …`, or a no-duration fallback.
- For a running turn, keep only the newest routine work row visible and place
  earlier work behind `+N previous tool calls` or `+N previous log entries`.
- Preserve original chronology when either disclosure expands. Do not fold a
  pending human decision or suppress the visible signal for a failed step.
- Treat a turn with no terminal answer explicitly: leave its error/interruption
  result visible rather than presenting an empty successful result.

**Acceptance:** settling a long turn changes the default transcript from many
process cards to one disclosure and one final answer; expanding the disclosure
reconstructs every original visible item in order.

### 3. Flatten and tighten the active-process UI

- Replace the outer `ActivityBlock`/standalone routine `ToolCallCard` chrome
  with a borderless compact work row: one 20px glyph slot, 11–12px metadata,
  one-line summary, restrained hover state, and an inline detail disclosure.
- Use a single hairline and small label for the completed-turn fold. Avoid a
  tinted background, large radius, success badge, step counter, and separate
  `Details` label.
- Remove the assistant gutter from process rows. Prototype the final answer
  both without an avatar and with one attribution mark at the start of the
  answer; choose the version that best preserves provider identity without
  reintroducing a rail.
- Normalize row spacing around commentary, work, the fold, and the final answer
  so related items read as one turn rather than a stack of cards.
- Keep expanded tool output, edit diffs, reasoning, subagent drill-ins, and
  workflow detail unchanged until opened; compactness must not reduce their
  diagnostic value.
- Respect reduced motion. The orb is the only active animation in the row.

**Acceptance:** at the supplied desktop viewport the running state has one
quiet live row and substantially less vertical chrome; the settled final answer
starts near the turn fold and reads as the primary content.

### 4. Make source references first-class navigation

- Add a tested file-reference parser for explicit Markdown links and inline
  code. Support absolute and workspace-relative paths plus `:line[:column]`
  and `#L…` forms, while rejecting URLs, versions, package identifiers, shell
  fragments, and other common false positives.
- Pass workspace ID/root context through `MessageList` → assistant markdown and
  other chat-markdown call sites. Resolve references against the active
  worktree, not the process's ambient current directory.
- Render a recognized path as a compact inline link using the existing
  `FileTypeIcon`, basename, optional disambiguating parent, and line label.
  Preserve copy/select behavior and show the resolved path in a tooltip.
- Extract the right-panel doc-opening sequence currently duplicated by the
  file tree/search surface into a shared action callable by chat.
- Add a transient reveal request to the doc pane/editor store so reopening an
  existing pane can move CodeMirror to the requested line/column without
  encoding location into the pane ID.
- Reuse the same source-link component for paths in Read/Grep/Edit tool bodies
  and, when fence metadata resolves to a real file, for code-block titles.
- Define graceful fallbacks: nonexistent or unsafe references remain
  selectable text; outside-workspace paths require an explicit supported open
  path and must never be silently remapped into the worktree.

**Acceptance:** references such as `src/components/chat/MessageList.tsx:1591`
and Markdown file links are keyboard/click accessible, open a syntax-highlighted
right-panel doc pane, and reveal the cited location.

### 5. Finish the prose/link polish and verify the whole interaction

- Retain CodeMux's favicon and safe-link transform. Add a concise full-URL
  tooltip and tune underline/hover contrast only if the final-answer visual
  pass shows it is needed.
- Tune final-answer heading, paragraph, list, inline-code, and source-link
  rhythm as one system; do not increase the general card count.
- Add dev-mock fixtures for the same long active/settled turn, including
  commentary, many tools, one failure, a pending approval, file references,
  code, and external URLs.
- Capture desktop and narrow-pane screenshots for active, newly settled,
  expanded history, file-open, and restored-session states.
- Update `docs/features/agent-chat.md` and `docs/reference/DESIGN-SYSTEM.md`
  only after the new behavior lands.

**Acceptance:** the screenshot suite demonstrates the same hierarchy before
and after remount; external links, file references, code blocks, focus rings,
keyboard disclosure, and copy behavior work in light and dark themes.

## Delivery Slices

1. **Turn model and pure derivation:** timestamps/turn attribution, lifecycle
   state, timeline row builder, reducer/hydrate tests. No visual redesign yet.
2. **Settled-turn and live-work presentation:** fold row, latest-work rule,
   flattened process rows, accessibility, virtualization/scroll tests.
3. **Source navigation:** parser, file links, shared right-panel open action,
   CodeMirror line reveal, tool-body integration.
4. **Polish and proof:** final typography/links, mock scenarios, responsive
   screenshots, canonical-doc updates, full verification.

Each slice should be independently reviewable and leave all providers usable.
Do not combine the lifecycle foundation and visual rewrite in one untestable
change.

## Verification Matrix

- Pure tests: terminal-answer selection; commentary before/between tools;
  active → settled; interrupted/error/no-answer turns; queued follow-ups;
  pending/resolved approvals; workflows/subagents; provider ordering variants.
- Hydration tests: cold replay, warm cursor tail, old rows without complete
  timing, remount while live, and restored fold duration.
- Timeline tests: only newest routine work visible, exact hidden count,
  chronological expansion, stable keys, and no actionable row auto-hidden.
- File-reference tests: POSIX/Windows paths, spaces, line/column anchors,
  duplicate basenames, URLs, semver, package names, traversal/outside-root
  behavior, missing files, and streaming partial Markdown.
- Editor tests: open new pane, reuse existing pane, repeated reveal request,
  keyboard activation, right-panel closed/open states, and remote workspace
  path resolution.
- Scroll/performance tests: settle while following the tail, settle while the
  reader is scrolled away, expansion above the viewport, 5,000-item cap, and
  no all-row rerender on token deltas.
- Visual checks: `npm run dev`, browser snapshots/interactions through
  `codemux browser`, desktop plus narrow viewport, light/dark and reduced
  motion.
- Final gate: `npm run verify` with `CARGO_BUILD_JOBS=2`.

## Open Questions

- Whether the final answer keeps one provider avatar or removes transcript
  avatars entirely. Recommendation: no mechanical-row avatars; decide the
  single final-answer mark from side-by-side screenshots.
- Whether an existing file pane should always steal right-panel focus on a
  repeated reference click. Recommendation: yes for a direct click; merely
  rendering a reference must never change panel state.
- Whether recognized files outside the active worktree may open through the
  main-area editor. Recommendation: keep them non-interactive in the first
  slice unless the workspace/remote path contract can validate them.

## Likely Touch Points

- `src/lib/agent-chat/types.ts`
- `src/lib/agent-chat/reducer.ts`
- `src/lib/agent-chat/hydrate.ts`
- `src/stores/agent-chat-store.ts`
- `src/tauri/events.ts`
- `src/tauri/commands.ts`
- `src-tauri/src/commands/agent_chat.rs`
- `src-tauri/src/database.rs`
- `src/components/chat/transcript-slots.ts`
- `src/components/chat/MessageList.tsx`
- `src/components/chat/ActivityBlock.tsx`
- `src/components/chat/ToolCallCard.tsx`
- `src/components/chat/ToolCallBodies.tsx`
- `src/components/chat/AssistantMessage.tsx`
- `src/components/chat/ChatMarkdown.tsx`
- `src/components/chat/ChatCodeBlock.tsx`
- `src/components/layout/right-panel.tsx`
- `src/components/layout/right-panel/doc-pane.tsx`
- `src/components/editor/EditorPane.tsx`
- `src/stores/editor-store.ts`
- `src/dev/tauri-mock.ts`
- `src/dev/mock-fixtures.ts`
- `docs/features/agent-chat.md`
- `docs/reference/DESIGN-SYSTEM.md`

## Already Landed

- Provider-normalized streaming items with turn IDs on assistant prose and
  reasoning, plus durable completion events.
- LegendList virtualization, stable slot reuse, send anchoring, tail following,
  and reader-owned scroll behavior.
- Expandable tool bodies, edit diffs, reasoning blocks, approvals, tasks,
  workflows, and subagent drill-ins.
- Streamdown/Shiki code rendering with language/file icons, copy, wrap, and
  terminal-theme syntax colors.
- Safe external-link handling with inline favicons and local-image previews.
- A dynamic right-panel pane deck with closable file panes backed by the
  existing syntax-highlighted editor.
- The shared monochrome `AgentOrb` and the one-orb-per-live-thing contract.

## Notes

- The useful architectural idea from the reference is the two-level timeline: collapse tool
  lifecycle noise first, then fold the whole settled process while retaining
  the final answer. Copy the behavior, not its framework-specific components.
- Keep the source transcript and persisted event order authoritative. Folding
  belongs in pure presentation derivation so export, resume, debugging, and
  provider reconciliation do not lose information.
- The largest implementation risk is scroll displacement when a live turn
  collapses. Treat the existing send-anchor and reader-owned-scroll contracts
  as invariants, not cleanup opportunities.
- The second-largest risk is false-positive file linking. A plain inline-code
  token is preferable to a confident chip that opens the wrong file.
