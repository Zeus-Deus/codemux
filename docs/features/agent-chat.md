# Agent Chat

- Purpose: Describe the current capability and constraints of the agent-chat
  subsystem.
- Audience: Anyone working in or around the chat panel feature area.
- Authority: Canonical feature-level reality doc.
- Update when: Behavior, constraints, expectations, or major touch points
  change.
- Read next: `docs/features/multi-provider-chat.md`,
  `docs/features/skills-sync.md`, `docs/features/mcp-server.md`,
  `docs/features/workflow-orchestration.md`,
  `docs/research/step-8-attachments.md`,
  `docs/archive/step-13-beta-toggle-research.md`.

## What This Feature Is

Agent Chat is the in-app GUI surface that lets users talk to CLI coding
agents (Claude Code, Codex, OpenCode) through a conversational pane instead
of a raw terminal. It surfaces a streaming chat UX — messages, tool
approvals, plan proposals, AskUserQuestion panels, image and file
attachments, slash commands, mode pills — over subprocess-backed runners.

## Interface Gate (formerly the Step 13 Beta gate)

The feature is **ON by default** — the Agent Chat GUI was promoted out of
Beta and is now the default interface. Two persisted feature flags still
gate the entire chat surface:

- `enable_agent_chat` — gates the chat pane kind, its Tauri command surface,
  the provider registry (Claude + Codex + OpenCode), and the MCP host
  runtime.
- `enable_lazy_workspace_creation` — gates the lazy-workspace path: sidebar
  `+` and boot-into-Home open a client-side chat draft instead of eagerly
  materialising a workspace; the draft is promoted on first message send.

Both flags default to `true`. The Settings → Personal → **Interface**
section flips them together via `set_agent_chat_enabled` (see
`src/components/settings/interface-section.tsx`); turning the toggle off
returns to the classic terminal-first (CLI) interface. Either flip triggers
a plain-quit (no auto-restart) to keep user data intact across the
legacy/GUI swap. The legacy experience (preset bar, terminal panes,
empty-state splash) is byte-identical when both flags are off.

**Promotion migration**: because the whole observability snapshot is
re-saved on every mutation, pre-promotion installs carry an explicit
`enable_agent_chat: false` from the old off-default. A one-time
`agent_chat_promoted` marker (`promote_agent_chat_default` in
`src-tauri/src/observability.rs`) flips both flags on at first load after
upgrade and stamps the marker; opt-outs made after that are never
overridden. The marker is a standalone **sentinel file**
(`<data root>/agent_chat_promoted`, next to `observability.json`) rather
than only the `agent_chat_promoted` snapshot key: an older binary
deserializes the snapshot without that key and re-serializes without it,
so a downgrade → upgrade round trip would erase a snapshot-only marker,
re-run the promotion, and force-revert a deliberate opt-out. Old binaries
never touch the sentinel. The snapshot field is still honoured on read
(and mirrored into the sentinel on first sight) for installs promoted
before the sentinel existed.

See `docs/archive/step-13-beta-toggle-research.md` for the original Beta
toggle scoping and `docs/archive/step-13-ui-smoke-checklist.md` for the
operator-verified gate.

## Current Model

The chat pane stack:

- **`src-tauri/src/agent_provider/`** — `AgentProvider` trait + shared types
  (`ProviderKind`, `ProviderSession`, `ProviderRuntimeEvent`,
  `ApprovalDecision`, `ProviderError`, …). Three concrete adapters:
  `claude/`, `codex/`, `opencode/`.
- **`src-tauri/src/json_rpc_child/`** — reusable `JsonRpcChildProcess`
  helper for long-lived subprocesses that speak newline-delimited
  JSON-RPC 2.0 over stdio. Used by both Claude (against the bundled
  Bun-compiled sidecar) and Codex (against `codex app-server`).
- **`sidecar/claude-agent/`** — Bun-compiled TypeScript subprocess that
  hosts Anthropic's Claude Agent SDK. Codemux never talks to Anthropic
  directly; the sidecar is a transport-only bridge that runs SDK
  `query()` in-process and forwards messages.
- **`src/components/chat/`** — chat pane UI (redesigned on the shadcn
  chat primitives, June 2026): `AgentChatPane`, `Composer` (with `+`
  popup, `@` mention popup, slash command popup, image paste/drop),
  `ChatTranscript`, `MessageList` (LegendList virtualized transcript),
  `transcript-slots.ts` (pure turn-grouping/tool-folding slot builder),
  `AssistantAvatar`, `ReasoningBlock` (collapsible thinking),
  `ToolCallCard` + per-tool bodies, `ActivityBlock` (one folded run of
  reasoning + tool calls, working/settled — see "Activity block" below;
  `activity-steps.ts` holds its pure step/summary/duration derivations),
  `DiffView` (red/green edit surface), `TaskSummaryCard` (TodoWrite
  receipt line — see "Thread receipt" under the Agent Tasks panel),
  `StreamingMarker` (shimmer tail status), `tool-visuals.ts`
  (icon/tint mapping), `PlanProposalBlock`, `ComposerPendingInputPanel`
  for AskUserQuestion (see "AskUserQuestion panel" below),
  `PermissionRequestBlock`, `ModePill`,
  `SessionSelector`, `DraftChatSurface`, `ChatHomeLanding`,
  `DebugCleanupBanner`, `DebugExitDialog`, and the picker family under
  `src/components/chat/pickers/`. Shared primitives live in
  `src/components/ui/` (message, bubble, attachment, marker, avatar,
  spinner) and `src/components/ai-elements/` (vendored
  AI Elements — reasoning, tool, task, prompt-input, shimmer,
  message/MessageResponse, code-block — with all Vercel AI SDK type
  imports replaced by local unions; the AI SDK is NOT a dependency).
  Assistant markdown renders through **Streamdown**
  (`parseIncompleteMarkdown` keeps mid-stream fences/emphasis clean).

### AskUserQuestion panel

`ComposerPendingInputPanel` renders the first *pending* `user-input`
request as a card docked above the composer (never inline in the
transcript — `MessageList` reduces `user-input` items to a tiny marker).
Its interior is the shadcn **Questionnaire** component
(`src/components/ui/questionnaire.tsx` over `@shadcn/react/questionnaire`);
the panel keeps the composer's chat-column rails and rounded card so the
two read as one surface.

- **One question per page.** Every question is mounted at once — the
  primitive hides the inactive ones with `hidden` + `inert` — so "is Q2
  in the DOM" proves nothing; the active page is the single un-hidden
  `[data-slot=questionnaire-item]`. Paging is controlled: `qi` is the
  source of truth and drives `Root.item` / `onItemChange`, with item
  names `q-<i>` (question text repeats across questions and cannot be an
  identity).
- **`Root.items` is load-bearing.** Each entry advertises
  `{ name, required, choices: [{ value: <option label> }] }`. The
  primitive maps the numbered shortcut chips off those choice *values*
  in order, and dev builds `console.warn` on any drift from the rendered
  tree (missing item, wrong value, wrong order, required/disabled
  mismatch). A test asserts that channel stays silent.
- **Forward navigation is gated by disabled buttons**, not by the
  primitive's click-then-show-error flow: answered-ness comes from the
  panel's own pick/free-text state, so Next/Send are simply inert until
  the question is answered.
- **Keyboard works in two layers.** The primitive handles digits, arrows
  and Enter for events originating inside its form. A document-level
  listener keeps 1-9 / ArrowLeft / ArrowRight / Enter working when focus
  is outside it (nothing focused, or the composer); it bails on
  text-entry targets, on modifier chords, and on anything inside the
  questionnaire form so a keystroke is never applied twice.
- **Free text always shows.** A `QuestionnaireInput` ("Something else…")
  sits under the options on every question; typing in it answers the
  question on its own. Enter there advances or submits via an explicit
  handler that `preventDefault`s, which the primitive's form-level
  handler then declines.
- **Output contract.** `AskUserQuestionOutput.answers` is keyed by
  question *text*; single-select is the picked label (free text replaces
  it), multiSelect is labels joined with `", "` (free text appended).
  `questions` echoes the original tool_use input verbatim so the CLI can
  assemble the tool_result.
- **Dev fixture.** `npm run dev` with `?askq=1` seeds a pending
  two-question AskUserQuestion on the seeded chat thread. It arrives as a
  *live* channel event, not in the persisted transcript, because cold
  replay expires orphan pending requests (`finalizeReplay`).

### Code blocks in chat

`ChatMarkdown.tsx` is the single place chat markdown is configured.

- **Syntax highlighting** is Streamdown's Shiki `code` plugin
  (`@streamdown/code`), enabled via `plugins={{ code }}`. Token colors come
  from a theme built out of the **terminal ANSI palette**
  (`src/lib/shiki-chat-theme.ts`), whose scope map mirrors the Lezer tag map
  in `src/lib/codemirror-theme.ts` — so a keyword is the same color in chat,
  the file editor, and the terminal. Keep the two maps in sync.
- The plugin is supplied by `useChatCodePlugin()`
  (`src/hooks/use-chat-code-plugin.ts`), a **module-level store** rather than
  a per-component hook: a transcript mounts one `ChatMarkdown` per assistant
  message, so a per-hook fetch would fire one `get_current_theme` IPC call and
  one `theme-changed` listener per message. The plugin instance is memoized on
  palette identity to avoid **render churn** — a fresh plugin identity on every
  streamed keystroke would propagate through the whole markdown tree. It is
  *not* about cache preservation: Shiki's highlighter and token caches live in
  module-level maps inside `@streamdown/code`, not on the plugin instance.
- That highlighter cache is keyed by theme **name**, not content, so
  `buildChatCodeThemes` hashes the built theme into the name. A fixed name
  would serve stale colors after a terminal-theme switch. `themeId` hashes the
  **built** object (`bg`, `fg`, `tokenColors`) rather than a hand-picked list
  of `ThemeColors` fields, so every color the token map actually consumes is in
  the identity *by construction* — including ones only reachable through a
  single scope, like `color9` behind `invalid` / `invalid.illegal`. Hashing a
  curated field list is how two palettes differing solely in `color9` used to
  collide on one name and serve each other's colors.
- **Codemux owns the card shell; Streamdown owns parsing.**
  `ChatCodeBlock.tsx` supplies Streamdown's custom `code` / `inlineCode`
  components and keeps using the shared Shiki plugin for tokenization.
  Fenced cards therefore have stable Codemux markup rather than depending on
  Streamdown's private header/action layout. The custom shell renders a
  file-aware title, per-block wrap and copy actions, and the highlighted body;
  the `prose-pre:*` / `prose-code:*` chain in `ChatMarkdown.tsx` still only
  neutralizes typography defaults.
- **Line numbers are off** (`lineNumbers={false}`) — chat snippets are quotes,
  not files. Streamdown only gives each line span `display: block` as part of
  its line-number class, so `globals.css` restores the line box explicitly;
  without that rule every line collapses onto one.
- **Word wrap** is the `chat.code_wrap` setting (default off), exposed as
  Settings → Appearance → "Wrap code in chat". It supplies each block's
  default; the header's wrap action can override one block without changing
  the global preference. Off keeps lines intact behind a horizontal scroll.
- **Highlighting is stale-while-revalidating.** `highlight()` only answers
  synchronously on a cache hit, and a streaming fence changes on every token,
  so rendering "the result for exactly this code, else raw" would flash the
  whole block back to uncolored text between every append. `useHighlightedCode`
  keeps the last result on screen and appends the not-yet-tokenized tail
  uncolored, so neither color nor text lags the stream. Reuse is guarded on the
  block's own state, an unchanged language, and the old code still being a
  prefix of the new — a fence that switches language or is rewritten falls back
  to raw rather than painting one language's colors onto another's source.
- Fence metadata comes from Streamdown's own `codeMeta` plugin, composed out of
  its exported `defaultRemarkPlugins` (passing `remarkPlugins` *replaces* the
  defaults, so `gfm` and `codeMeta` are named explicitly rather than
  reimplemented locally). `title=`, `file=`, `filename=`, and bare
  filename/path forms render a file icon plus title. A bare token only counts
  as a filename when it ends in a real-looking extension and is not
  version-like, so ` ```txt 1.5 ` and ` ```js v2.0 ` stay plain fences while
  `main.rs`, `.env.local`, and `@scope/pkg/file.tsx` still caption. Without a
  title, common language ids map to a synthetic filename and the existing
  `material-file-icons` system supplies the language/framework icon; unknown
  languages fall back to a text label.
- The fence body keeps whatever the author wrote: mdast terminates a fence with
  exactly one newline, and only that one is stripped, so a snippet genuinely
  ending in blank lines keeps them in both the render and the clipboard.

Card styling (all in `globals.css`, all on design tokens):

- A fence with **no language** is labeled `text`, so copy and wrap remain
  discoverable and the header never becomes an unexplained empty strip.
- A fence still waiting on its closing ``` carries `data-incomplete` and dims
  its title, so a card that is still filling in (and whose copy action would
  only capture the partial snippet) reads as provisional. Opacity only — no
  motion, since a transcript can stream several blocks at once.
- Wrap/copy actions remain visible in the header. They use the shared compact
  ghost-button treatment and tooltips, with `aria-pressed` on wrap and a
  temporary copied state.
- The card uses a 12px radius, a quiet header/body divider, and distinct token
  surfaces. Code remains 12px JetBrains Mono with compact 1.4 line-height and
  more body padding than the preceding flattened Streamdown card.

### Rich external links in chat

Assistant and plan markdown renders absolute `http(s)` links with the
destination site's favicon inline. The markdown label remains authoritative —
so the same treatment works for `PR #235`, issue labels, documentation names,
and bare URLs instead of relying on GitHub-specific parsing. Bare URLs keep the
favicon attached to their protocol; labelled links keep it attached to the
first character, preventing an icon from wrapping onto a line by itself.

`rehypeRichExternalLinks` (`src/lib/agent-chat/rich-links.ts`) decorates the HAST
children rather than replacing Streamdown's anchor component. This preserves
Streamdown's safe-link confirmation behavior. `MarkdownLinkFavicon` requests a
32px image from the same Google favicon service already used for project
avatars and falls back to a token-colored globe.

Three cases render that globe with no network request at all: while the
message is still streaming (a bare URL is autolinked on every frame, so
in-flight prefixes like `docs.p` would each fire a lookup), for hostnames that
are not publicly resolvable (`localhost`, single-label names, `.local`/
`.internal`/`.lan`/`.home`/`.corp` suffixes, and private or reserved IP
literals — those names are never sent to the third-party favicon service), and
for hosts whose icon recently failed to load (a five-minute TTL, capped cache).
Fragment, relative, `mailto:`, `javascript:`, and other non-web links are left
unchanged. The dev mock's rich transcript includes both GitHub and docs
examples for visual verification.

### Local screenshot links in chat

Assistant and plan Markdown upgrades absolute local image destinations into a
visual proof card. Both the form agents most often produce —
`[Terminal screenshot](/absolute/path.png)` — and standard image syntax —
`![Terminal screenshot](/absolute/path.png)` — render an inline, labelled
preview instead of a dead underlined filesystem link. Clicking the card opens
a near-fullscreen lightbox; a missing or reaped temp file becomes a stable
"Image unavailable" card rather than a broken-image glyph. PNG, JPEG, GIF, and
WebP are accepted on POSIX paths, Windows drive paths, and local `file://` URLs.
Relative paths stay ordinary Markdown because a chat message has no durable
document directory.

`rehypeLocalImageLinks` (`src/lib/agent-chat/local-image-links.ts`) performs the
narrow HAST upgrade and `MarkdownLocalImage` owns the card/lightbox. Desktop
WebKit loads the absolute path through Tauri's asset protocol. If that fails in
the browser dev mock or a web-remote client, `agent_chat_read_local_image`
returns only an existing, absolute, supported image with the same 25 MB ceiling
as chat attachments; the frontend caches a bounded blob URL. This is separate
from `agent_chat_read_image`, whose containment check remains restricted to
Codemux's persisted chat-image root. Ordinary local non-image links are not
upgraded. The rich dev transcript includes a local screenshot link so the card
and lightbox stay visually testable.

## What Works Today

- **Three end-to-end providers** behind one unified picker:
  - **Claude** — Claude Agent SDK via the Bun-compiled `claude-agent`
    sidecar (JSON-RPC over stdio).
  - **Codex** — `codex app-server` subprocess, JSON-RPC over stdio.
  - **OpenCode** — Rust-direct HTTP against a managed `opencode serve`
    child (`kill_on_drop`, generated `OPENCODE_SERVER_PASSWORD`).
  - All three render in a single 2-column picker (provider rail + searchable
    model list); favorites persist via zustand + `localStorage`.
  - **Model rows show the resolved version + blurb** (like the terminal
    `/model` picker): when `ChatModelInfo.description` is present the row
    subtitle renders `<driver> · <description>` (truncated, full text in a
    `title` tooltip), e.g. `Claude · Opus 4.8 with 1M context · Best for
    everyday, complex tasks`; the trigger pill mirrors label + description
    into its tooltip.
  - **Row-jump shortcuts.** While the popover is open, `Ctrl+1..9`
    (`Cmd+1..9` on macOS) activates the Nth row of the *filtered* list. The
    capture-phase `keydown` listener is registered only while the popover is
    open and calls `preventDefault` + `stopPropagation`, so the global
    tab/workspace bindings that own the same chord are unaffected — the
    collision was resolved by scoping, not by rebinding. Each row renders its
    own kbd chip (`JUMP_MOD_LABEL`).
  - **Alias folding + label promotion (Claude).** Alias rows
    (`default`/`opus`/`fable`/`sonnet`/`haiku`) and full-id rows whose label
    is a nickname now render **promoted concrete names** ("Claude Opus 4.8"),
    parsed from the live version-bearing description with the maintained
    catalog as fallback. The `default` row itself is folded away by
    `dedupe_default_alias` whenever a concrete twin exists: the twin absorbs
    it, moves to index 0, and its description takes a `Recommended · ` prefix
    — so the lead row reads e.g. label `Claude Opus 4.8`, subtitle
    `Claude · Recommended · Best for everyday, complex tasks`, not the
    worked example above. For persistence compat, `selectModel`
    (`src/stores/provider-capabilities-store.ts`) deliberately breaks the
    strict-id-match rule for the literal id `"default"` and falls back to
    `models[0]`, so drafts and threads persisted before the fold still
    resolve a real model — which is what keeps their trigger label, tooltip,
    active-row highlight, and reasoning/speed picker availability correct. Backend guarantees every Claude row carries a
    description: SDK-provided string verbatim when the CLI supplies one →
    maintained blurb for known full ids (blurb-only; the label already
    carries the version) → alias backfill (`ALIAS_CANONICAL_IDS` in
    `src-tauri/src/agent_provider/claude/capabilities.rs` maps
    `default`/`opus`/`fable`/`sonnet`/`haiku` to canonical maintained ids and
    synthesizes `<Version>[ with 1M context] · <blurb>`). Model *selection*
    itself passes the picked id through unchanged; alias ids are resolved to
    the latest concrete model by the Claude CLI/SDK, not by Codemux.
- **Streaming chat UX**: Streamdown-rendered messages, tool approvals
  (per-tool body rendering), collapsible reasoning blocks (thinking
  deltas reduce into a `reasoning` ChatViewItem that seals on any
  non-thinking boundary and finalizes with a "Thought for Ns"
  duration), **Activity blocks** (one folded run of reasoning + tool
  calls — see "Activity block" below), red/green diff cards for edits,
  TodoWrite task checklists, plan proposals (`ExitPlanMode`),
  AskUserQuestion panels, shimmer streaming marker, debug-mode banner
  + exit dialog.
- **Mode pills**: Ask / Allow always / Plan / Debug, with Shift+Tab cycling
  and silent-restart on pill removal.
- **Capability-gated speed picker**: models that advertise
  `supports_fast_mode` show an explicit Standard / Fast picker beside the
  reasoning control. Fast is a premium-usage choice, persists per thread,
  and takes effect through a transcript-preserving silent restart. Codex
  launches/resumes the app-server thread with `serviceTier: "fast"` (or
  explicit `null` for Standard, so a global fast default cannot override
  the composer). Switching to a model without the capability, including
  after a capability-catalog change, heals the thread back to Standard.
  OpenCode does not advertise the capability today. **Claude no longer
  advertises it either**: `merge_sdk_with_maintained`
  (`agent_provider/claude/capabilities.rs`) clamps `supports_fast_mode`
  to `false` even when the SDK reports `supportsFastMode` (observed on
  Opus 5). The SDK flag is a capability advertisement, not an entitlement
  check — on accounts without Extra Usage the server silently serves
  `usage.speed: "standard"` while the pill claims Fast, so the picker is
  withheld until an entitlement-feedback loop exists (re-enable path in
  `docs/research/opus-5-agent-chat-support.md`; the sidecar's
  `fastModePerSessionOptIn` plumbing remains in place, unused).
- **Attachments** via `+` and `@`: files, folders, GitHub issues + PRs,
  images via paste / drop / picker. Inline chips, send-time injection,
  expand, caps, gif guard, chip tooltips. See
  `docs/research/step-8-attachments.md`.
  - **Image chips show the image, not a generic icon.** `AttachmentChip`
    builds an object URL from the attachment's in-memory
    `resolvedImage.bytes` (already kept for the optimistic user bubble,
    so this costs no IPC) and renders a 30×22px thumbnail in the icon
    slot; the chip swaps from the pill radius to `rounded-md` with a 3px
    inset so the preview reads as a picture, and the hover tooltip adds a
    ≤260×180px preview above the filename/token lines. Two pasted
    screenshots were previously indistinguishable — both chips read
    `pasted-image.png` behind the same icon. The URL is revoked on
    unmount; missing bytes, an image that fails to decode, or an
    environment without `URL.createObjectURL` (jsdom) all fall back to
    the original icon pill.
  - **Ctrl+V image paste has a Linux clipboard fallback.** WebKit2GTK
    strips image payloads from the JS `paste` event, so `Composer`'s
    handler tries `e.clipboardData.items` first (works in the browser
    dev mock / Chrome) and, when that yields no image, falls back to the
    `paste_clipboard_image` Rust command (reads the OS clipboard
    server-side, re-encodes PNG via the shared `encode_rgba_to_png`,
    returns `{ bytes, mime }`). Composer wraps the bytes in a `File` and
    hands them to the same `handleAttachImage` (`AgentChatPane`) the `+`
    picker uses, so validation/staging are identical. This mirrors the
    new-workspace dialog's `paste_clipboard_image_to_file`, but returns
    bytes (not a temp-file path) because the chat stages bytes in
    memory. A text-only clipboard rejects the command → default paste
    runs untouched.
  - **Images stage to disk at ATTACH time, not send time.** The moment
    an image chip's bytes resolve, `beginImageStaging`
    (`src/lib/agent-chat/image-staging.ts`) uploads them via the
    raw-body `agent_chat_stage_image` command (bytes as the invoke's
    raw request payload, MIME on an `x-media-type` header — the fs
    plugin `write_file` mechanism; a base64-JSON body fallback covers
    the web-remote transport, which can't carry raw invoke bodies) into
    `<config>/codemux/agent-chat/images/staging/`. The chip gains
    `stagedImage: { path, mediaType }`; the send then passes only
    `images: [{ path, media_type }]` refs on `agent_chat_send_turn`,
    and the backend `finalize_chat_images` promotes staged files into
    the thread's dir (rename) and reads the bytes for the provider.
    Bytes therefore NEVER cross IPC as a JSON `number[]` — that path
    (≈4 bytes of JSON per image byte, minutes of stall for a few
    screenshots) is deleted; a chip whose staging failed blocks the
    send with a toast instead of silently falling back. Removed chips
    fire `agent_chat_discard_staged_image` (staging-dir-contained
    delete); leaked staging files are reaped by a 24h-grace startup
    sweep (`sweep_stale_staged_images`).
  - **Attached images render in the sent bubble.** On send,
    the in-memory image bytes are also turned into `data:` URLs
    (`buildImageDisplaySources`) and stamped onto the optimistic
    `UserMessageItem.images` — so the sent turn shows its own thumbnails,
    not just the provider. The persisted `user_message` envelope gains an
    optional `images: [{ path, media_type }]` (absolute fs paths written
    backend-side); on hydrate those map onto `images[].src`.
    `UserMessage` renders a wrapping thumbnail row (rounded, bordered,
    ≤160px, broken-image fallback) with click-to-open lightbox
    (`Dialog`, Esc / click-outside). `resolveAssetSrc` normalises both
    forms: `data:` URLs (fresh optimistic send) pass through untouched,
    absolute paths (hydrated) go through `convertFileSrc`. Serving those
    converted paths requires the asset-protocol scope to match
    dot-directories (`~/.config/...`) — `tauri.conf.json` sets
    `assetProtocol.scope: { allow: ["**"], requireLiteralLeadingDot:
    false }`, since Unix glob scopes skip leading-dot components by
    default (this was the "every hydrated thumbnail is a broken-image
    icon" bug). Belt-and-braces: when an fs-path `<img>` still errors
    (dev browser mock, web-remote client — no asset protocol), a
    `useImageWithFallback` hook reads the bytes back over IPC
    (`agent_chat_read_image`, path validated inside the chat-image
    root) and renders a cached blob URL before surfacing the
    placeholder.
  - **Agent-produced images render inline (not just user attachments).**
    When a tool result carries an image — a screenshot from `Read` on a
    PNG, or any browser/screenshot tool — the block is already forwarded
    to the client verbatim (tool-result `content` is opaque JSON on the
    Rust side, so nothing drops it). The renderers just never turned it
    into an `<img>`: every tool body funnels `result_content` through a
    `contentToString` helper whose only special case was `{text}`, so an
    image block was `JSON.stringify`-ed into a base64 text dump. Fix is
    frontend-only: `extractToolResultImages`
    (`src/lib/agent-chat/tool-result-images.ts`) normalises the two wire
    shapes — the Anthropic `{type:"image", source:{type:"base64",
    media_type, data}}` block and the `{type:"image", source:{type:"url",
    …}}` / OpenAI `{type:"image_url", …}` variants — into a renderable
    `data:`/http `src` (rejecting non-image media types like PDFs and
    unsafe URL schemes). `ToolCallBody` (`ToolCallBodies.tsx`) renders
    those below the per-tool body as a thumbnail row + click-to-open
    lightbox (`ToolResultImages.tsx`, mirroring the `UserMessage`
    attachment lightbox), and both `contentToString` copies filter only
    successfully normalised blocks (`isRenderableImageBlock`) so the same
    payload never double-renders as base64 text; rejected PDF, malformed,
    or unsafe-URL blocks remain visible in the raw fallback instead of
    disappearing. `ToolCallCard` auto-expands when a result first gains a
    renderable image (`hasToolResultImages`) so asynchronously arriving
    screenshots become visible without a manual expand, while a later user
    collapse remains respected. Standalone provider image blocks (not wrapped
    in a tool result) are still unsupported — they would need a new backend
    `CompletedItem` variant. Absolute image paths written into assistant
    Markdown are supported separately by the local screenshot-link renderer
    above. The dev mock seeds
    an image `Read` tool result in the demo transcript
    (`richChatTurnEnvelopes`).
- **Slash command popup** with cross-provider parsing. Five groups:
  - **MODES** — `/plan`, `/ask`, `/debug` (state-only mode-pill
    activation, typed text stripped) plus `/default` (returns to
    normal build mode; only listed while a non-default mode is
    active). Built in `buildModeCommands`
    (`src/lib/agent-chat/slash-commands.ts`).
  - **WORKFLOWS** — `/workflow` (Claude-gated; inserts literal text).
  - **SETTINGS** — `/model` (GUI-local: strips the typed text and pops
    the footer's model picker via an incrementing `openSignal` prop on
    `ModelPicker` / `MultiProviderModelPicker`; the composer skips its
    usual textarea refocus for this pick so the popover isn't
    dismissed by focus-outside). Built in `buildModelCommand`.
  - **SKILLS** — dynamic, from the skills registry. Every discovered
    portable skill is offered when its projection says the active provider can
    use it. Unique names keep `/name`; collisions get an exact qualified token
    such as `/claude:user:deploy` or `/codex:project:deploy` (with a short id
    suffix only when provider + scope + name also collide). `skill-tokens.ts`
    resolves that token to a stable skill id and removes only the recognized
    Codemux token before send. The backend re-resolves the id against the
    thread's current cwd inventory, so the popup selection cannot drift into a
    different same-named body or carry an arbitrary frontend-supplied path.
    Native-only entries stay visible in Settings but are omitted from foreign
    provider popups with their reason.
  - **COMMANDS** — **provider-native slash commands, discovered live**
    (never hardcoded). The claude-agent sidecar's `list-commands`
    JSON-RPC method opens a transient SDK `query()` (same lifecycle as
    `list-models`, but with `settingSources: ["user","project","local"]`)
    and returns `supportedCommands()` — the deployed CLI's built-ins
    (`/compact`, `/clear`, `/init`, `/review`, `/context`, …) plus
    custom `~/.claude/commands` and `<cwd>/.claude/commands` entries.
    Rust side: `agent_provider/claude/slash_commands.rs`
    (`ClaudeSlashCommandCache`, per-cwd, app-lifetime, errors not
    cached) behind the `list_chat_slash_commands` Tauri command
    (Codex/OpenCode resolve to an empty list — no discovery surface).
    `dedupe_commands` also drops commands the CLI marks `isHidden` but
    the SDK reports anyway (`is_internal_command`): any `__`-prefixed
    name, plus an exact-match denylist of `heapdump`,
    `workflow-launch-exec`, `design-consent`, `design-revoke`,
    `extra-usage`, `pro-trial-expired`, `rate-limit-options`. Each is
    unusable from a Codemux thread — a debug affordance, a renamed
    alias, or gated on a session kind Codemux never creates (the
    workflow entries describe their own gate: "server-launched
    sessions only"). Against the deployed CLI this trims 54 discovered
    commands to 48. The filter is menu-only: a hand-typed name is still
    forwarded verbatim.
    Frontend: `provider-commands-store.ts` (lazy load on first popup
    open, 60s TTL, keyed `(provider, cwd)`), `buildProviderCommands`
    (case-preserving map + reserved-name filter: collisions with
    modes / `/workflow` / `/model` / skill names are dropped so the
    local behaviour wins). Selecting a provider command inserts the
    literal `/name ` text (same mechanics as skills/`/workflow`); the
    text is forwarded verbatim and the provider interprets the leading
    slash itself — Codemux never executes provider commands locally.

    **Why the CLI's interactive commands are absent, and should stay
    absent.** The CLI registers commands as `local`, `local-jsx`, or
    `prompt`. The `local-jsx` ones render an ink/TUI surface and exist
    only in an interactive terminal — `/remote-control` (alias `/rc`),
    `/login`, `/resume`, `/rewind`, `/terminal-setup`, `/theme`,
    `/statusline`, … The Agent SDK does not report them, and forwarding
    one anyway returns `"<name> isn't available in this environment."`
    Listing them would only offer rows that cannot work. Verified two
    ways: `initializationResult().commands` and `supportedCommands()`
    return the *identical* 54-command list (diffed, zero difference),
    so switching probe APIs would change nothing. Remote control does
    surface in the init result, but as session capability flags
    (`remote_control_auto_enable`, `remote_control_auto_on_by_default`,
    `ide_rc_auto_enable_gate`) rather than a command — deliberately not
    consumed, since Codemux ships its own web remote access
    (`docs/features/web-remote-access.md`).
- **Cross-provider skill system**: provider-aware inventory, exact explicit
  invocation, watcher, conflicts, per-row Codemux availability, and
  per-target compatibility. Server-side sync remains a separate filesystem
  concern (see `docs/features/skills-sync.md`).
  - `SkillInventoryService` merges readable filesystem definitions with the
    cwd-scoped Codex `skills/list` and OpenCode `GET /skill?directory=...`
    catalogs. Adapter errors are isolated and returned beside successful
    results; a provider binary that is not installed is a normal empty catalog,
    not an error banner. Claude's SDK command list is kept separate because its
    current response does not prove stable skill provenance; readable Claude
    files remain portable while unmatched SDK entries stay provider-native
    commands.
  - Project discovery walks cwd ancestors for portable definitions, and the
    filesystem watcher covers those inherited roots as well as the direct cwd.
    Invalid legacy names remain visible as unavailable Settings rows with the
    validation reason instead of disappearing from discovery. Canonical
    SKILL.md paths remain the strongest identity; a provider-catalog identity
    is used for native-only entries. The provider catalog owns its native
    enabled state.
  - Every definition carries a projection for Claude, Codex, and OpenCode:
    native, explicit-portable, native-only, or unavailable, plus compatibility
    reasons and the invocation route. Compatibility is no longer calculated
    once against a hard-coded Claude target.
  - Sends carry stable `skill_ids`, the token-stripped user text, and the actual
    `include_plugins` preference over IPC. The backend revalidates them at the
    persisted thread cwd under that same discovery scope. A cold-cache forced
    inventory is reused for resolution rather than harvesting both catalogs a
    second time. Codex receives structured `{type:"skill",
    name,path}` items for readable selections; one native Claude selection
    keeps `/name` command semantics only when mode/effort/attachment framing
    has not changed its arguments, otherwise it uses the portable envelope so
    wrappers do not become the command's `$ARGUMENTS`. OpenCode and other
    foreign portable cases receive a normalized envelope containing body,
    source provenance, and the absolute base directory for relative `scripts/`,
    `references/`, and `assets/`. Source-specific frontmatter never becomes a
    permission grant.
    The durable/optimistic transcript uses `display_text`, so it shows the
    user's unexpanded command rather than an injected body.
  - Provider-native automatic invocation is unchanged. The Settings switch is
    explicitly “available in Codemux”: disabling a row removes it from the
    Codemux popup and resolver but does not rewrite provider configuration or
    hide the same skill from its native provider.
    Pre-inventory path-hash disabled ids migrate opportunistically to the
    canonical-repository preference id when the same project skill is next
    discovered, so upgrades do not re-enable it.
  - **Scan roots** (`skills::paths::enumerate_scan_paths` — the single
    source of truth; scanner, watcher, conflict detection and the
    Settings grouping all derive from it, so adding a provider root is
    a one-line change there):
    - *User*: `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`,
      `~/.opencode/skills`, `~/.config/opencode/skills`,
      `~/.codemux/skills`
    - *Project*: `<root>/.claude/skills`, `<root>/.codex/skills`,
      `<root>/.agents/skills`, `<root>/.opencode/skills`,
      `<root>/.codemux/skills`
    - *Plugin* (only when `include_plugins`):
      `~/.claude/plugins/marketplaces/*/plugins/*/skills`,
      `~/.claude/plugins/external_plugins/*/skills`
  - Codex reads **both** `.codex/skills` and `.agents/skills` at each
    scope; `.agents` is the newer convention and is frequently a
    symlink to the older one.
  - `~/.codex/skills/.system/` is excluded (Codex built-ins, not user
    content) — see `is_codex_system_dir`.
  - **Roots are deduped by canonical path** before scanning. Several of
    these roots are commonly symlinked to each other, and walking both
    sides of an alias would surface every skill twice — which the
    name-based `detectConflicts` would then report as a conflict. The
    first root to claim a directory wins, so enumeration order encodes
    precedence (user before project; canonical location before alias).
- **MCP host runtime** (Step 9): Codemux discovers user-installed MCP
  servers across Codemux / Claude / Cursor paths, spawns each child once,
  exposes tools to the Claude SDK via an in-process facade with dynamic
  `setMcpServers` refresh. Settings panel + composer `+` popup surface
  enable/disable + status badges + tool list modal + 50-tool cap warning.
  See `docs/features/mcp-server.md`.
- **Permissions settings page** with per-tool body rendering and
  `AllowAlways` rule persistence.
- **Session lifecycle**: transcripts persist + replay on session resume (by
  durable row-id cursor — see § "Hydration by cursor (warm resume)");
  session history selector; permission-mode mid-session restart;
  pane-scoped chats; new-tab preset launch; base-branch picker; stop-click
  restarts the session so the next turn works.
- **Restart auto-resume + persisted picker config**: a chat pane reopened
  after an app quit resumes transparently. The provider session map is
  empty on every launch (no startup rehydration), so the backend
  `ensure_live_session` choke point in `commands/agent_chat.rs` rebuilds a
  dead session from its `agent_chat_sessions` row on the next
  `agent_chat_send_turn` — reusing the
  SAME `thread_id` (keeping the attached Channel, pane snapshot, and store
  slice valid) and passing `resume_cursor: {"resume": sdk_session_id}`
  when the row carries one. Codex's adapter returns its provider-native
  `{"threadId": ...}` cursor at session start; the command layer now persists
  that id, and the adapter accepts the database/frontend's generic `resume`
  wrapper before emitting the official `thread/resume { threadId }` RPC. This
  keeps the prior Codex thread's complete model-visible history, including
  image inputs, available when the user clicks **Continue run** after an app
  restart; the Continue turn itself remains a plain follow-up and does not
  duplicate the prior attachments. The adapters fall back to a fresh session,
  whose transcript still hydrates from the DB, when no durable provider id
  exists or when the resume-start fails. Each thread's picker config (`model`,
  `effort`, `context_window`, `permission_mode`, `fast_mode`) is persisted on the session row —
  written at `agent_chat_start_session`, updated fire-and-forget by the
  picker handlers via `agent_chat_update_session_config`, always persisted
  by `agent_chat_set_model` / `agent_chat_set_permission_mode` (which now
  swallow a `SessionNotFound` from the dead-session apply into `Ok`, so
  the value takes effect on the next auto-resume), and carried across
  silent restarts by `migrate_agent_chat_session`. On pane mount the
  frontend fetches `agent_chat_get_session(threadId)` and re-seeds the
  pickers + resume cursor from the row, falling back to the provider
  default model only when no row (or no persisted model) exists —
  fixing the post-restart "reset to Opus" regression.
  - **Pending requests are not conversation history.** Claude/Codex keep
    approval and `AskUserQuestion` callbacks only in the live
    sidecar/app-server process. Resuming the transcript cannot reconstruct
    those callbacks, so `agent_chat_respond_to_request` never starts a new
    Claude/Codex session merely to answer an old request. It persists
    `request_response_failed { reason: "stale_provider_callback" }` instead;
    reducer replay terminalizes the request and explains that the user should
    Continue so the agent can repeat it. Hydration (`replayPayloads` in
    `src/lib/agent-chat/hydrate.ts`) applies the same terminal state to an
    unresolved persisted request, but only when BOTH guards say the callback
    is gone:
    - `opts.runLive` — the backend's `agent_chat_turn_active` probe. A live
      turn means an ordinary workspace-switch remount, so its requests stay
      actionable.
    - `opts.provider` — OpenCode is the exception: its permission state lives
      in the external HTTP server, so its adapter opts into request resume
      (`pending_requests_survive_session_restart`) and
      `agent_chat_respond_to_request` re-adopts the session to deliver the
      answer. Hydration mirrors that opt-in through the pure frontend table
      `providerRequestsSurviveSessionRestart` (in
      `src/lib/agent-chat/capability-defaults.ts`, no extra IPC) and leaves
      OpenCode requests `pending`. Expiring them would be unrecoverable: the
      reducer's `request_opened` early-returns on a known request id, so no
      re-broadcast can resurrect a request the UI already terminalized.
    Terminalizing is one-way but not destructive of a real answer: the
    reducer's `request_response_failed` arm skips a request already in the
    `resolved` state. Providers cannot distinguish "unknown request" from
    "already answered" (Codex drops the pending entry on the first reply), so
    a duplicate respond — the same thread answered from two windows — reports
    a failure for a request that actually succeeded; without the guard that
    persisted event would durably flip an answered approval to "expired" on
    every replay.
    - **Wire surface.** `ProviderError::RequestNotPending { request_id }`
      (`agent_provider/errors.rs`, mirrored as
      `SerializableProviderError::RequestNotPending`) is the shared
      classification every adapter converges on: Codex returns it in place of
      the old `ValidationError`, OpenCode maps HTTP 404/409/410 onto it, and
      Claude derives it in `is_request_not_pending_rpc` (`claude/session.rs`)
      from an RPC code of `-32602` **or** `-32603` *plus* a message containing
      `not found or already resolved` / `unknown pending` / `no pending
      approval` — the sidecar's `respondToRequest`
      (`sidecar/claude-agent/src/methods/index.ts`) rethrows those as
      `InvalidParamsError` (-32602), and `-32603` is kept only so an older
      bundled sidecar still classifies. The match is deliberately narrow: an
      unrelated internal error keeps its real classification.
      `agent_chat_respond_to_request` converts `RequestNotPending`,
      `SessionNotFound`, and `SessionClosed` into the same
      `ProviderRuntimeEvent::RequestResponseFailed { thread_id, request_id,
      reason, message }`, whose `RequestResponseFailureReason` enum currently
      has one variant, `StaleProviderCallback` (serde
      `"stale_provider_callback"`). The event is persisted
      (`should_persist_event`), counts as turn-settling in `activity_update`,
      and maps to `PaneStatus::Review` in `map_event_to_pane_status` — the
      thread needs the user, but no turn is running.
    - **Frontend shape.** `PermissionRequestItem.resolution` gains a
      `{ state: "failed", reason: "stale_provider_callback", message }`
      variant (`src/lib/agent-chat/types.ts`), rendered in three places:
      `PermissionRequestBlock.tsx` (the control itself), `MessageList.tsx`,
      and `ToolCallCard.tsx` (error header + the expiry explanation). A linked
      workflow card goes to `stopped`.
  - **NULL `permission_mode` heal**: rows created before the
    `permission_mode` column existed read back NULL. The frontend seeds its
    permission picker to the provider default ("Full access" =
    `bypassPermissions` for Claude, `danger-full-access` for Codex) for such
    rows, so `ensure_live_session` resolves a NULL to that same provider
    default (`fallback_permission_mode`) before rebuilding — otherwise the
    SDK launches in `default` mode and prompts for every Edit/Bash while the
    UI still says Full access. When it substitutes a default it also
    best-effort persists the resolved mode back to the row (never failing
    the resume on a DB write) so future rebuilds and the frontend seed
    agree. A one-time `database.rs` migration backfills the same defaults
    onto existing NULL Claude/Codex rows (OpenCode has no permission modes,
    so its rows stay NULL). This became user-visible in v0.13.2 once the
    dead-run watchdog started rebuilding sessions mid-lifetime, not just
    after an app restart.
  - **Silent bypass-downgrade heal (sidecar `canUseTool`)**: even when
    every layer forwards `bypassPermissions` correctly, the Claude CLI
    re-checks its danger-mode consent state at process boot. When that
    read transiently fails or looks absent — which happens when concurrent
    sessions rewrite the shared settings/consent files at the same moment —
    the CLI silently strips bypass and boots in `default` mode, warning
    only on stderr, so the SDK starts gating tools while the composer still
    shows "Full access" (intermittent; re-picking Full access restarts the
    session and usually wins the race). The invariant that fixes it: the
    SDK never calls `canUseTool` for a session genuinely running under
    bypass. So the sidecar's `canUseTool` (`sidecar/claude-agent/src/permissions.ts`)
    treats any invocation while the session's INTENDED mode is
    `bypassPermissions` as that downgrade — it auto-allows the call with
    its original input (no Allow/Deny card, no parked request) to honor the
    user's explicit choice, and asks `ClaudeSession` to fire a one-shot
    live `setPermissionMode("bypassPermissions")` restore (best-effort; the
    CLI may reject a live switch when its boot gate stripped the launch
    flag, which is fine — the per-call auto-allow already guarantees Full
    access semantics). The intended-mode getter reads `startInput.permissionMode`,
    which `set_permission_mode` keeps current, so a Plan-pill flip to
    `plan` correctly re-enables prompting. `AskUserQuestion` (a
    clarification form, not a permission) and `ExitPlanMode` (denied with a
    stop-now message) keep their interactive paths. The one-shot restore
    re-arms when `ensureLiveQuery` rebuilds the query, since each CLI boot
    may be downgraded independently. NOTE: the recurring "Full access
    still prompts" reports traced to the null-mode launch below, which
    this guard cannot catch — when no mode is passed at launch the
    sidecar's intended mode reads `undefined`, not `bypassPermissions`,
    so the downgrade condition never matches.
  - **Provider-native Full-access launch heal**: the session-history dropdown's New Chat and
    resume-a-past-chat flows (`src/hooks/use-agent-chat-session-actions.ts`)
    launched sessions with `permission_mode: null` (and no
    model/effort/context config), so the SDK booted the Claude CLI in
    `default` mode and gated every tool behind Allow/Deny cards — while
    the composer pill showed "Full access" because a fresh store slice
    defaults to `DEFAULT_THREAD_PERMISSION_MODE` (`bypassPermissions`)
    and the DB row's `permission_mode` stayed NULL (`keep_or_set(None)`
    never writes). This only hit long-lived main-workspace panes: every
    other launch path (fresh-pane mount, draft materialize, worktree
    prestart) already passed a real mode. The original two-layer fix made
    the hook pass a concrete mode and made `agent_chat_start_session`
    heal NULL at the backend boundary. A follow-up closed the
    multi-provider variant of the same drift: permission-mode strings are
    provider protocol values, not shared UI enums — Claude Full access is
    `bypassPermissions`, while Codex Full access is
    `danger-full-access`. The frontend now resolves the active provider's
    advertised `default_permission_mode` (with provider-native bootstrap
    fallbacks) for New Chat, history resume, fresh/recovered panes, draft
    materialization, provider switches, and mode restoration, then seeds
    the slice's `permissionMode` AND
    `sessionLaunchMode` so the pill, restart detection, and the actual
    launch agree. At the backend boundary, both `agent_chat_start_session`
    and `ensure_live_session` canonicalize the two known cross-provider
    Full-access values as well as healing NULL before the provider consumes
    the input. The schema-open migration repairs already-persisted Codex
    rows carrying `bypassPermissions` (and the symmetric Claude mismatch).
    This is required because the Codex adapter deliberately omits unknown
    modes from `thread/start`; forwarding Claude's value therefore makes
    Codex fall back to approvals while the picker still says Full access.
- **Stale-resume self-heal (Claude)**: a persisted `sdk_session_id` can go
  stale — the CLI deletes old conversation JSONLs (cleanup, version
  updates), and resuming one fails every turn with
  `error_during_execution — No conversation found with session ID: …`,
  permanently wedging the thread. Three layers now recover it:
  - **Sidecar** (`sidecar/claude-agent/src/session.ts`): detects the
    stale-resume `result` error (or the thrown-error variant), SUPPRESSES
    it, emits a `resume-fallback {threadId, staleSessionId}` notification,
    rebuilds a FRESH query (all resume state cleared) and replays the
    pending user turn verbatim — so the send transparently succeeds. One
    recovery per turn (a repeat failure surfaces normally). The CLI
    validates `--resume` eagerly, so the error can also arrive right after
    session start with no turn in flight; recovery still fires, just with
    nothing to replay.
  - **Rust** (`protocol.rs`/`session.rs`/`translate.rs`/`agent_chat.rs`):
    `resume-fallback` clears the in-memory `sdk_session_id`, translates to
    `ResumeCursorUpdated(null)` (the persist path clears the DB column on
    exactly JSON null via `clear_agent_chat_sdk_session_id`) + a
    `resume-fallback: `-prefixed `RuntimeWarning` that the frontend
    classifier (`runtime-notice.ts`) promotes to an inline transcript
    notice. The rebuilt query's fresh `sdk-session-id` re-persists shortly
    after. The replayed turn stays `Running` (no state clobber).
  - **Resume preflight** (`claude_session_file_missing` in
    `commands/agent_chat.rs`): before `ensure_live_session` /
    `agent_chat_start_session` hand a persisted Claude cursor to the SDK,
    they positively confirm `<config>/projects/*/<id>.jsonl` still exists
    (`$CLAUDE_CONFIG_DIR` or `~/.claude`). Only DEFINITIVE absence drops
    the cursor (and best-effort clears the column); any IO doubt keeps it,
    so a valid resume is never lost to a flaky probe.
- **Reusable JSON-RPC-over-stdio child-process helper** with timeout,
  graceful shutdown, bidirectional notifications, server-initiated
  requests, and child-exit cleanup.
- **Auth probes** (`probe_installed`, `probe_authenticated`) for each
  provider.
- **Integration tests** covering each adapter, the JSON-RPC helper, the
  Claude sidecar (38 Bun tests + 27 fake-sidecar Rust tests + 43
  translate/protocol unit tests), and the picker UI (Vitest).
- **Codex spawn race fix**: probe spawns retry on ETXTBSY (text file busy);
  `interrupt_turn_sends_turn_interrupt` and
  `auth_probe_unauthenticated_matches_common_patterns` are gated to Unix
  in CI to dodge the `fake_codex_app_server` helper-binary build race
  under cargo's parallel scheduler.

## Agent Tasks panel

Provider-authored plans are normalized into the durable
`ProviderRuntimeEvent::TasksUpdated { thread_id, tasks }` snapshot. This is
agent state, not a second editable project-management surface: users can inspect
the plan and progress but cannot check, reorder, or rewrite its rows.

- **Provider mapping.** Codex maps `turn/plan/updated`; OpenCode maps
  `todo.updated`; Claude maps legacy `TodoWrite` and the current
  `TaskCreate` / `TaskUpdate` / `TaskList` result stream. Child/subagent plans
  remain child-scoped and never replace the orchestrator's panel. No ACP-based
  provider adapter exists in Codemux today; the canonical event is deliberately
  provider-neutral so a future adapter can map ACP `plan` updates without UI
  changes.
- **Thread focus.** `useActiveChatTasks` resolves the active surface's focused
  leaf pane, then reads only that pane's thread snapshot. Switching panes or
  split focus therefore switches the task source instead of leaking another
  chat's plan into the workspace panel.
- **Conditional chrome.** A compact `Tasks N/M` composer control and right-panel
  Tasks tab appear only for a non-empty snapshot. Clicking the composer control
  toggles the shared right panel. A stale persisted `tasks` panel selection
  falls back to Files when the focused pane has no task state. Both controls
  carry run state so progress stays readable from any tab: the composer chip
  turns amber with a spinner while a step is in flight and green with a check
  once every row is done, and the Tasks tab shows a blinking amber dot while a
  step is running and the tab is not the active one.
- **Live affordances end with the run.** The snapshot is durable and
  nothing rewrites it when a turn finishes, so an `in_progress` row is not
  evidence that work is happening — the thread's `streaming` flag is. Both
  live affordances are therefore gated on it: the composer chip's spinner
  (`taskChipSummary` in `src/lib/agent-chat/task-summary.ts`, consumed by
  `AgentChatPane`) and the Tasks tab's blinking dot (`right-panel.tsx`, via
  the `streaming` field `useActiveChatTasks` now returns). A plan the
  provider left mid-step otherwise spun forever, including across a restart
  (`TasksUpdated` is persisted and hydrate-replayed). The chip is **not**
  hidden when the run ends — the durable snapshot is by design — it simply
  renders the same `N/M` counts statically. Rows inside the panel keep
  their provider-reported state.
- **Presentation.** The read-only panel preserves provider order and renders
  flat, numbered rows (no per-row card chrome) with three visually distinct
  states — done (green circled check, dimmed, struck through), in-progress
  (amber spinner, tinted row, full-strength text), pending (hollow dot,
  muted) — so the eye lands on the active step. The header carries a
  Queued / Working / Complete status badge, `N/M done`, the last-update time
  (`tasksUpdatedAt`, stamped by the reducer on each `tasks_updated`), and a
  3px tone-colored progress bar; the provider's one-line explanation renders
  above the rows as run intent. Dependency ids resolve to task titles when
  possible, and a footer offers a Copy action (markdown checklist via
  `tasksToMarkdown`). Snapshots persist in `agent_chat_messages` and hydrate
  through the ordinary event reducer, so reopening a thread restores the same
  toggle and panel (`tasksUpdatedAt` then reflects hydration time, not the
  original wall time).
- **Thread receipt.** `TaskSummaryCard` no longer prints the full todo list in
  the transcript — that duplicated the panel with a copy that never updated.
  Each `TodoWrite`-style call collapses to a one-line receipt ("Task list
  created · 3 items" / "Task list updated · 2/3 done") that flips the right
  panel to Tasks when a `workspaceId` is threaded (same pattern as
  `WorkflowRunCard`'s "Open panel"); without one the row renders inert. The
  panel is the live truth; the thread just records that a plan landed.

## Hydration by cursor (warm resume)

Opening a thread no longer replays it from event zero. Hydration is a **tail
read against a durable DB cursor**, driven by `hydrateThreadByCursor`
(`src/lib/agent-chat/cursor-hydrate.ts`) — the pane's only hydrate call
(`AgentChatPane.tsx`). The old shape (a preflight `agent_chat_list_messages`
replay whose result was discarded except for `messages.length`, then a second
full replay inside `hydrateThread`) is gone; a cold open parses each payload
once.

- **The cursor is a row id, not a rendered count.** `agent_chat_messages` has
  an autoincrement `id` with a `(thread_id, id ASC)` index, so no migration was
  needed. `agent_chat_list_messages_after(thread_id, after_id)` returns
  `{ id, payload }` rows strictly greater than the cursor in id order, and the
  slice stores the last one as `lastPersistedEventId`. A **warm revisit of an
  unchanged thread therefore reads and reduces nothing** — the tail is empty.
  A companion `agent_chat_thread_head_id` probe rides in the same `Promise.all`
  so a cursor left over from a foreign id space (a re-homed session) is caught
  rather than trusted.
- **Read-after-attach, with the live stream held.** The window between the DB
  read and the store write used to lose events. Hydration now runs
  `hold → wait for attach → read → apply → filtered release`: the batcher's
  hold parks incoming events for the thread,
  `waitForAgentChatAttach(threadId)` (`attach-registry.ts`, 2 s cap, never
  rejects) waits for the channel registration published by
  `use-agent-chat-events.ts`, and the release drops only the `content_delta`s
  the read already settled (`dropDeltasSettledByRead(cursor)`) so a streamed
  block cannot be appended twice. A hydrate cancelled by unmount drops its
  buffer instead of releasing it.
- **Hold tokens are owned.** `holdAgentChatEvents` mints a fresh `Symbol`;
  `release`/`drop` no-op unless they present the token that is currently
  installed. So an older overlapping hydrate's teardown cannot release the
  newer hydrate's hold and let events through mid-read.
- **A cold empty read never wipes a populated transcript.** If a cold hydrate
  reads zero rows for a thread the store already has messages for, the rows are
  assumed to be filed under a different id (session resume re-homing) rather
  than deleted: the transcript is left alone, the cursor is invalidated, and the
  read is retried exactly once after 500 ms.
- **Duplicates are killed on two axes.** Live channel events carry their
  `persisted_id`, and `applyLiveEvents` skips any event at or below
  `lastPersistedEventId` (so the tail read and the live stream can overlap
  freely). Separately, an optimistic user bubble carries a `client_nonce` that
  hydration and the live `turn_queued` reconciliation both match on, so the
  persisted copy replaces the optimistic one instead of doubling it.
- **User turns are fanned out, because nothing else would carry them.**
  Providers never echo a user turn, so its `agent_chat_messages` row is the
  only record of it. The backend therefore mints a
  `ProviderRuntimeEvent::UserMessage` from the row it just wrote and sends it
  to every channel attached to the thread, stamped with that row's id — the
  same persist-before-fan-out order `forward_event` uses, through the same
  `fan_out_to_thread_channels` helper. Its serialized shape is deliberately
  identical to the stored `{"type":"user_message", …}` envelope, and one
  reducer case folds both, so a client that saw the turn live and a client
  that only replays the rows cannot end up with different transcripts.
  Without the fan-out, a *second* client watching the thread (a desktop window
  while the phone sends) received only the assistant reply — persisted at a
  higher id — and `applyLiveEvents` advanced its cursor past a user row it had
  never seen. Every `id > cursor` tail read then skipped that row, so the other
  client's prompt was lost permanently, with no revisit that could recover it.
  The sender is unaffected either way: it drops its own copy on the nonce while
  still advancing its cursor over the row. The queued-follow-up path fans out
  at dispatch (when the envelope is actually written) and only when a nonce
  exists, since `turn_queued` has already put a greyed bubble on every client
  attached at enqueue time and the nonce is what keeps this copy from becoming
  a second one.

### Large tool results hydrate as metadata

A single 13 MB thread used to ship every collapsed tool body across IPC just to
render twelve preview lines. On the **read path only**, a persisted
`tool_result` whose serialized content exceeds
`LAZY_TOOL_RESULT_THRESHOLD_BYTES` (32 KiB) is replaced by a stub keyed
`__codemux_lazy_tool_result` carrying `{ row_id, bytes, preview (2 KiB,
truncated on a char boundary), line_count, has_images: false }`. The collapsed
card renders from the preview and line count; expanding fetches the real body
with `agent_chat_get_tool_result(row_id)` and `resolveLazyToolResult` swaps it
into the item, producing a new item object so exactly one row re-renders.

Two constraints are load-bearing:

- **An image-bearing result is never stubbed.** `shape_persisted_payload`
  returns early when the content holds a renderable image block, and its
  accept-set deliberately mirrors the frontend's `tool-result-images.ts` — a
  stubbed image would render as a broken attachment with no way back.
- **Live events and `agent_chat_list_messages` are untouched.** Only
  `agent_chat_list_messages_after` shapes payloads, so the mirror/web-remote
  path and the streaming path still see verbatim content.

Two known gaps, neither reachable today: the stub's own doc comments describe
a "copy" path that does not exist (nothing in the chat UI copies
`result_content` — the clipboard writers copy tool *input* and markdown code
blocks), and `grepHitCount` (`activity-steps.ts`) reads `result_content` to
label a Grep step "N hits", so an oversized Grep result falls back to "done".
Both become real bugs the moment a copy-result or transcript-export surface
lands; a consumer of a tool body must go through the fetch.

### Warm slices are byte-weighted, not unbounded

Hydrated threads stay in the store so a revisit is free, which without a bound
is a leak on a 161-thread profile. `evictColdThreads` drops least-recently-
touched slices once the estimated total exceeds `WARM_SLICE_BUDGET_UNITS`
(128 Mi units of estimated payload characters, not heap bytes; per-item weights
memoized in a `WeakMap`). LRU stamps live in a module-level map **outside** the
slice, so touching a thread never changes slice identity and never re-renders
anything. Exempt from eviction: the thread being hydrated, any thread a pane is
mounted on (refcounted), and any *busy* slice — streaming, mid-turn, holding a
pending request, or holding unsent composer work (draft text or staged
attachments). An evicted thread simply cold-hydrates again.

### Streaming is batched at display cadence

Provider `content_delta` events used to be one `set()` and one render per
token. `src/lib/agent-chat/event-batcher.ts` coalesces them at frame cadence:
deltas queue and drain on a `requestAnimationFrame`, while **every other event
kind flushes the thread's whole queue synchronously, itself included, in
arrival order** — so an approval, a tool result, or a turn/session state change
is never delayed behind a token, and ordering is preserved. A hidden document
has no rAF, so the batcher falls back to a 32 ms timer and drains immediately
on `visibilitychange`. Pane detach and pre-hydrate are explicit flush seams.

The scheduler idles when there is nothing it could drain: a queue belonging to
a HELD thread can only be moved by `release`/`drop`, so re-arming for one would
wake every frame, drain nothing and re-arm — a 60 Hz no-op loop lasting exactly
as long as the hydrate's IPC round trip, which is when the main thread is
busiest. `flushAll` therefore re-schedules only while some *unheld* queue
remains, and ending a hold is what wakes the flush back up.

### The pane subscribes by field, not by slice

`AgentChatPane` used to read its entire thread slice, so a draft keystroke
re-entered the whole tree. It now takes six narrow subscriptions: `draft`
(a scalar), a `useShallow` `timeline` group
(`messages`/`streaming`/`stalled`/`interrupted`/`activeTurnId`), a `useShallow`
`settings` group (model, permission mode, effort, context window, fast mode,
mode, debug-activity flags), `stagedAttachments` with an empty-array
sentinel, and one scalar each for `tasks` and `contextUsage`. The last two are
deliberately *not* folded into `settings`: the Tasks panel updates on tool
events and the context meter once per provider usage report, and either riding
the settings group would re-run every picker consumer on a cadence it has no
stake in. The pane itself still re-renders on a keystroke — it owns the
composer — but **no timeline row does**, which
`transcript-keystroke-isolation.test.tsx` asserts directly (a draft write
leaves the timeline fields reference-equal and every row's render count
unchanged, while a real message change still re-renders exactly that row).

Per-second elapsed labels are also out of React: `TickingText`
(`src/components/chat/TickingText.tsx`) renders an empty span and writes
`textContent` from an interval, so the activity bar's and subagent cards' 1 Hz
readouts never enter a commit. It is only valid for output that is a pure
function of `now` plus already-rendered props. The Orchestration panel's
per-phase labels deliberately stayed on a coarse 5 s `now` prop instead —
that value crosses a component boundary, so the trade there is a few seconds
of precision on a secondary readout for a 12× drop in panel re-renders.

## Transcript scroller (issue #77 contract, LegendList)

The transcript body (`MessageList.tsx`) is a real windowed list built on
`@legendapp/list/react` 3.2.0 — the windowed-list architecture used by
comparable agent transcript UIs. Only the visible range plus an 800px draw
buffer is mounted; dynamic row heights are measured and cached. This replaces
the shadcn MessageScroller's all-rows-mounted `content-visibility` approach,
whose cold estimated heights caused visible corrections during fast scrolling,
especially on WebKit.

The list scroll container carries `[scrollbar-gutter:stable_both-edges]` and
every row, the header, and the footer sit on the shared `CHAT_COLUMN` rails
(`chat/chat-column.ts`). Reserving the gutter on one edge only would shrink
the box the centered rows are measured against and slide the whole transcript
half a scrollbar to the left of the composer, which sits *outside* the
scroller on the same rails; both edges keeps the two concentric. Effective
content width stays `min(760px, paneWidth - 32px)`, with the horizontal
gutter outside the max-width box.

Layout derivation lives in the pure `buildTranscriptSlots`
(`transcript-slots.ts`): turn grouping (avatar once per assistant
turn), Activity-run folding (reasoning + tool calls → one `activity`
slot — see "Activity block" below), and per-slot identity.

Contract preserved from the pre-redesign renderer:

- **Stable keys + typed, memoized rows.** Per-slot keys remain
  `slot.item.id` / `run:<first-id>` and `getItemType` separates size averages
  for user, assistant, activity, workflow, and other row kinds.
  Rows render through **two** memo levels (issue #129): a whole-row
  wrapper `SlotRowMemo` and the leaves `ItemRowMemo` / `ActivityRowMemo`.
  The wrapper's skip depends on **slot-object reuse**: `buildTranscriptSlots`
  rebuilds every slot on each store update (each streaming token), so
  `reuseTranscriptSlots` swaps unchanged slots back to their previous object
  identity (matched by key; item bodies compared by `item` reference,
  activity bodies by `working` + element-wise `items`), returning the prev
  array itself when nothing changed. With that reused identity plus the
  reducer's stable `item` / `approval` refs, an untouched row's wrapper props
  stay shallow-equal and it skips reconciliation entirely — so a streaming
  token re-renders **exactly one wrapper and one leaf**, and the rest of the
  transcript is untouched. (Activity rows still rebuild their `items` array
  each build, as the old tool-group rows did; the stable `run:<first-id>` key
  keeps the scroller row from remounting as the run grows and transitions
  working → settled.) The shells around the window are memoized too —
  `ChatTranscript`, `MessageList` and `MessageTrail`/`TrailRail` are `memo()`d,
  `keyExtractor` / `itemsAreEqual` are hoisted to module scope, and the
  `ListHeaderComponent` element is memoized like the footer already was — so a
  parent re-render does not walk back into the list. `MessageList.memoization.test.tsx`
  guards it.
- **Stick-to-bottom and free-scroll are one virtualizer contract.**
  `initialScrollAtEnd` opens hydrated sessions at the latest row.
  `maintainScrollAtEnd` follows data, item-size, footer-size, and viewport
  layout changes only while the reader remains near the end (3% threshold).
  `maintainVisibleContentPosition={{ data: true, size: false }}` preserves the
  reader's anchor as data changes while they browse history. Browser-native
  anchoring stays disabled so it cannot compete with LegendList's measured
  position model. The former WebKit-specific anchoring shim is deleted.
- **The new-turn scroll contract** (`send-scroll-state.ts`). A composer
  submission is treated as an explicit *navigation intent*, not as a data
  update: the transcript surfaces the new turn, reserves room for the answer,
  and follows the stream until the reader deliberately leaves. Three states
  and one anchored message identity describe the whole behavior — `MessageList`
  owns them; nothing above it decides where the viewport goes.

  - `following-end` (default) — hydrated threads open at the latest row and
    stay pinned while the reader is at the edge.
  - `anchoring-turn` (on send) — the optimistic prompt is parked
    `SEND_ANCHOR_OFFSET` (16px) below the transcript top and the response
    streams into the space reserved beneath it.
  - `free-scrolling` (on gesture) — manual navigation cancels live follow.

  **Identity, not "the last row".** `AgentChatPane.handleSubmit` issues a
  `sendAnchor` (`{ clientNonce, nonce }`) in the *same batch* as the optimistic
  `appendUserMessage`, reusing that bubble's existing `clientNonce`; the
  incrementing `nonce` keeps two byte-identical prompts distinguishable as two
  intents. `MessageList` resolves the matching `user_message` slot by nonce
  (last match wins, `resolveSendAnchorIndex`) rather than by index, because
  queued follow-ups and control rows can land after the prompt. The same
  contract covers text sends, image sends, the one-click "Continue run" (it
  routes through `handleSubmit`), and dispatching a queued turn with "send
  now". Both failed-send rollback paths call `clearSendAnchor`, so a dead send
  never strands reserved end space; a failed "send now" clears it too (that
  bubble stays queued, so there is no rollback to ride along with), and so
  does switching threads.

  **The anchor expires when the turn settles.** It is a live-turn intent, not
  a durable property of the thread, so `AgentChatPane` clears it on the
  falling edge of the same combined live signal the transcript gets
  (`streaming || isSending`, so the optimistic window between Enter and the
  backend's first `Running` counts as live). Two things would otherwise go
  wrong once a run finished: a later `MessageList` remount on the same thread
  — the subagent drill-in/out swap, for instance — would re-park a
  long-finished prompt near the top instead of opening at the latest row, and
  LegendList's built-in end pin, which an anchor deliberately disables, would
  never come back for the item/footer layout growth the anchor's geometry does
  not model (a late-loading image growing a row with no data change).
  Expiry is a lifecycle event, not navigation: if the reader has already
  gestured into `free-scrolling`, the transcript drops the anchor bookkeeping
  **without** re-claiming the viewport, so a turn finishing never yanks
  someone browsing history back to the tail or takes away the pill they are
  using to get there.

  **Anchor after measurement, never on a timer.** The resolved slot index
  feeds LegendList's `anchoredEndSpace` (`anchorOffset: 16`). Its `onReady`
  callback — fired once the row is measured and the reserved space sized — is
  what positions the row, via `scrollToIndex({ animated: false, viewPosition:
  0, viewOffset: 16 })`, instant per Codemux's "immediate" contract. If the
  list ref is not live yet it retries per frame (a frame budget, not an
  assumption that layout completes in N ms). While an anchor is mounted the
  built-in `maintainScrollAtEnd` is switched **off**: follow-the-tail and
  anchor-the-new-turn are two targets for one viewport and must not both drive
  it. As the answer grows, an effect advances by exactly
  `scrollDeltaToRevealEnd` — zero while the turn still fits, so the prompt
  stays near the top, then just enough to keep the growing tail on screen.
  The geometry is pure and unit-tested (`send-scroll-state.test.ts`).

  **Opting out is the reader's call.** Only real navigation gestures release
  live follow — `wheel`, `touchmove`, and a `pointerdown` **whose target is
  the scroll container itself** (passive listeners on the scrollable node),
  plus subagent-jump navigation. The `pointerdown` scoping matters: a
  scrollbar drag targets the scroller, but accepting a plan, answering an
  approval, expanding a tool card, or starting a text selection targets a
  descendant row. Treating those as navigation froze the viewport for the rest
  of a live run, because while an anchor is mounted the built-in pin is off
  and the advance effect is the only thing moving it. Deliberately *not*
  `scroll`:
  every programmatic correction emits one, and trusting those is what used to
  leave the pill stuck on. A generation counter pairs "we still own the
  viewport" against "a gesture has happened", and every async continuation
  re-checks it, so a gesture landing mid-flight wins. Scrolling back to the
  edge re-claims follow. Incoming tokens never move a reader who is already
  free-scrolling.

  **Pill honesty.** "Jump to latest" is *shown* on a 150ms trailing debounce
  and *hidden* immediately — raw `isAtEnd` reads false throughout mount and
  layout settling, so an undebounced show flashes it on every thread open.
  A send hides it synchronously in the same render that first sees the anchor.
  Using the pill re-claims follow (it is the deliberate way back) rather than
  cancelling it.
- **Variable heights.** LegendList measures rows and observes layout changes;
  Activity blocks can expand in place while retaining the correct scroll
  position and cached size model.
- **Live-control rows never unmount** (`alwaysRender={{ keys }}`). Windowing
  would otherwise discard transient local interaction state when the reader
  scrolls away. `deriveAlwaysRenderKeys` (`always-render-keys.ts`, pure and
  unit-tested in `always-render-keys.test.ts`) pins queued `user_message`
  rows, `permission_request` rows still `pending` / `responding`, and
  `tool_call` / `workflow_run` rows whose linked approval is `pending` /
  `responding`. Nothing else qualifies, so the pinned set stays tiny even on
  a 5,000-message thread.
- **Streaming marker** (shimmer status) renders as the last row inside
  the list footer so footer-layout following keeps it visible while streaming;
  the jump-to-latest pill calls the same list `scrollToEnd` API. **A working
  Activity block already shows the single live line, so the marker is
  suppressed when one is the transcript tail** — `MessageList` passes
  the thread `streaming` flag into `buildTranscriptSlots`, and the
  marker only renders when the tail slot is not a working Activity block
  (it still fills the gap right after send, before any step arrives).
  The marker carries a **live elapsed-time suffix** ("Writing… · 40s"):
  `StreamingMarker` derives the active turn's start with
  `deriveTurnStartedAt` (the earliest reducer-stamped `started_at` of the
  reasoning / tool steps after the last non-queued user turn) and ticks a
  text node via `setInterval` writing `textContent` directly — no
  per-second React re-render of the transcript. No start is derivable in
  the gap right after send or on a hydrated transcript whose rows predate
  the timestamp fields; the suffix is then omitted rather than faked.
- **Empty deltas never materialize a message.** The partial-message
  stream often opens a new text content block with an empty (or
  whitespace-only) first delta. `appendTextDelta` (`reducer.ts`) drops it
  instead of creating an empty `assistant_message`: an empty row would
  render near-blank AND, as a non-step item, flush the live Activity run
  as settled mid-turn — so the transcript would read finished-but-empty
  while the turn is still working. The drop is deterministic, so a
  hydrate/replay of the same events (`hydrate.ts`) keeps the identical
  item count as live streaming. `shouldShowThinkingIndicator`
  (`thinking.ts`) also keeps the tail marker up for a
  streaming-but-empty `assistant_message` as defense in depth for a
  provider that lands one anyway.

### Perf on Linux WebKitGTK (issue #129)

Transcript-scroll behavior on the Linux app webview. The original round
was frontend-only workarounds for a CPU-rendered webview; the webview now
runs with accelerated compositing (`src-tauri/src/webview_tuning.rs`), so
the gates below have been lifted and the remaining items are the
structural wins:

- **Viewport edge-fade mask — on everywhere (re-enabled on Linux).** The
  design edge fade (`WS_FADE_STYLE`) is a CSS `mask-image` on the scroll
  viewport. It was originally gated off on Linux WebKitGTK, where the app
  ran the webview in non-composited (CPU) mode and a viewport mask forced
  a full-viewport re-rasterization on every scroll frame. Accelerated
  compositing is now restored on Linux (SHM buffer transport, see
  `src-tauri/src/webview_tuning.rs`) and the mask measures as free — ~16ms
  frames with and without it — so the fade is on by default on every
  platform. The decision stays a pure, injectable helper
  (`transcript-fade.ts`, `decideTranscriptFade`), cached once per session,
  and still honors a `localStorage["codemux:transcript-fade"] = "on" |
  "off"` override (escape hatch for a driver stack where it still hurts);
  a one-time `console.info` states the verdict only when an override is in
  play. The shared `isLinuxWebKitGtk` check lives in `@/lib/webkit` and is
  still used by the terminal renderer probe and the smooth-scrolling
  setting.
- **Animated wheel scrolling is off by default on Linux** (Settings →
  Appearance → Scrolling re-enables it). The webview's kinetic scroll
  animation runs on a fixed timeline, so a high-resolution wheel that
  emits many small deltas per flick queues animations and the transcript
  moves *slower* the faster you scroll. The preference is machine-local
  (`appearance.smooth_scrolling` in the settings store) and is pushed to
  the webview via the `set_smooth_scrolling` command
  (`smooth-scrolling-section.tsx`, `use-smooth-scrolling.ts`); the toggle
  only renders on Linux WebKitGTK, where it has an effect, and is hidden
  on the web remote client (`isRemoteClient()`) — `useSmoothScrollingInit`
  and `useRendererModeInit` likewise no-op there, because the command acts
  on the desktop host's webview, not the browser the page is rendered in.
- **Renderer selection is guarded four ways** (`webview_tuning.rs`). The
  accelerated `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` default is only applied
  when it can work and can be backed out of:
  - **WebKitGTK ≥ 2.40 floor** (`FORCE_SHM_MIN_WEBKIT`, probed at runtime
    via `webkit_get_major/minor_version` in `runtime_webkit_version` — no
    GTK init needed). Older WebKit doesn't recognize the variable, so
    setting it would silently reinstate the original crash; those builds
    drop straight to the legacy compat flags with no sentinel bookkeeping.
  - **Crash sentinel, disarmed on clean exit.** Startups that never reach
    page-load are counted in an on-disk sentinel and stick the process
    back on legacy CPU flags after repeated failures. `mark_clean_exit`
    fires on `tauri::RunEvent::Exit`, so quitting during boot no longer
    counts as a crash (ownership-gated, so a second instance can't clear
    the first's counter).
  - **`=0` is a real opt-out.** `WEBKIT_DISABLE_*=0` / `…FORCE_SHM=0`
    survive the inherited-env scrub and do NOT mark the process
    CPU-rendered (`env_value_disables`) — only a non-`0` value counts as
    the user pinning the legacy renderer.
  - **Recovery.** The CPU fallback is sticky by design; deleting
    `webview-renderer.sentinel` under the app data dir re-arms the
    accelerated path on the next launch (the stderr note says so when the
    fallback engages).
- **Slot-object reuse + a memoized whole-row wrapper** (see "Stable keys
  + two-level memo rows" above): `reuseTranscriptSlots` preserves an
  untouched slot's object identity across the per-token rebuild, so with
  the two-level memo a streaming token re-renders exactly one row wrapper
  and one leaf — the rest of the transcript skips reconciliation instead
  of re-running all *n* wrappers per token.
- **Real row windowing.** Long-thread cost no longer depends on WebKit's
  `content-visibility` support. LegendList bounds mounted rows and maintains a
  measured position model on every supported webview.
- **DMABUF renderer hypothesis** from the issue is handled natively:
  `configure_renderer_env` defaults to `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`
  (accelerated compositing, SHM buffer handoff, ~16ms scroll frames vs
  ~56ms on the old CPU path), with a crash-sentinel fallback to the legacy
  `WEBKIT_DISABLE_*` flags for hardware that cannot start on it. The
  `tauri:dev` scripts no longer hardcode `WEBKIT_DISABLE_DMABUF_RENDERER=1`,
  and renderer vars inherited from a parent Codemux terminal are scrubbed so
  they are not mistaken for a user override; `CODEMUX_WEBKIT_COMPAT=1` opts
  into the legacy renderer explicitly. Whichever renderer wins is reported to
  the UI by the `get_renderer_mode` command, and the transcript edge-fade mask
  disables itself automatically in compatibility (CPU) mode.

### Activity block (Activity Stream)

`ActivityBlock.tsx` replaces the old per-tool "Thought / Thought /
Ran…" spam with **one card per contiguous run of mechanical steps**
(reasoning items + tool calls). `buildTranscriptSlots` folds a run of
`reasoning` and groupable `tool_call` items into a single `activity`
slot; `activity-steps.ts` holds the pure step/summary/counter/duration
derivations (unit-tested in `activity-steps.test.ts`).

- **Working vs settled is derived.** A run is *working* when the thread
  is `streaming` and the run is the tail run of the active turn;
  everything else *settles*. Working shows a collapsed row — amber
  spinner (`status-working`), bold "Working", a shimmering mono live
  action, a `N done · M running` counter, a rotating chevron. Settled
  rolls up to a green circled check + a derived summary sentence
  (`deriveActivitySummary`: read-heavy → "Explored the codebase",
  command-heavy → "Ran commands", edit-heavy → "Edited files", mixed →
  sensible; its "Worked through N steps" fallback counts **all**
  non-reasoning steps including `other`-family tools — Task/agent spawns,
  MCP tools — so it never claims fewer steps than the header meta) + a
  mono `N steps · 1m 12s` meta (duration from step
  `started_at`/`completed_at`, omitted when < 1s / not derivable, e.g.
  hydrated transcripts) + a Details/Hide toggle. On settle the block
  re-renders collapsed (any mid-stream expansion resets).
- **Shared step rows.** Both states expand to the same mono rows: a dim
  short verb (`read`/`grep`/`edit`/`run`/`think`), a truncating summary
  (from `describeToolCall` / the thought's first line), and a dim
  right-aligned meta (`2 hits`, `+9 −1`, `ok`, `running`, `failed`).
  Working dims completed steps (~0.6) and keeps the running one full
  opacity; settled shows all green checks. Clicking a step row expands
  its full detail inline (`ToolCallBody` / `DiffView` for tools, the
  thought text for reasoning) — there is no side panel for tool output.
- **What stays standalone (breaks the run).** Approval-gated tool calls
  (`approval_request_id` set — the inline approval footer must render),
  TodoWrite / task-summary tools (`TaskSummaryCard` stays a visible
  receipt row), `permission_request`/plan/AskUserQuestion rows, assistant
  and user prose. Errored tool calls **do** fold in but stay
  discoverable: a red ✕ step row, and the settled header appends a
  subtle red `· N failed` to the meta. A lone reasoning run with no
  tools keeps rendering as `ReasoningBlock`s (its live streaming text is
  better served there); a lone settled tool call keeps rendering as a
  single `ToolCallCard` (GROUP_MIN is 2 for settled runs, relaxed to 1
  only for a single live working step). `ToolGroupCard` is retired.
- **Timestamps.** `ToolCallItem` gained optional `started_at` /
  `completed_at`, stamped by the pure reducer from its injectable
  `Clock` (backward-compatible optionals so persisted transcripts still
  parse); these plus `ReasoningItem`'s `started_at`/`duration_ms` feed
  the settled duration.

`ChatTranscript` is a thin shell that derives `showThinking`, forwards
the optional `sessionStartedAt` (top session-start divider), and sizes
the list. jsdom tests assert on the slot builder and rendered DOM
(see `MessageList.test.tsx` / `MessageList.virtualization.test.tsx` /
`transcript-slots.test.ts`).

### Navigation trail (turn rail)

`MessageTrail.tsx` renders a slim **navigation trail** in the
transcript's left gutter — one tick mark per user turn — so long threads
can be scanned and jumped without scrubbing the scrollbar. It lives
as a sibling of LegendList, absolutely positioned over the dead left
margin of the shared `CHAT_COLUMN` transcript column, so it
never overlaps text. It needs **no new setting** — it is part of the
already flag-gated pane and simply hides on short threads.

- **Data source is the virtualizer's first visible index.** The
  pure helpers in `message-trail.ts` (`buildTrailEntries` /
  `deriveActiveTrailIndex`) derive one entry per user-turn slot and the
  active turn from LegendList's `onFirstVisibleItemChanged` callback. This
  tracks the virtual range without observing or mounting every row.
- **Threshold.** Hidden entirely below `TRAIL_MIN_TURNS` (3). The
  subscribing rail only mounts past the threshold, so short threads pay
  nothing for visibility tracking (the engine's tracking is pay-for-use).
- **Bounded density.** A very long thread downsamples evenly to a
  height-derived tick cap (≤ `TRAIL_MAX_TICKS`, 60) so the gutter never
  overflows; the in-view turn's tick is always re-injected so the active
  turn stays represented.
- **Jump behavior.** Clicking a tick calls LegendList's index-based
  `scrollToIndex({ animated: false, viewOffset: 10 })`. The target need not
  be mounted; the virtualizer resolves it through its measured position
  model and mounts the destination range.
  The rail overlays the viewport's left gutter but is its **sibling**, so
  its `<nav>` forwards `onWheel` manually — otherwise wheeling over that
  28px strip would be a dead zone. The forward is two steps, both
  required: dispatch a real `wheel` event on the list node, then write
  `scrollTop` to perform the actual scroll
  (a synthetic wheel event has no default scroll action). Ticks are real `<button>`s (`Enter`/`Space`,
  focus-visible, `aria-label` "Jump to turn N: …"); the rail is a
  `<nav aria-label="Conversation turns">`. Hovering a tick shows one
  shared preview card (prompt + reply start) after a short delay.
- **Virtualization-safe.** Active tracking and jumps use list indices, never
  DOM discovery, so the trail works across unmounted history. Unit tests cover the pure helpers
  (`message-trail.test.ts`) and the component (`MessageTrail.test.tsx`).

## Subagent view (cross-provider)

When a provider session delegates to subagents, the chat pane shows a
**Subagents orchestration card** in the transcript and lets the user
**enter** a subagent for a read-only, in-pane drill-in of its own stream.
One canonical model drives all three providers. Design spec:
`docs/plans/assets/Subagents.dc.html`; locked decisions:
`docs/archive/subagent-view.md`.

### Canonical event model (`agent_provider/events.rs`)

Two additive, backward-compatible changes to `ProviderRuntimeEvent`
(every new field is `#[serde(default)]`, so payloads persisted before
subagents existed still deserialise, and the frontend merges non-null
fields into per-subagent view state):

- A new event `SubagentUpdated { thread_id, subagent }` carrying a
  merge-able `SubagentSnapshot`. Serialised (snake_case) fields:
  `subagent_id` (stable demux key, required), `parent_item_id?`,
  `name?`, `agent_type?`, `model?`, `status`
  (`pending|running|completed|failed|stopped`), `activity?`,
  `result_text?`, `tool_use_count?`, `total_tokens?`, `duration_ms?`,
  `provider_ref?`. All but `subagent_id` / `status` are optional because
  providers dribble a subagent's identity out across many events.
- A `subagent_id: Option<String>` tag on `ContentDelta`,
  `ItemCompleted`, and `RequestOpened`. Non-null routes the (otherwise
  unchanged) item / delta / approval into that subagent's sub-transcript
  instead of the parent flow — so the drill-in reuses every existing
  renderer, and a child's permission request still bubbles into the
  parent view (labelled "from subagent X") rather than stalling the turn
  invisibly.

Subagent events are emitted thread-scoped under the **parent**
`thread_id`; the `subagent_id` inside the payload is the demux key. They
persist to `agent_chat_messages` under the parent thread as usual
(`should_persist_event` also persists `SubagentUpdated`), so DB hydrate
replays cards + child transcripts through the same reducer with **zero
schema migration**. The TypeScript mirror lives in `src/tauri/events.ts`.
The backend also consumes these `SubagentUpdated` statuses to keep the
left-sidebar working spinner alive while subagents outlive the parent
turn — see [Sidebar status indicators](#sidebar-status-indicators).

### Per-provider keying + mapping

- **Claude** (`claude/translate.rs`, keying = spawning
  `parent_tool_use_id`): an assistant `tool_use` block named `Agent`
  **or** `Task` spawns the row; every SDK message's top-level
  `parent_tool_use_id` routes inner items into the sub-transcript; the
  previously-dropped `task_started` / `task_progress` / `task_updated` /
  `task_notification` system events become status + activity + usage
  (mapped via a `task_id → tool_use_id` table), with the background
  `async_launched` tool_result deferred to a later `task_notification`
  for completion. The sidecar sets `agentProgressSummaries: true` (the
  only sidecar change) so `task_progress.summary` feeds the activity line.
- **Codex** (`codex/{protocol,translate,session}.rs`, keying = child
  `threadId`): a persistent per-session demux routes wire events by
  `params.threadId`; `collabAgentToolCall` / `subAgentActivity` map to
  spawn/lifecycle + `agentsStates[..].message` activity (raw rendering
  suppressed); child `thread/started` / `turn/*` update subagent status
  and never the parent turn. `thread/started`, `turn/started`, and
  `turn/completed` decode **both** the legacy flat and the v2 nested wire
  shapes (custom `Deserialize`), and the success arm accepts both
  `succeeded` and `completed`.
- **OpenCode** (`opencode/{protocol,translate,sse,client}.rs`, keying =
  child `sessionID`): an `SseRouter` grows a watched-session set (parent
  ∪ transitive descendants via `SessionInfo.parentID`) instead of
  dropping child-session events; the `task` tool part decodes
  `state.metadata.{parentSessionId,sessionId,model}` (note the camelCase
  lowercase-`d` keys) and `time.{start,end}`, emitting the row on the
  first `running` update (`pending` carries no metadata) and stripping
  the `<task_result>` / `<task_error>` envelope for `result_text`. Child
  `permission.asked` bubbles as a `RequestOpened` and its reply is POSTed
  to the **child** session's permissions endpoint. `question.asked`
  remains a known gap (a different reply endpoint the current
  `ApprovalDecision` model can't express — decoded as `Other`, no
  regression).

### Frontend (`src/lib/agent-chat/`, `src/components/chat/`)

- `types.ts` adds `SubagentView` and a `SubagentRunItem`
  (`kind: "subagent_run"`) `ChatViewItem`; `reducer.ts` merges snapshots
  (non-null wins, status is monotonic and never regresses to pending) and
  routes `subagent_id`-tagged events into the subagent's own `items` via
  shared item-builders (one card per contiguous spawn group; a new turn
  ⇒ a new card; per-subagent cap 500). Pure helpers live in
  `subagents.ts`; `subagent_run` is a standalone unfoldable transcript
  slot.
- `SubagentsCard.tsx` — the orchestration card: aggregate header
  ("N tasks · running in parallel" / "X done · Y active"), one row per
  subagent (status spinner/check/x, name + mono model, shimmering
  activity line while running / muted result when done, mono
  `elapsed · N tools` meta, Enter button, chevron), an inline "Recent
  activity" peek (last 3 child tool rows + "Enter subagent"), and the
  footer note. The card root carries `data-subagent-card={item.id}` so
  the docked activity bar (below) can locate + scroll to + flash-
  highlight it by plain DOM query — no prop plumbing through
  `MessageList`/`ChatTranscript` needed.
- `SubagentView.tsx` + `SubagentBreadcrumb.tsx` — the read-only drill-in:
  a `← Orchestrator › ⟨ordinal⟩ Name` breadcrumb with model chip and
  right-aligned blinking status, a tone-tinted read-only banner, the
  sub-transcript folded through `buildTranscriptSlots` + the existing
  renderers, and a live shimmer tail while running.
- `AgentChatPane.tsx` holds `viewMode` (orchestrator ↔ subagent) local
  state; entering swaps the transcript body and the sub-header for the
  breadcrumb, Esc / back returns, and the composer stays parent-bound
  with the placeholder "Steering goes to the orchestrator…".

### Docked live activity bar (`SubagentActivityBar.tsx`)

Replaces the old pane-header / title-bar "N subagents running" pills
(removed — they read as a broken tab strip entry once the drill-in and
orchestration card already say the same thing). One docked bar per
thread, mounted in `AgentChatPane.tsx` between the transcript and the
composer, hidden while `enteredSubagentId` is set (design: the bar only
shows in the conversation view) and rendered as `null` entirely while
idle — no resting state.

- **Whole-thread rollup.** `runningSubagentEntries(messages, streaming)`
  (`subagents.ts`) flattens every `running`/`pending` subagent across
  **every** `subagent_run` card in the thread, tagging each with its card
  id and a `from task N` label (omitted when the thread has only one card
  — there is nothing to disambiguate). The bar's count and expand-list
  both key off this list, so scattered work from different replies still
  reads as one signal.
- **Live activity respects the end of the run.** `isLiveActivity` (the
  shared predicate behind `runningSubagentEntries` and
  `countRunningSubagents`) drops rows carrying `backgroundTask` once
  `streaming` is false. That flag mirrors the wire
  `SubagentSnapshot.background_task` (see [Sidebar status
  indicators](#sidebar-status-indicators)) and is merged **stickily** in
  `mergeSnapshot` — a later snapshot that omits it never promotes the row
  back to "real subagent". Without this, a background shell command that
  never reports a terminal status kept the bar and its spinner up forever
  after the turn settled, including across a restart (subagent snapshots
  are persisted and hydrate-replayed). `AgentChatPane` passes the thread's
  streaming flag; mid-run nothing changes.
- **1 running** → the whole bar is one click target labelled "View";
  clicking jumps straight to that subagent's card.
- **>1 running** → the action chip reads "Show all" / "Hide" with a
  rotating caret; clicking the bar toggles an expand list that opens
  upward (`rise-in` class) above the bar: a header
  ("N subagents running · across this thread · tap one to jump") and one
  row per running subagent (spinner, name, shimmering activity, mono
  elapsed, `from <label>`, chevron) — clicking a row collapses the list
  and jumps to that subagent's card.
- **Just finished** (the running count is observed transitioning from
  `>0` to `0` — never on initial mount or a thread hydrate): the bar
  flashes green for ~2.5s (check icon, "Subagents finished", muted mono
  "all tasks complete · results are in the thread"), then unmounts. The
  flash is itself clickable (design gallery "Jump" CTA): it jumps to the
  card of the last subagent that was still running before the
  transition.
- **Jump + highlight.** `AgentChatPane.tsx` sends a keyed jump request through
  `ChatTranscript` to `MessageList`. The list resolves the matching slot and
  calls LegendList's `scrollToIndex`, then applies `subagent-card-highlight`
  after the destination row mounts
  (ember `box-shadow` ring, ~1100ms, token-driven per the design-system
  no-hardcoded-color rule) via plain `classList`. The bar itself never
  touches the DOM directly; off-screen targets are valid because navigation
  is index-based.
- All tones consume design-system tokens (running = `status-working`,
  finished flash = `status-open`, highlight ring = `accent-ember`); the
  top-edge sweep animation (`cm-sweep` in `globals.css`) and `.cm-blink`
  (still used by the breadcrumb's status dot) honor reduced-motion.

### Run-state settlement (issue #153)

The transcript layer's only "running → terminal" transition used to be a
terminal `subagent_updated` snapshot. When the Claude adapter's demux
loses track (sidecar restart/resume) the spawning `Task` tool's raw
parent-scoped `tool_result` flows through and no terminal snapshot ever
arrives, so a subagent could show "N running" forever — persisted that
way, and resurrected on hydrate. The frontend now settles run state from
several additional signals (the transcript-layer counterpart of the
backend `SubagentTracker`; pure helpers in `subagents.ts`, unit-tested):

- **`tool_result` derivation.** A parent-scoped `tool_result` (no
  `subagent_id`) whose `tool_use_id` matches a still-running subagent's
  demux id **or** its `parentItemId` (the spawning tool_use/call id,
  merged from `SubagentSnapshot.parent_item_id`) settles that subagent
  (`completed`, or `failed` on error). A subagent-**tagged** `tool_result`
  routes into the sub-transcript and never settles the row. The matching
  Workflow tool's own `tool_result` likewise settles the run and its
  in-flight phase agents.
- **Forced settle.** `session_state_changed{closed|error}` and a new user
  turn (mirroring `SubagentTracker::clear_thread` on send) interrupt every
  running/pending subagent and stop running workflows; a queued follow-up
  (sent while streaming) leaves the active turn's subagents alone.
- **Interrupted-on-hydrate.** The hydrate reconciliation force-settles any
  subagent still running at end-of-replay to a **view-only `interrupted`**
  status (`SubagentViewStatus = SubagentStatus | "interrupted"`, never on
  the wire; muted-amber tone, a non-spinning glyph) and stops a still-
  `running` workflow — a resumed transcript never renders a live spinner.
  A `pending_approval` workflow is preserved (a resting state awaiting the
  user, not a runaway spinner).
- **`statusAssumed` + revive-on-running-snapshot.** Every inferred
  settlement stamps `statusAssumed`, so a later **real** `running`
  snapshot (e.g. a Claude background task re-emitting `running` via
  `task_progress`) revives the row back to `running`, and a later real
  terminal snapshot wins by rank and clears the flag. This makes the
  inference self-healing on live streams and on replay.
- **`runtime_warning` notices.** A small classifier (`runtime-notice.ts`)
  promotes the user-facing runtime warnings — a provider rate-limit
  rejection (`rate_limit_info.status === "rejected"`), the SDK's
  enumerated `assistant error: …` — into a compact muted-amber inline
  `runtime_notice` transcript row; all other `runtime_warning`s stay
  console-only debug noise.

The dev mock seeds one subagent turn in the demo transcript and exposes
`window.__codemuxChatMock.streamSubagents()` for a live two-subagent
lifecycle over the real per-thread Channel. Because the seeded transcript
is served through the hydrate path, its still-running subagent (and the
mid-run `/workflow` demo) correctly settle to `interrupted`/`stopped` on
load; the live running bar remains demoable via `streamSubagents()`.

**Workflow runs are a Claude-only relative of this system.** When a
Claude session runs a top-level `Workflow` tool_use (a script that
coordinates many subagents across named phases), the same translate
layer emits a separate `workflow_updated` event instead of treating it
as an ordinary subagent, and top-level subagents spawned while that
workflow is active get stamped with a `workflow_id`/`phase` so they
route into the workflow's phases rather than the generic Subagents card.
The chat pane renders this as a `WorkflowRunCard` (approval / running /
summary) and a conditional **Orchestration** right-panel tab instead of
the inline Subagents card + drill-in described above. See
`docs/features/workflow-orchestration.md` for the full model; this file
does not duplicate it.

## Dead-run detection & interrupted turns

Issue #154. A run can die without emitting a terminal event — the
sidecar / app-server / shared server is killed by a laptop sleep, a
crash, or a provider usage-limit cutoff. Before this work every "working"
indicator stayed live forever (`pane_statuses` stuck on `Working`, the
thread `streaming` flag true, the background-browser `LIVE` chip never
released) and the user had to *know* the run was dead and manually type
"continue". Three additive parts close the gap.

### Synthetic child-exit terminal event

When a provider's child process dies unexpectedly **mid-turn**, its
watchdog now settles the in-flight turn before tearing the session down:

- Claude (`claude/session.rs`) and Codex (`codex/session.rs`) per-session
  child-exit watchdogs recover the `active_turn` (set during both
  `Running` and `WaitingApproval`) **before** clearing it and emit a
  synthetic `TurnCompleted { status: Error { subtype: "child_exited" } }`,
  then the existing `SessionStateChanged::Error`.
- OpenCode has no per-session child; its SSE listener (`opencode/sse.rs`)
  reconnects up to `MAX_RECONNECT_ATTEMPTS` (5) and, on giving up, now
  runs the same terminal sequence for the session (previously it emitted
  only a `RuntimeWarning`). Whether a turn is in flight is tracked on
  `EventContext.turn_active`, armed by `send_turn` **only after the
  `prompt_async` POST succeeds** (a failed POST never started a turn, so a
  later SSE give-up must not synthesize a `child_exited` completion for a
  phantom turn) and disarmed when a parent `TurnCompleted` / terminal
  session state flows.

### Live recovery (no app restart)

The synthetic completion is only half the story: after the watchdog fires,
the next send on that thread must actually rebuild the dead session. All
three providers now converge on one shape:

- Each dead session is flagged so the provider's `has_session` reports it
  **absent**, which routes the next send through `ensure_live_session`
  (`commands/agent_chat.rs`) to rebuild it — the same choke point that
  handles post-restart auto-resume. OpenCode's SSE listener flips a shared
  `dead` `AtomicBool` on give-up; Claude and Codex set a per-session `dead`
  `AtomicBool` in their child-exit watchdog. It is deliberately **not**
  keyed off `SessionStatus::Error`: Codex sets `Error` on a mere *turn*
  failure (a live child that refused a prompt) and on a non-retryable
  server error, and Claude sets it from a `session-error` notification —
  none of which mean the child is gone. Only the watchdog (a confirmed dead
  child) sets `dead`, so a recoverable error stays recoverable in place.
- The dead session stays in the provider's `sessions` map (only
  `stop_session` removes it), so each provider's `start_session` now
  **evicts-and-replaces** a dead entry under the write lock (atomic
  check→remove against a concurrent rebuild) and shuts the corpse down
  cleanly, instead of failing the rebuild with the "session already exists"
  `ValidationError`. Without this the OpenCode rebuild in particular
  dead-ended: `has_session` said absent, `ensure_live_session` called
  `start_session`, and the `contains_key` guard found the corpse and
  errored — every send failed permanently.
- **OpenCode resume cursor.** OpenCode keeps sessions on disk, so a rebuilt
  session can readopt its old server-side session id and keep the
  conversation context. The adapter emits a `ResumeCursorUpdated` carrying
  the OpenCode session id on every session start (it has no SDK cursor
  notification of its own), so `forward_event` persists it on the
  `agent_chat_sessions` row exactly like Claude/Codex. On rebuild,
  `ensure_live_session` threads `{"resume": <session_id>}` back in and
  `OpenCodeSession::start` **validates** the id with
  `GET /session/{id}/message?limit=1` before readopting it — a 404/error
  means the id is stale, so it falls back to a fresh `POST /session` (the
  visible transcript still hydrates from the DB either way). Best-effort:
  `supports_session_resume` is now `true` for OpenCode.
- **OpenCode shared-server respawn.** The rebuild only works if the shared
  `opencode serve` child is actually reachable — and the SSE give-up that
  triggers this whole path usually means that child *died* (laptop sleep,
  crash), while `OpenCodeServerManager` cached its handle forever (nothing
  in production calls `stop()`). `ensure_running`
  (`opencode/manager.rs`) therefore probes the cached server with a
  short-timeout authenticated `GET /` on every call: any HTTP response
  (401/404 included) proves it alive. Failure kinds are deliberately
  distinguished (two-strike policy): a connect error (dead loopback port
  refuses instantly, deterministically) is an immediate dead verdict,
  while a probe timeout (or other transport error) only condemns after
  one retry also fails — a server that is merely wedged for a moment
  must not be SIGKILLed, since that invalidates every live session's
  handle at once. A confirmed corpse is dropped (`kill_on_drop` reaps
  it) and respawned under the same lock, so racing callers serialize
  onto the fresh server. The respawn mints a new port +
  password — fine, because the only sessions holding the old handle are
  the dead ones this scenario invalidated, and the disk-persisted session
  readopt above recovers the conversation on the new server. An HTTP
  probe (not `Child::try_wait`) on purpose: `try_wait` needs
  `&mut Child` behind the shared `Arc`, and can't detect a
  hung-but-alive server anyway.

The event list is built by the shared pure helper
`child_exit_events(thread_id, active_turn, message)` in
`agent_provider/events.rs`. **Ordering is load-bearing**: `TurnCompleted`
first so the activity + pane-status trackers see the turn settle while the
thread state is intact, then `Error` to tear tracking down. When no turn
is in flight (`active_turn` is `None`) only the `Error` event is produced —
a child that dies while idle has no dangling turn, so no spurious
`interrupted` signal reaches the frontend. Because `turn_completed`
persists (`should_persist_event`), hydrate after a restart replays a
settled turn. The subtype string `"child_exited"`
(`CHILD_EXITED_SUBTYPE`) is the frontend's key; a user-initiated stop
produces subtype `"interrupted"` instead, so it never triggers the
Continue affordance.

### Stall watchdog (`RunStalled`)

A child that is alive but silent (usage-limit cutoff, wedged tool call)
emits no terminal event at all, so a heartbeat is needed. A new
Tauri-managed `RunActivityTracker` (`commands/agent_chat.rs`, mirroring
`SubagentTracker`) records a wall-clock `last_event` per thread inside
`forward_event`, via the pure `activity_update(current, event, now)`:
mid-turn events (running, deltas, items, subagent/workflow updates,
dispatched queue turns, resolved requests) stamp `now`; approval prompts
(`RequestOpened` / `WaitingApproval`) additionally **pause** the stall
clock (waiting on the user is expected silence); turn/session settlement
removes the entry so the map stays bounded. Wall clock (`SystemTime`) on
purpose — after suspend/resume the elapsed silence includes the sleep
window, which is exactly the wake-from-sleep case the issue asks for.

`spawn_stall_watchdog` (started from `lib.rs` right after
`spawn_event_bridge`, same `agent_chat_enabled` gate) sweeps every
`STALL_SWEEP_INTERVAL` (30s) and, for any thread mid-turn and silent for
at least `STALL_THRESHOLD` (600s — long enough that a quiet legitimate
tool call never trips it), forwards a new additive
`ProviderRuntimeEvent::RunStalled { thread_id, silent_for_secs }`. The
event is **transient**: never persisted, re-emitted each tick while still
stalled (keeps the surfaced duration fresh), leaves the sidebar dot
untouched (`map_event_to_pane_status` returns `None` for it — the run may
be fine), and is cleared by the reducer on the next real activity. It
renders as an amber "No activity for Nm — the agent may have stopped."
tail notice in `MessageList` (design tokens only).

### Interrupted turn + Continue affordance (frontend)

A new `interrupted` flag on `ChatThreadState`
(`src/lib/agent-chat/types.ts`) drives the recovery UX:

- **Hydrate detection** — the pure `lastTurnUnsettled(payloads)`
  (`src/lib/agent-chat/hydrate.ts`) returns true when the last persisted
  `user_message` envelope has no later `turn_completed`; `replayPayloads`
  ORs that with the replay-observed `child_exited` completion so both the
  "app died mid-turn" and "watchdog settled the turn" cases surface.
- **Live-run guard on hydrate** — `lastTurnUnsettled` cannot distinguish
  "run died mid-turn" from "run is healthy but the pane wasn't watching":
  only the active workspace renders, so switching workspaces unmounts the
  inactive pane while its run keeps going and keeps persisting rows
  (subagent `item_completed`s in particular grow the disk transcript past
  the frozen in-memory slice, which is exactly what lets the remount
  hydrate guard pass). Before this guard, switching back to a workspace
  mid-run reliably showed a false "Run interrupted" divider + Continue
  chip until the next live event cleared it. The remount hydrate effect
  (`AgentChatPane.tsx`) now probes the backend **after** fetching the
  payloads (ordering matters: if the turn settles between the two calls,
  the settling `turn_completed` row is already in the fetched payloads,
  so a stale "alive" answer can't mislabel a finished run) via the new
  `agent_chat_turn_active` command — true iff a live (non-dead) session
  is bound to the thread and its `active_turn` is set. The result is
  threaded as `hydrateThread(threadId, payloads, { runLive })` →
  `replayPayloads(payloads, { runLive })`: `runLive: true` forces
  `interrupted: false` (suppressing both the unsettled-tail heuristic and
  a replay-observed `child_exited` — an in-flight turn means the run is
  alive/recovered) and sets `streaming: true` so the streaming marker
  shows instead of the divider. A probe error degrades to `false`
  (previous behavior). Backend: default trait method
  `AgentProvider::turn_active` (false), implemented for Claude and Codex
  as a cheap in-memory `sessions` map check (`!is_dead()` +
  `active_turn.is_some()`) and for OpenCode via the session's SSE
  event-context `turn_active` flag behind the same absent/dead guards.
- **Live detection** — the reducer sets `interrupted` on a
  `turn_completed` with `subtype === "child_exited"` and (belt-and-braces)
  on a `session_state_changed{error}` while streaming. It clears on the
  next running / dispatched-queue / content event, on the optimistic
  user-message append, and on a **success** `turn_completed`. Clearing on
  success matters because the stdout-reader / exit-watchdog race can persist
  BOTH a synthetic `child_exited` completion and the real success completion
  for one turn (the success is the later of the two, since the watchdog only
  synthesizes while `active_turn` is still set); without it a
  genuinely-finished run stays labelled "Run interrupted" forever. Applied
  in the reducer, so it holds on live events AND on hydrate replay.
- **Render** — `MessageList` shows a "Run interrupted" tail divider
  (cloning the `SessionStartMarker` hairline pattern, amber label) while
  not streaming, and `Composer` shows a one-click "Continue run" chip in
  its attachment strip. The chip calls `AgentChatPane`'s
  `handleContinueRun`, which routes `handleSubmit("Continue", { continueRun:
  true })` through the normal `agentChatSendTurn` path — same
  optimistic-bubble + rollback + backend auto-resume as a manual send, with
  two deliberate differences: the Continue send **does not consume the
  composer** (it leaves the user's in-progress draft and staged attachment
  chips intact and sends a plain "Continue" with no images — a resume must
  not eat a half-typed next message), and the send-failure rollback
  **restores `interrupted`**. The optimistic append clears `interrupted`, so
  `removeUserMessageByNonce` takes the pre-append value and re-arms the flag
  on failure; otherwise one failed Continue click would drop the divider +
  chip with no recovery affordance left.
- **Deliberate Stop vs interrupted.** A user Stop settles its turn with a
  persisted `turn_completed{subtype:"interrupted"}` (Claude via the sidecar
  `session-ended`, Codex via the app-server `turn/completed`), which flows
  asynchronously. `handleStop` now **awaits the interrupt before tearing the
  session down** so the interrupt is ordered ahead of `stop_session`, whose
  graceful shutdown (EOF → grace → kill) then leaves the settlement time to
  reach the event bridge (which persists from the broadcast independent of
  the child's liveness). Without the await the settlement could lose the
  race and a deliberate Stop would hydrate as "Run interrupted" after a
  restart.

The dev mock exposes `window.__codemuxChatMock.streamRunStalled()` and
`window.__codemuxChatMock.interruptRun()` for browser QA of the amber
notice, the divider, and the Continue chip.

## AskUserQuestion answer reply (PR #165)

When the agent asks a structured question (`AskUserQuestion` →
`RequestOpened { request_kind: "user-input" }`) and the user answers it in
the panel, the transcript now **echoes the chosen answer back as a
right-aligned user reply bubble** (`src/components/chat/UserInputAnswer.tsx`),
instead of leaving only a muted "Answered" marker on the question block. The
conversation therefore reads as a genuine back-and-forth — question from the
assistant, visible reply from the user — rather than a question that silently
resolves in place.

`MessageList.tsx` derives the reply row from the resolved
`user-input`/AskUserQuestion request state (the same request the panel
rendered), so it is a pure render of existing state — no new reducer
`ChatViewItem`, no new backend event, and it survives restart because it is
recomputed from the persisted request/answer rather than stored separately.
The bubble shows the selected option label(s) (multi-select answers render each
choice); the underlying `respond-to-user-input` RPC path is unchanged. The dev
mock seeds an answered AskUserQuestion so the reply bubble can be screenshotted
in the browser pane.

## Current Constraints

- **Flag-gated, default on.** The chat pane is the default interface; the
  Settings → Interface toggle switches back to the classic CLI view. See
  "Interface Gate" above.
- **Single instance per provider.** A user with multiple Codex accounts or
  multiple OpenCode connections sees them collapsed under one rail entry.
  Multi-instance lifting is planned for v2 (the `ProviderInstanceId` shim
  already exists at `src-tauri/src/agent_provider/instance.rs`).
- The event broadcaster uses a bounded channel (default 1024) — slow
  subscribers lose old events. This is deliberate; downstream UI must
  treat the stream as live-only.
- **Image attachments in `send-turn`** route through the `images` array
  on user turns as staged-file refs (`{ path, media_type }` — see the
  attach-time staging note above; the command layer reads the bytes back
  for the provider adapters, whose `SendTurnInput`/`ImageInput` contract
  is unchanged); the SDK paths are wired but multi-modal-everywhere is
  still settling.
- **OpenCode credential management lives in OpenCode itself.** Codemux
  never reads or writes upstream API keys.

## Important Touch Points

- `src-tauri/src/agent_provider/mod.rs` — module re-exports.
- `src-tauri/src/agent_provider/types.rs` — newtypes, enums, input/output
  structs.
- `src-tauri/src/agent_provider/events.rs` — `ProviderRuntimeEvent` and its
  sub-enums.
- `src-tauri/src/agent_provider/errors.rs` — `ProviderError` plus
  `SerializableProviderError`.
- `src-tauri/src/agent_provider/provider.rs` — the `AgentProvider` trait
  itself.
- `src-tauri/src/agent_provider/codex/mod.rs` — `CodexAgentProvider`.
- `src-tauri/src/agent_provider/codex/protocol.rs` — wire-level types for
  the `codex app-server` JSON-RPC protocol.
- `src-tauri/src/agent_provider/codex/translate.rs` — pure translation
  functions from Codex notifications / server-initiated requests to
  `ProviderRuntimeEvent`.
- `src-tauri/src/agent_provider/codex/session.rs` — per-thread session
  state plus background tasks that forward events. Its `account/read` gate
  treats login as missing only when the active provider requires OpenAI auth
  and the response has no account; `requiresOpenaiAuth` alone is provider
  metadata and remains true for a valid ChatGPT login.
- `src-tauri/src/agent_provider/codex/auth.rs` — installed and one-shot auth
  probes (`codex --version`, `codex login status`).
- `src-tauri/src/json_rpc_child/mod.rs` — the `JsonRpcChild` helper.
- `src-tauri/tests/json_rpc_child.rs` — helper tests.
- `src-tauri/tests/codex_adapter.rs` — Codex adapter integration tests.
- `src-tauri/tests/helpers/fake_rpc_child/main.rs` — in-tree JSON-RPC peer
  used as the JsonRpcChild fixture.
- `src-tauri/tests/helpers/fake_codex_app_server/main.rs` — scripted
  fixture that impersonates the `codex app-server` subprocess.

The three `fake_*` helper `[[bin]]` targets are gated behind the
`test-fixtures` cargo feature (`required-features` in
`src-tauri/Cargo.toml`). Tauri's CLI bundles every ungated `[[bin]]` of
the app package, so without the gate the fixtures shipped in the
deb/rpm/AppImage at `usr/bin` (through v0.9.1). A self-referential
dev-dependency turns the feature on for `cargo test`, so plain
`cargo test` still builds the fixtures and `env!("CARGO_BIN_EXE_fake_*")`
resolves — no `--features` flag needed anywhere.

## Notes

- Keep this file about current truth, not future plans. Future chunks (Claude
  adapter, Codex adapter, chat pane, orchestration wiring) will each own
  their own `docs/plans/` entry at time of work.

## Sidecar (claude-agent)

Location: `sidecar/claude-agent/`.

**Why it exists.** The Claude integration is built on a SDK that only
runs under a JavaScript runtime. Rather than reverse-engineer that
SDK's wire protocol and maintain a Rust port, we run it inside a tiny
TypeScript subprocess and talk to it over JSON-RPC. This is the
officially supported integration path, and it means Codemux's Rust
side stays provider-agnostic.

**Current state.** Full SDK host. The sidecar depends on
`@anthropic-ai/claude-agent-sdk` and exposes 13 JSON-RPC methods
(session lifecycle, turn streaming, approvals, probes — see "Exposed
RPCs" under "Claude Agent SDK integration" below). The original
`ping` scaffold method remains as the liveness probe.

**Toolchain.** [Bun](https://bun.sh) 1.3+ is the sole dependency. Bun
handles install, test, and `bun build --compile` to produce a
standalone binary per target platform.

**How to build locally.** From the repo root:

```sh
bash scripts/build-claude-sidecar.sh
```

or directly:

```sh
cd sidecar/claude-agent
bun install
bun run build:all
```

The per-target binary is staged at
`src-tauri/binaries/codemux-claude-sidecar-<target-triple>`, which is
where Tauri's `resources` glob picks it up. The Rust integration
tests (`src-tauri/tests/sidecar_ping.rs`) look for the same path and
skip cleanly (with a build hint) if the binary is missing.

**How it's shipped.** `tauri.conf.json`'s `bundle.resources` array
includes `binaries/codemux-claude-sidecar-*`. Tauri's packager picks
up the per-triple variant and embeds it into the AppImage / deb / rpm
/ NSIS installer under `usr/lib/codemux/binaries/` on Linux and next
to `codemux.exe` under `binaries/` on Windows. (It originally shipped
as an `externalBin` under `usr/bin/`; moved to a resource because
linuxdeploy's patchelf step corrupts the ~100 MB bun-compiled binary
during AppImage bundling — see commit 025fa19.) At runtime, the
`setup()` hook in `src-tauri/src/lib.rs` calls
`resolve_sidecar(resource_dir)`
(`agent_provider::claude::sidecar_path`), which prefers an existing
`CODEMUX_CLAUDE_SIDECAR_PATH` override, then the
`<resource_dir>/binaries/` copy, and — **in debug builds only** —
falls back to the dev-tree `<CARGO_MANIFEST_DIR>/binaries/` copy so
`npm run tauri:dev` (whose resource dir does not carry the sidecar)
still resolves it and the agent-chat GUI can spawn. The winner is
pinned into the `CODEMUX_CLAUDE_SIDECAR_PATH` env var so the adapter
(which has no `AppHandle` access at construction time) can find the
same binary. The debug fallback is compiled out of release via
`debug_assertions`, so the release resolution stays resource-dir-only;
when the binary exists nowhere the hook leaves the env var unset and
the Claude provider stays unconfigured rather than panicking. The release
workflow (`.github/workflows/release.yml`) installs Bun and
pre-stages the binary so tauri-action finds it before bundling. CI
(`.github/workflows/ci.yml`) does the same, with a zero-byte
placeholder fallback for constrained runners.

**Packagers note.** Downstream packagers (AUR, custom distros) MUST
copy the sidecar from the Tauri resource layout (`usr/lib/codemux/
binaries/codemux-claude-sidecar-<triple>` on Linux) into their own
package — repackaging only `usr/bin/codemux` will leave the agent-
chat Claude provider unable to find its sidecar and the first send
fails with `provider_not_configured: Claude`.

**Protocol.** Newline-delimited JSON-RPC 2.0 over stdin/stdout. The
Rust side spawns the sidecar through
[`JsonRpcChild`](../../src-tauri/src/json_rpc_child/mod.rs) — the same
helper the Codex adapter uses — so adding new methods is a matter of
registering handlers in `buildMethods` in
`sidecar/claude-agent/src/methods/index.ts` (dispatched from `main.ts`).

**Workspace env injection.** `agent_chat_start_session` overlays the
chat pane's workspace env onto `StartSessionInput.env` before the
provider consumes it (`workspace_env_overlay` in
`src-tauri/src/commands/agent_chat.rs`): `CODEMUX=1`,
`CODEMUX_WORKSPACE_ID`, `CODEMUX_PANE_ID` (the chat pane),
`CODEMUX_BROWSER_CMD`, `BROWSER`, plus the workspace-level vars from
the terminal path's `workspace_pty_env` helper (reused `pub(crate)` so
the two surfaces stay in lockstep). The Claude and Codex adapters pass
`input.env` through to their per-session child spawns, so the agent's
Bash subprocesses carry `CODEMUX_WORKSPACE_ID` and `codemux browser
open` routes to the agent's own workspace instead of falling into the
control layer's legacy active-workspace path (the pre-fix bug: with
the beta on, browser panes landed in whatever workspace the user was
viewing). Caller-provided env entries always win (insert-if-absent);
an orphaned pane (no owning workspace) injects nothing. OpenCode's
shared long-lived server cannot take per-session env — it relies on
the control layer's cwd fallback (`resolve_workspace_id_by_cwd` in
`src-tauri/src/control.rs`; see `docs/features/browser.md`).

## Claude Agent SDK integration

The sidecar (`sidecar/claude-agent/`) now hosts Anthropic's Claude
Agent SDK in-process. All Claude inference goes through the SDK's
`query()` — Codemux's Rust side never talks to Anthropic directly.

### ToS boundary

Three hard rules, enforced by `sidecar/claude-agent/scripts/check-tos-boundary.sh`
(run on every `bun test` and as a standalone CI step):

1. **No credential reads.** The sidecar must not open, stat, or
   reference `.claude.json`, `~/.anthropic/`, or any file path that
   could contain an OAuth token.
2. **No Anthropic HTTP requests.** The sidecar must not reference
   `api.anthropic.com` or `anthropic.com`. The SDK makes these requests
   itself; the sidecar is a transport only.
3. **No direct `claude` inference.** All inference runs through
   `@anthropic-ai/claude-agent-sdk`'s `query()`. The sole exception is
   `sidecar/claude-agent/src/auth-probe.ts`, which is allow-listed for
   `claude --version` and `claude auth status` subprocess calls.

A fourth rule — no `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` env
reads — is enforced by the same check.

Run the check manually:

```sh
cd sidecar/claude-agent
bun run check-tos
```

### Session lifecycle

One `ClaudeSession` per chat thread, always in streaming-input mode.
The prompt argument to `query()` is an `AsyncPromptQueue<SDKUserMessage>`
that user-facing `send-turn` RPCs push into. A single background task
iterates the returned `Query` and forwards every message to the Rust
side as a `sdk-message` notification — no classification or
translation happens in the sidecar.

Two side-channel notifications are emitted in addition to the raw
`sdk-message` for the two tool uses production integrations special-case:

* `plan-proposed` — fires when an assistant `ExitPlanMode` tool_use
  block lands (and from the permission bridge before denying the tool).
* `user-input-requested` — fires when an assistant `AskUserQuestion`
  tool_use block lands.

### Exposed RPCs

| Method | Purpose |
|---|---|
| `start-session` | Spawn a new ClaudeSession. |
| `send-turn` | Queue a user message onto the session. |
| `interrupt` | Halt the current turn (`query.interrupt`), await the SDK query iterator's exit, and emit `turn-interrupted` instead of `session-ended` — the session survives with the same thread id and the next `send-turn` rebuilds a resumed query. Also auto-aborts any outstanding tool approval via `request-resolved` (deny). |
| `set-model` | Swap the session's default model. |
| `set-permission-mode` | Change the session's permission mode. |
| `update-mcp-tools` | Push an updated MCP tool list into the live session (`query.setMcpServers`); lets servers that come up after `start-session` surface without a chat restart. Empty list removes the codemux MCP. Idempotent. |
| `respond-to-request` | Resolve a pending `canUseTool` approval. Rethrows the SDK's "not found or already resolved" family as `InvalidParamsError` (-32602) so Rust can classify it as `ProviderError::RequestNotPending` — see "Pending requests are not conversation history". |
| `respond-to-user-input` | Answer an `AskUserQuestion` prompt. |
| `initialization-result` | Read the SDK's cached init payload. |
| `stop-session` | Close a session; idempotent. |
| `probe-installed` | Shell out to `<binary> --version`. |
| `probe-authenticated` | Shell out to `<binary> auth status`. |
| `ping` | Liveness probe from the scaffold. |

Deliberately NOT exposed (per the integration research): the ~15
other `Query` methods (`setMaxThinkingTokens`, `applyFlagSettings`,
`supportedCommands`, `supportedModels`, `supportedAgents`,
`mcpServerStatus`, `getContextUsage`, `reloadPlugins`, `accountInfo`,
`rewindFiles`, `seedReadState`, `reconnectMcpServer`,
`toggleMcpServer`, `streamInput`, `stopTask`). These ship as
follow-ups only when UI calls for them. (`setMcpServers` graduated
from this list — `update-mcp-tools` wraps it.)

### Options construction

Exactly 16 SDK `Options` fields are populated (`cwd`, `model`,
`pathToClaudeCodeExecutable`, `settingSources: ["user","project","local"]`,
`effort` (cast through `unknown` for forward-compat), `permissionMode`,
`allowDangerouslySkipPermissions` (only with `bypassPermissions`),
`settings` (when non-empty), `resume`, `sessionId`,
`includePartialMessages: true`, `agentProgressSummaries: true` (so
`task_progress` events carry a human-readable `summary` for the subagent
card's activity line), `canUseTool`, `env: process.env`,
`additionalDirectories` (when non-empty), `extraArgs` (when non-empty)).
The other 30+ fields in the SDK's Options surface are intentionally
left unset — they become features when the UI surfaces them.

### Testing

The sidecar ships with 70 Bun tests across six files: ping/liveness,
session unit tests using a `FakeQuery` injected via
`setQueryFactoryForTests`, permissions tests exercising the canUseTool
bridge, MCP-bridge tests, respond-to-request tests, and Stage 3
real-tools coverage. The Rust
side has 7 end-to-end integration tests (`sidecar_sdk.rs`) that spawn
the compiled binary and cover the paths that don't require real
Anthropic auth. Testing a real session is a manual smoke test — it
requires a logged-in `claude` binary and live network egress, so it
lives outside CI.

## Claude adapter

`src-tauri/src/agent_provider/claude/` implements `AgentProvider` by
driving the claude-agent sidecar via `JsonRpcChild`. Mirror of the
Codex adapter's structure (`mod.rs`, `protocol.rs`, `translate.rs`,
`session.rs`, `auth.rs`) with one addition — `sidecar_path.rs`
resolves the bundled sidecar binary at runtime.

### Architecture

One sidecar subprocess per chat thread. Deliberately NOT multiplexed:

- Per-session memory isolation.
- One session's sidecar crashing doesn't affect the others.
- Simpler state model — nothing cross-session inside the sidecar.

The adapter spawns the bundled binary (at
`src-tauri/binaries/codemux-claude-sidecar-<triple>` or a path from
`CODEMUX_CLAUDE_SIDECAR_PATH`), sends `start-session` with the
user's claude binary path, then wires two background tasks: one
consuming the sidecar's notification broadcast, one consuming its
incoming-request mpsc (currently unused — the sidecar doesn't issue
server-initiated requests).

### Event translation

The sidecar forwards SDK messages opaquely as JSON. `translate.rs`
does all classification on the Rust side, which keeps the sidecar's
surface tiny and lets the Rust side evolve independently. Two paths:

- Sidecar-specific notifications (`session-configured`,
  `request-opened`, `plan-proposed`, `user-input-requested`,
  `session-ended`, `session-error`, `turn-interrupted`,
  `resume-fallback`) map directly to trait events. `resume-fallback`
  (stale-resume self-heal — see "Stale-resume self-heal" above) maps to
  `ResumeCursorUpdated(null)` + a `resume-fallback: `-prefixed
  `RuntimeWarning`. `turn-interrupted` (emitted only by the explicit interrupt
  RPC) maps to `TurnCompleted(interrupted)` + `SessionStateChanged(Ready)`,
  keeping the session alive; a spontaneous `session-ended
  {reason:"interrupted"}` still maps to `Closed`.
- SDK messages are structurally classified by `type` (and `subtype`
  for `system:*`). 15+ known shapes; unknown variants always surface
  as `RuntimeWarning` with the raw payload preserved. The
  notification task wraps translation in `catch_unwind` so a
  translation bug can't silently kill the event stream.

### Configuration

```rust
let provider = ClaudeAgentProvider::new(ClaudeProviderConfig {
    sidecar_binary: None,       // None => resolve at runtime
    claude_binary: None,        // None => "claude" on PATH
    event_channel_capacity: 1024,
}).await?;
```

`CODEMUX_CLAUDE_SIDECAR_PATH` overrides the search for testing and
manual override. The capacity default is 1024; smaller tests can use
less.

### Testing

27 integration tests in `src-tauri/tests/claude_adapter.rs` drive
the adapter against a `fake_claude_sidecar` binary that impersonates
the real sidecar's RPCs without involving the SDK. Script-driven
notifications let each test choreograph the exact event sequence.
Two additional tests use the REAL compiled sidecar for
`probe-installed` / `probe-authenticated` with a mock `claude`
binary.

43 unit tests in `translate.rs` / `protocol.rs` / `sidecar_path.rs`
cover SDK-message classification, notification mapping, approval
decision translation, and binary-path resolution.

### Dogfood smoke test

Run manually against a real `claude` CLI + real Anthropic auth:

```sh
# One-time: build the sidecar for the host platform.
bash scripts/build-claude-sidecar.sh

# Run the ignored dogfood test.
cargo test --manifest-path src-tauri/Cargo.toml \
  --test claude_adapter claude_real_session -- --ignored --nocapture
```

The test starts a session, sends "Say hi.", and asserts a content
delta arrives within 60 seconds. Never runs in CI.

## Pane kind registration

The pane tree now carries an `AgentChat` variant alongside `Terminal`,
`Browser`, and `Split`. The enum lives in
`src-tauri/src/state/state_impl.rs` and mirrors into
`src/tauri/types.ts` so the frontend's discriminator pattern stays
consistent:

```rust
PaneNodeSnapshot::AgentChat {
    pane_id: PaneId,
    title: String,
    thread_id: Option<String>,
    provider: Option<ProviderKind>,
    cwd: Option<String>,
}
```

All optional fields default to `None` through `#[serde(default)]`, so
older persisted layouts (or test fixtures produced before this
variant landed) deserialize cleanly. Three serde round-trip unit
tests in the state module guard the schema:
`agent_chat_pane_serde_round_trips`,
`agent_chat_pane_defaults_round_trip`, and
`agent_chat_pane_deserializes_without_optional_fields`.

Pane creation is available on `AppStateStore::create_agent_chat_pane`,
which inserts the new pane by splitting the workspace's currently
active pane horizontally — the same insertion path
`create_browser_pane` uses. `AppStateStore::agent_chat_thread_id` /
`set_agent_chat_thread_id` read and assign an existing binding, while
`set_agent_chat_binding` writes provider + thread together after
`start_session` so a provider handoff cannot leave a pane snapshot routed
through its previous adapter.

The pane renderer at `src/components/chat/AgentChatPane.tsx` is now the
full chat UI (transcript, composer with `+`/`@`/slash popups, mode pill,
streaming indicators, content blocks, inline approvals, plan proposal
panel, AskUserQuestion panel, debug-mode banner). The pane header lives
in `AgentChatPaneHeader.tsx` and surfaces session controls + the
multi-provider model picker. The empty-state composer (before a session
exists) lives in `DraftChatSurface.tsx` and uses that same unified picker;
the "no panes" home landing lives in `ChatHomeLanding.tsx`.

### Thread Scope (first-send scope controls)

`DraftChatSurface` (the lazy-creation draft) renders its scope controls
**below** the composer as `ThreadScopeRow`
(`src/components/chat/pickers/ThreadScopeRow.tsx`) — a **scope strip**:
a narrower bar attached flush under the composer card (inset each side
by the card's 20px corner radius, bottom-only rounding, one tonal step
less elevated — the shared `SCOPE_STRIP` / `SCOPE_STRIP_INSET` class
constants), threaded through `Composer`'s `belowComposerSlot` prop (a
sibling of `zone1Override`, which both surfaces pass `null` while a
project root resolves — the cwd label moved into the row's location
control). `AgentChatPane` fills the same slot with the read-only
Context Row — see below for why.

**One surface, not two: `ThreadScopeRow` is `DraftChatSurface`-only.**
Every control in this row answers "what should the first send
CREATE?" — a question only a draft can be asked, because a draft has
nothing behind it yet. `AgentChatPane` is bound to a real workspace,
which owns exactly ONE project root and ONE checkout (`cwd` /
`worktree_path`) shared by every tab, surface and pane inside it, so
none of those answers are the pane's to give. The pane renders the
read-only Context Row from its FIRST render instead (see "Context Row"
below) — same strip shape at every message count, so scope never
appears to change across the first send.

Through `v0.15.0` the pane rendered the full row, and both of its
controls were app-wide relocations wearing per-thread clothing:

- **"New worktree"** did not rescope the thread. It created a SECOND
  workspace off the parent repo mid-send and moved the user into it —
  and since the row renders on every `messages.length === 0` thread, it
  re-asked on each additional chat tab, including tabs of a workspace
  that was *already* worktree-backed, offering a choice that workspace
  could not honour.
- **The location picker** activated a different project's first
  workspace (or opened the new-workspace dialog), abandoning whatever
  the user had typed in the pane they were looking at.

Creating a worktree now belongs to the surfaces that are honest about
creating a workspace (this draft surface, the sidebar worktree flow);
switching project or workspace belongs to the sidebar. Deleted with the
change: `AgentChatPane`'s `handleDeferredWorktreeSubmit`,
`src/lib/agent-chat/prestart-worktree-session.ts`, the row's
`ThreadScopeLocation` / `ThreadScope` discriminated props (flat props
again, one consumer), its `trailing` slot, and the workspace-mode
branches of `LocationControl`.

States, matched to the draft's target / `checkoutMode`:

- **Project · current checkout** — a `location · checkout`
  group on the left (ghost text buttons, each opening a popover) and a
  `from ⑂ <branch>` control on the right. (The centered muted scope-hint
  line that originally sat below the row was removed in `a003467`; the
  only surviving hints are inside the checkout popover.)
- **Project · worktree** — the checkout popover's "New
  worktree" option reveals an optional mono name input ("name — leave
  empty to auto-name") with a hint that empty names auto-derive from the
  first message, like the CLI.
- **Home** — only the location control renders (no checkout/branch —
  there's no project to scope); its hint line was removed with the others.
  `PresetBar` returns
  `null` outright for a home draft (previously it rendered fully
  disabled) via an early-return on the pre-existing `isHomeDraft` check.

**Draft store fields.** `ChatDraft` gained three optional fields
(`chat-draft-store.ts`), defaulted in `makeDraft` and read with `??`
fallbacks at every call site so pre-redesign persisted drafts still
deserialize: `checkoutMode: "current" | "worktree"` (default
`"current"`), `worktreeName: string` (default `""`), `baseBranch:
string` (default `""` — also doubles as the row's best-effort display
of "the project's current checked-out branch"; seeded from the
main/master/first-branch heuristic once the branch list loads, since a
draft has no workspace snapshot to ask).

**Location binding.** The row takes `target` + `onChangeTarget` and
does exactly one thing with them: retarget the client-side draft (Home
/ any known project / "Open another project…"). Nothing is created
before first send, and nothing navigates — the popover has no way to
activate a workspace, which is what made the retired workspace-mode
binding (`onSelectProject` / `onSelectHomeWorkspace`) a relocation
disguised as a scope control.

**Location popover is keyboard-first.** The "Run in" popover is a cmdk
`Command` with `shouldFilter={false}` plus a `CommandInput`
("Search projects…"), so opening it focuses the search field and the
whole flow is type → `Enter` with no mouse. Every cmdk popover in the
*chat* surfaces routes Radix's open-autofocus through `focusCmdkOnOpen`
(`src/components/chat/pickers/focus-cmdk-root.ts`) — `ThreadScopeRow`'s
three popovers, `ModelPicker`, `MultiProviderModelPicker`,
`PermissionModePicker`, `ReasoningPicker`, `SpeedPicker`, plus the
launch-time `launch-model-picker.tsx` / `launch-reasoning-picker.tsx`.
(Other cmdk-in-Popover surfaces — `branch-picker.tsx`,
`project-picker.tsx`, `sidebar-ports-popover.tsx`, `agent-launcher.tsx`
— do not use it; they work incidentally because their `CommandInput` is
the first tabbable element.) It targets the
`[cmdk-input]` when the popover has one — the search field must own
focus so typing filters immediately, and being a descendant of the
root it still forwards arrow/Enter to cmdk — and falls back to the
`[cmdk-root]` div on input-less pickers, which is what keeps arrow-key
navigation alive there. The input MUST win when present: cmdk's root
keydown only handles navigation keys, so focusing the root on an
input-bearing popover would leave printable keystrokes with nowhere to
go until the user clicked the search box.
Filtering + ranking is `fuzzyFilter` from `src/lib/fuzzy.ts` (shared
scored subsequence matcher, also used by the GitHub issue/PR pickers):
prefix beats substring beats scattered subsequence, word-boundary and
camelCase hits score like initials (`hap` → `hermes-agent-personal`),
shorter candidates win ties. The haystack is the display name; a query
containing `/` switches it to the full project path, which is how two
checkouts of the same repo are disambiguated. Matching name AND path
at once was tried and reverted — a short query is a subsequence of
nearly every long path, so the list stops narrowing (a query grazing
the shared `/home/…/projects/` prefix would otherwise match every row).
"Home directory
(~)" is filtered as a row like any other (it matches `home` and `~`),
while "Open another project…" sits outside `CommandList` and is never
filtered — it has to stay reachable exactly when nothing matched. The
query resets on close so the next open starts from the full list.

**Location list — Active / Settled sections.** The "Run in" popover
lists every project that has a live workspace, i.e. the same set the
sidebar draws from. Because parking a workspace (settling OR snoozing —
see `docs/features/sidebar.md`) hides its sidebar card without closing
anything, that set only ever grows: a long-lived install accumulates a
dozen-plus projects that are all still valid targets. A flat, unsorted,
uncapped list buried the project you touched a minute ago below one you
settled months ago. So the popover mirrors the sidebar's own vocabulary
(`project-scope-list.ts`):

- **Active** — at least one workspace is unparked (still has a sidebar
  card). Ordered most-recently-active first, using the inbox store's
  `activity` stamps. Unstamped projects rank last and keep their
  app-state order, so the ordering is strictly better than insertion
  order rather than dependent on a signal we can't guarantee.
- **Settled · still open** — every workspace is parked. BOTH parked
  lifecycles fold into this side of the partition — a project whose
  workspaces are all *snoozed* must not read as Active any more than a
  fully-settled one — and any future parked state must fold in the same
  way (`partitionProjectScopes` documents the invariant). Deprioritized,
  never hidden: starting a thread here is how you resurrect parked work.
  Ordered most-recently-parked first and collapsed to
  `SETTLED_COLLAPSED_COUNT` behind a "Show N more" row. The
  currently-targeted project is always shown even from the hidden tail —
  a checkmark the user can't see reads as a lost selection.
- Section headings appear **only** when something is parked; otherwise
  the popover stays the flat list it always was.
- A search spans both sections: each is filtered + ranked by
  `fuzzyFilter` separately (Active always above Settled), a non-empty
  query reveals the full settled tail, and the "Show N more" row never
  appears in search results. `CommandList` is capped + scrollable,
  bounded by `--radix-popover-content-available-height` so the taller
  sectioned popover can't clip off the top of a short window.
- Avatar loading is scoped to the rows actually on screen (it used to
  fetch 3 UI-state keys for EVERY known project on every open — 50+ IPC
  round trips on a long-lived install); expanding or searching pays only
  for what it newly reveals, and the per-open fetch generation resets so
  appearance edits made elsewhere in the session are picked up.

All signals come from `sidebar-inbox-store`, so the picker's order can't
disagree with the sidebar. It reads that store but never writes it:
selecting a parked project deliberately does **not** un-settle or wake
it, because the inbox already resurfaces a parked workspace the moment
its agent goes `working` — which is exactly what first send does.
Un-parking on mere selection would also fire while the user is only
browsing locations.

**Pane-surface state.** `AgentChatPane` holds no scope state at all —
no `checkoutMode`, no `worktreeName`, no `baseBranch`, no scope
callbacks. Its `belowComposerSlot` is the Context Row whenever a
project root resolves, at every message count, reading
`workspaceProjectRoot` + the snapshot's `git_branch` directly (the
branch chip hides while that's `null`, e.g. a home-rooted pane).

**Deferred worktree creation.** The old immediate-create "+ New
worktree…" row is gone — picking "New worktree" only sets
`checkoutMode`; the worktree is created as part of the SAME submit that
sends the first message. The shared naming/creation helper is
`createDeferredWorktree` (exported from
`src/lib/agent-chat/materialize.ts`): name precedence is trimmed
`worktreeName` verbatim → `generateBranchName(firstMessage,
projectPath)` (the CLI's AI first-message naming, backed by
`branch_name.rs::generate_ai_name`; the underlying `claude --print` call
is bounded by a 30s timeout, and any failure — spawn error, timeout,
non-zero exit, empty output — is logged via `log::warn!` to the native
log instead of failing silently) → `generateRandomBranchName(projectPath)`
on error/empty, then
`createWorktreeWorkspace` off the row's `baseBranch` with the `"empty"`
layout. Only ONE surface routes it (the pane arm was removed — see
"Location binding" above):

- **Draft surface** — `materializeAndSend` takes an optional
  `worktreeProjectPath` (resolved by `DraftChatSurface`:
  `target.projectPath` for a `project` target, the resolved
  `existingWorkspaceProjectRoot` for `existing_workspace`); it creates
  the worktree, resolves the pane/session cwd to the new worktree's real
  cwd (preferring the `cwd` the create response now carries — see the
  cwd-resolution invariant below), and continues down the existing
  create-pane → mark-materialized → start-session → send-turn ordering
  (shared `thread_id` throughout, so the first send can't hit
  `session_not_found`). **Instant feedback:** the optimistic user bubble
  (client-nonce'd) is appended into the pre-minted thread slice at the
  TOP of `materializeAndSend`, before any workspace/session work, and
  `DraftChatSurface` immediately swaps its landing for a
  `DraftPendingConversation` — the sent bubble (with `data:`-URL
  thumbnails) plus a shimmer status line driven by the new `onPhase`
  hook ("Creating worktree…" / "Setting up workspace…" / "Starting
  session…" / "Sending…") — and clears the composer. Every failure
  branch rolls the bubble back via `removeUserMessageByNonce` and
  restores the composer text. Previously nothing visible happened until
  the whole chain (incl. the 5-9s AI branch naming and up-to-8s MCP
  prime) resolved. Both first-send surfaces also fire a fire-and-forget
  `agent_chat_prime_mcp` on mount so MCP warmup overlaps typing (the
  registry's `prime_lock` serializes it against the start-session
  backstop prime so children can't double-spawn).
- **Pane empty state** — no longer participates. Every submit on a
  workspace-bound pane goes through the unmodified `handleSubmit` and
  stays in that pane's own workspace. (Its empty-thread first send is
  wrapped only to TITLE that workspace — see "Current-checkout
  auto-naming" below — never to create one.)

**CWD-resolution invariant.** A freshly-created worktree
workspace only reaches the Zustand app-store via the async
`app-state-changed` Tauri event, which has essentially never been
processed by the time `create_worktree_workspace`'s `invoke` promise
resolves. The previous synchronous read-back (`useAppStore.getState()`
right after the invoke) therefore missed ~always and silently left the
pane + agent session launching at the PARENT checkout cwd (the project
root) — so agents committed into the user's real working copy. Both
surfaces now share `waitForWorkspaceCwd(workspaceId, timeoutMs = 5000)`
(`src/lib/agent-chat/wait-for-workspace-cwd.ts`): it awaits the
workspace landing in the store with a non-empty `cwd`, racing the
store subscription (`app-state-changed`) against a polled direct
`get_app_state` fetch (written back into the store) so a
dropped/reordered event can't strand it. Since the first-send
performance pass, `create_empty_workspace` / `create_worktree_workspace`
return `WorkspaceCreated { workspace_id, cwd }` (frontend:
`createEmptyWorkspaceResult` / `createWorktreeWorkspaceResult` +
tolerant `normalizeWorkspaceCreate`; the legacy string-returning
wrappers still exist for the ~20 other callers), so both surfaces read
the cwd straight off the create response and `waitForWorkspaceCwd` is
only the fallback when it's somehow absent. **Hard invariant:** a
`checkoutMode === "worktree"` submit must NEVER create the pane or start
the session at the parent/project-root cwd — on timeout,
`materializeAndSend` fails the send via `markSendFailed` (draft text
preserved) rather than proceeding.

**Current-checkout auto-naming.** `checkoutMode === "current"` (the
default) still creates no worktree and no branch, but it is no longer
naming-inert. A worktree first-send gets a human-readable workspace name
for free — `createDeferredWorktree` resolves an AI branch name and the
backend's `set_workspace_worktree` (`state_impl.rs`) overwrites the
workspace title with that branch — whereas the current-checkout path
creates no branch and so used to keep the backend default
(`format!("Workspace {workspace_index}")`, i.e. `Workspace 58`) forever.
That was the last asymmetry: Home already derived a title, worktrees
inherited one, and only "run on the current checkout" stayed numbered.
Both surfaces now name it from the first message via the shared
`autoNameWorkspace` (`materialize.ts`):

- **Draft surface** — a `project` target routes through
  `createProjectWorkspace`, which is `createEmptyWorkspace` + a fired-off
  `autoNameWorkspace`. Same treatment in `materializeWithPreset`.
- **Pane empty state** — the pane has no checkout mode to choose; it
  always sends into the checkout its workspace is already on, so its
  first send is a current-checkout send by construction.
  `handleCurrentCheckoutFirstSubmit` fires `autoNameWorkspace` and falls
  straight through to the unmodified `handleSubmit`, so the send path is
  byte-for-byte what it was. The interception is gated on
  `messages.length === 0`, a non-home workspace, and a resolvable project
  root; everything else stays on `handleSubmit`. It re-checks
  `handleSubmit`'s own bail-outs (`sendInFlightRef`, `threadId`, empty
  text) BEFORE naming, in the same order: `messages.length` only flips
  once the optimistic bubble lands, so two Enter presses in one tick both
  reach this handler and only the in-flight ref rules the second one out —
  without it they would race two `claude --print` calls into two renames,
  the second landing before the first's title reached the store, where the
  apply-time guard below could no longer see it.

**Same namer as worktrees, on purpose.** Naming routes through
`generateBranchName` — the exact call `createDeferredWorktree` and the
CLI use — rather than the cheap `deriveTitleFromFirstMessage`. These
titles sit side by side in the sidebar; deriving from message text would
put `take a look at this image, when i make a…` directly above
`sidebar-workspace-ordering` and the list would read as two different
products. One namer means every workspace title has the same shape.
`deriveTitleFromFirstMessage` survives as the fallback when the namer's
IPC itself rejects (truncated message text still beats `Workspace 58`)
and as the Home path's naming, which stays message-derived — a
home-rooted workspace is not a project checkout, so there is no repo for
`generateBranchName` to name a branch against or deconflict within.

**Fire-and-forget, guarded at apply time.** `generateBranchName` shells
out to `claude --print` (5-9s warm, 30s timeout), so awaiting it would
put that on the critical path of a send that otherwise makes no extra
backend calls. The send proceeds immediately and the sidebar card
re-titles itself seconds later — the "conversation names itself shortly
after you send" behavior chat UIs use. Renaming twice (instant
truncation, then upgrading to the slug) was rejected: two visible title
changes per workspace is churn and the intermediate value is the ugly
one. Because the rename lands seconds late, the
`isDefaultWorkspaceTitle` guard (`derive-title.ts`) is re-read AFTER the
namer resolves, not at call time — on the pane path the workspace may
have existed for days, and a user can rename it mid-flight; either way a
user-chosen or branch-derived name is never clobbered. The guard takes
the workspace's directory alongside its title, because a
backend-assigned title can BE the directory name: it accepts the
directory-name default (`~/projects/codemux` → `codemux`), the legacy
`Workspace {n}` shape, and an absent title (nothing there to protect).
See `docs/features/workspace-creation.md` § Workspace Titles for where
each default comes from. A workspace absent from the store is one just
created whose `app-state-changed` hasn't landed, so absence is not
evidence of a user-chosen name and naming proceeds. Naming never rejects
into the send path (it is cosmetic), and an empty/whitespace-only message
skips it entirely rather than blanking the title.

**Backend adopt-don't-duplicate guard.** `git_create_worktree` can
return an EXISTING worktree path without creating anything (the
path-reuse short-circuit in `git.rs`, when the dir is already registered
to the same branch). `create_worktree_workspace_impl` therefore looks
for a live LOCAL workspace already claiming the resolved path
(`find_live_workspace_for_worktree_path` — `worktree_path` claim on path
identity alone; `cwd` fallback additionally requires the workspace's
`git_branch` to equal the requested branch, since a bare path match
would adopt across branches; canonicalized comparison,
remote/attach-only rows skipped) and adopts it: activate + return that
id, instead of leaving two workspaces — and two agents — on a single
checkout. Lookup and insert are ONE atomic operation under the state
lock (`AppStateStore::adopt_or_create_worktree_workspace`, which also
stamps the new workspace's `worktree_path` claim before releasing), so
two concurrent creates for the same path can't both miss the probe and
both insert; the slow `git worktree add` stays outside the lock. The
adopt path deliberately skips everything the create path would re-run on
a live workspace (PTY spawn, setup scripts, `.mcp.json` rewrite, preset
launch), and DROPS `initial_prompt` / `agent_preset_id` rather than
injecting them into a session someone else is already using — the
command returns `adopted: true` (`WorkspaceCreated { workspace_id, cwd,
adopted }`; also in the control-socket JSON), and the new-workspace
dialog toasts "…switched to it.", appending "Your prompt wasn't sent."
only when a prompt was actually entered, so the drop is never silent. Archived workspaces live in `archived_workspaces`, so they
neither block creation nor get silently un-archived.

**Branch control ↔ checkout mode coupling** (editable scope only — the
pane surface has no checkout mode to couple to)**.** The branch popover
(`ThreadScopeRow`'s `BranchControl` — search, All/Worktrees tabs, kind
icons, mono ages, amber WORKTREE badge on rows with a worktree on this
device) shares one `baseBranch` value for both "the checked-out branch"
(current mode) and "the worktree's base" (worktree mode). Picking a
DIFFERENT branch while on `"current"` auto-flips `checkoutMode` to
`"worktree"` with the picked branch as base — the control never
silently repoints the user's real checkout. Picking the SAME branch, or
any branch while already on `"worktree"`, just updates `baseBranch`.

`WorktreePicker` and `DerivativeBranchPicker` were **deleted** —
`ThreadScopeRow` absorbed both first-send surfaces and the "switch to a
sibling worktree" affordance moved to the sidebar (siblings stay
reachable there); `ProjectPicker`
(`src/components/overlays/project-picker.tsx`) survives for the
new-workspace dialog only.

**GUI chrome suppression.** When the GUI flag is on and the chat pane is
the **sole root** of its surface (not a split), `AgentChatPaneHeader` does
NOT render — the title bar absorbs the tab, its session-history dropdown,
close, and "Restore checkpoint" (`PaneNode` gates on `isSurfaceRoot`);
subagent status now lives in the docked activity bar (see "Docked live
activity bar" above), not either header, so there is no "N subagents
running" pill left to absorb. In split layouts the per-pane header still
renders, so split/close/drag keep working. The session-switch orchestration
(stop → hydrate → resume), the checkpoint-restore state, and the grouped
session list were extracted into shared pieces so the legacy per-pane header
and the titlebar tab drive one implementation, not a fork:
`src/hooks/use-agent-chat-session-actions.ts`,
`src/hooks/use-agent-chat-checkpoint-restore.ts` +
`src/components/chat/restore-checkpoint-dialog.tsx`, and
`src/components/chat/session-history-menu.tsx` (`SessionHistoryList` +
`useSessionHistory`, now consumed by both `SessionSelector` and the
title-bar chat tab). See `docs/features/gui-chrome.md`.

### Context Row (running-thread status)

Design: `.design-import/Context Row.dc.html` (claude.ai/design handoff).
`AgentChatPane` never renders `ThreadScopeRow`; its `belowComposerSlot`
is the read-only **Context Row** from the pane's FIRST render, at every
message count (`AgentChatPane.tsx:2626`) — the same tucked
scope-strip shell (`SCOPE_STRIP` / `SCOPE_STRIP_INSET` exported from
`ThreadScopeRow.tsx`), but scope is committed (the workspace
binding locked it in) so the left side is not clickable:

- **Left** — static ghost-styled spans: folder icon (home icon +
  `text-status-remote` for a home-rooted pane) + project name (`title`
  tooltip = the resolved project root), a `·` separator, then a
  `font-mono` branch-icon span with the workspace's checked-out branch
  (`title="Branch: <name>"`) — omitted entirely when the workspace has
  no `git_branch`.
- **Right** — `<WorkspaceStatusCluster />` (`src/components/chat/WorkspaceStatusCluster.tsx`).
  (It used to also fill `ThreadScopeRow`'s `trailing` slot in the
  empty-thread state; that slot was deleted with the scope-row removal,
  so the cluster now renders only inside this Context Row.)

The row renders whenever `workspaceProjectRoot` resolves — no
message-count gate (see "Pane-surface state" above); otherwise
`belowComposerSlot` falls through to `undefined` (unresolved
project root, e.g. very early boot).

**`WorkspaceStatusCluster`** is self-contained — it reads
`useActiveWorkspace()` directly rather than taking props (valid here
because `PaneContainer` only ever mounts panes for the active
workspace, so any `AgentChatPane` instance's workspace IS the active
one). It renders:

- the **background-browser indicator** (first in the cluster, mirroring
  the browser control in terminal headers) when the workspace has a live, pane-less
  `agent_browser_sessions` entry — the shared
  `BackgroundBrowserIndicator` + `useBackgroundBrowserSession` pair
  (`src/components/browser/background-browser-indicator.tsx`, shared with
  the active terminal's compact header control). Click opens the
  peek overlay (`useBrowserPeekStore().open`). The cluster's
  `enableAgentChat` gate is implicit in the cluster's mount context (a
  disabled flag renders a placeholder instead of the pane);
- a **behind chip** (`↓N`, `text-warning`) when `git_behind > 0`;
- a **PR chip** (`#N`, tone-tinted via `PR_CHIP_TONE` in
  `src/components/github/pr-status-icon.tsx`) that opens `pr_url` on click;
- a **linked-issue chip** (`Issue #N`) — the shared
  `IssueDetailPopover` (chip variant, `side="top" align="end"`,
  `src/components/github/issue-detail-popover.tsx`) the old bar used,
  reused verbatim so a thread's linked issue stays visible after PR #144
  relocated the bar's content here. Unlike the git chips it renders
  independent of the branch, so a branch-less workspace with only a
  linked issue shows the Issue chip alone;
- a **workspace-details button** (window icon + chevron-up) that opens
  a `~290px` popover (`side="top" align="end"`) with:
  - a header — workspace title, then `project · device` in
    `font-mono` subtext;
  - detail rows: Branch, Base (only once known — fetched on open via
    `getGithubPrByPath(cwd, pr_number)` for `PullRequestInfo.base_branch`,
    since `WorkspaceSnapshot` doesn't carry it; omitted on fetch
    failure), Behind base (warning tone, if any), Ahead (success tone,
    if any), Uncommitted (`+A −D`, if any), Pull request (`#N ·
    <state>`, tone-tinted via `prStatusTextClass`), Issue (`#N ·
    <state>`, only with a `linked_issue` — `text-success` when Open else
    `text-muted-foreground`, the same tone the sidebar/issue components
    use), Location (`this device` or the resolved host name);
  - a footer with up to two quick actions: **View PR #N** (only with a
    `pr_url`) and **Sync ↓N** (only while `git_behind > 0` — calls
    `gitPullChanges(cwd)` with a busy label + an error toast on
    failure, mirroring `changes-panel.tsx`'s `handlePull`).

Renders `null` with no active workspace, or with neither a `git_branch`,
a background browser session, nor a `linked_issue`. The git chips +
details popover additionally require the branch, so a git-less workspace
with a live background browser shows the Browser pill alone, and one
with only a linked issue shows the Issue chip alone — matching the old
bar's visibility set (`hasGit || prState || linked_issue || browser`).

**No global bottom bar.** The full-width `WorkspaceContextBar` was removed.
The Context Row owns chat-local detail inline; the sidebar and hover card own
cross-surface workspace detail; and an active terminal places only the live
background-browser launcher in its existing pane header. See
`docs/features/workspace-context-bar.md`.

## Context-window meter (composer)

A donut-ring icon in the composer footer's right cluster, immediately
before the Send/Stop button, showing the thread's live context-window
occupancy. Clicking it opens a popover (`side="top" align="end"`) with:
a "Context Window" header row (`{pct}% · {used}/{max}` in
`font-mono tabular-nums`), a horizontal progress bar, a
"Total processed" row (lifetime tokens, only when strictly greater than
used), and — when the provider auto-compacts — a
"`{Provider}` automatically compacts its context when needed." note.
Above 90% occupancy the ring and bar switch to the `danger` token.
Hidden entirely (no reserved space) until the thread's first usage
snapshot; the draft surface never shows it.

**Two numbers, one bar.** `used_tokens` is the *live* occupancy of the
model's window — bounded, clamped to `max_tokens`, drops after a
compaction; it alone drives the ring/bar/percent. `total_processed_tokens`
is the monotonic lifetime sum for the thread including everything
compaction discarded — unbounded, text-only, omitted by adapters unless
strictly greater than used. `max_tokens` is only ever provider-reported —
never guessed; without it the UI degrades to a bare token count (no
percent, no bar).

**Event + persistence.** Adapters emit
`ProviderRuntimeEvent::ContextUsageUpdated { thread_id, usage:
ContextUsageSnapshot }` (`agent_provider/events.rs`). The shared
`ContextUsageTracker` (`agent_provider/context_usage.rs`) enforces the
cross-provider rules in one place: clamp used≤max, sticky last-known
window, monotonic lifetime total with omit-unless-greater, and
no-op/zero suppression. The event is persisted (`should_persist_event`),
so hydrate replays the latest snapshot through the reducer and the
meter survives restart with no bespoke schema — same trick as
`SubagentUpdated`. Latest snapshot wins per thread
(`ChatThreadState.contextUsage`, `reducer.ts`
`case "context_usage_updated"`); the reducer additionally never lets
the lifetime total regress and the snapshot rides through
`migrateThreadId` on silent session restart.

**Per-provider sources.**

- **Claude** — (a) assistant `message.usage`
  (`input + cache_creation + cache_read + output`; subagent messages
  with `parent_tool_use_id` are excluded, preserving the existing
  usage-hygiene rule); (b) turn-end `result`, whose `modelUsage[*]
  .contextWindow` is the authoritative `max_tokens` (arrives only at
  first turn end — mid-first-turn snapshots carry `max_tokens: null`,
  which is why the registry seed below matters) and whose per-turn
  figure feeds the lifetime accumulator once per turn; (c)
  `system.compact_boundary` — `used = post_tokens`,
  `last_used_tokens = pre_tokens` (unclamped high-water mark), total
  carried forward. `compacts_automatically: true`.
- **Codex** — the pinned app-server's `thread/tokenUsage/updated`
  notification (`{ last, total, modelContextWindow }`): used =
  `last.totalTokens`, lifetime = provider-maintained `total`, max =
  `modelContextWindow`. Child-thread reports are dropped.
  `compacts_automatically: true`.
- **OpenCode** — the assistant message's `tokens` block
  (`input + cache.read + cache.write + output + reasoning`), lifetime
  via a per-message high-water delta so incremental re-broadcasts don't
  compound; `max_tokens` from the upstream catalogue's `limit.context`
  (probed in the background, re-probed on `set_model`). A model swap
  **invalidates** the window rather than waiting for the re-probe: the
  window belongs to the model, so `set_model` clears both the routing
  context's copy and the tracker's sticky one
  (`ContextUsageTracker::clear_max_tokens`, the explicit counterpart to
  `observe_max_tokens(None)`'s deliberate no-op) and re-publishes the
  current occupancy with no denominator. The meter degrades to a bare
  token count for the gap instead of dividing by the old model's window.
  The lifetime total and its per-message high-water map survive the swap
  — they de-duplicate incremental re-broadcasts, which is model-independent.

**First-paint seed.** The capability registry carries numeric windows
purely as a seed until the provider's own report lands:
`ContextWindowOption.context_window_tokens` (Claude `200k`/`1m` options
→ 200_000 / 1_000_000) and `ChatModelInfo.max_context_tokens`
(OpenCode, harvested; `None` for Claude — the window there is a
property of the *selected* option — and for Codex). Resolution lives in
`src/lib/agent-chat/context-usage.ts` (`resolveContextWindowTokens`,
`deriveContextUsageDisplay`, and the `formatContextTokens` banding:
lowercase `k`/`m`, one decimal only in the 1k–10k and ≥1M bands, `1m`
never `1.0m`, the band picked from the rounded figure so 999_600 reads
`1m` rather than a nonexistent `1000k`, null/NaN → `"0"`).

**Components.** `src/components/chat/ContextUsageMeter.tsx`
(`{ usage, seedMaxTokens?, providerLabel? }`, test hooks
`context-usage-trigger` / `context-usage-readout` /
`context-usage-total-processed`), wired `AgentChatPane` → `Composer` →
`ComposerFooter` via optional `contextUsage*` props. The dev mock seeds
one snapshot into the showcase thread so the meter is visible under
`npm run dev`.

## Tauri command surface

Session/lifecycle commands are gated on the `enable_agent_chat` feature
flag and return a `feature_disabled: ...` string when the flag is off;
session commands also return a `provider_not_configured: ...` string
when the target provider is missing from the registry. The read-only /
DB-only history commands (`agent_chat_list_sessions`,
`agent_chat_get_session`, `agent_chat_update_session_config`,
`agent_chat_get_checkpoint`, rename/delete) are intentionally NOT
flag-gated, so a pane re-seeds or falls back to defaults rather than
surfacing a `feature_disabled` string.

| Command | Purpose |
|---|---|
| `agent_chat_create_pane` | Insert a new chat pane in a workspace, returns the new pane id. |
| `agent_chat_close_pane` | Close a chat pane. Idempotent — double-close is a no-op. |
| `dev_agent_chat_spawn_test_pane` | Debug-only. Spawns a chat pane in the active workspace for manual QA from the browser devtools. |
| `agent_chat_start_session` | Start a provider session, writing the returned thread id back onto the pane. |
| `agent_chat_send_turn` | Queue a user turn on a thread. Auto-resumes a dead session (rebuilt from its persisted row) before the turn, so a pane reopened after an app restart never sees `session_not_found`. |
| `agent_chat_interrupt_turn` | Halt the currently-running turn. On a live session the turn ends as `TurnCompleted(interrupted)` and the session stays `Ready` (the sidecar keeps the thread alive and the next `send_turn` rebuilds a resumed query). A `SessionNotFound` (nothing to interrupt on an already-dead session) is swallowed into `Ok`; unlike `send_turn` it does NOT auto-resume a dead session just to interrupt it. |
| `agent_chat_turn_active` | Cheap in-memory liveness probe: `true` iff a live (non-dead) session is bound to the thread and its `active_turn` is set (Running or WaitingApproval). Never touches the subprocess and never auto-resumes. Used by the remount hydrate path to suppress the false "Run interrupted" divider on a healthy mid-flight run (see "Live-run guard on hydrate"). |
| `agent_chat_respond_to_request` | Resolve a pending approval / input request. Claude/Codex only respond on the original live session; if its process-local callback is gone, a durable `request_response_failed` event expires the control instead of starting a replacement session that cannot know the request. OpenCode may resume its server-backed pending permission first. |
| `agent_chat_set_model` | Swap a thread's model. Persist-always, apply-if-live: writes the model to the session row unconditionally, then applies to the live session if one exists; a `SessionNotFound` from the apply is swallowed into `Ok` (the value takes effect on the next auto-resume). |
| `agent_chat_set_permission_mode` | Swap a thread's permission mode. Same persist-always, apply-if-live contract as `agent_chat_set_model`. |
| `agent_chat_stop_session` | Gracefully close a session. Idempotent. |
| `agent_chat_get_session` | Return a single persisted session row by thread id, INCLUDING rows whose `sdk_session_id` is still null (unlike `agent_chat_list_sessions`), with the per-thread config columns. Not flag-gated. Used by the pane's mount-seed to restore pickers + resume cursor after a restart. |
| `agent_chat_update_session_config` | Persist a per-thread config patch (`model` / `effort` / `context_window` / `permission_mode`) to the session row. DB-only, no live-session requirement; only the provided fields overwrite (`COALESCE`). Not flag-gated. Fired fire-and-forget by the picker handlers. |
| `agent_chat_get_checkpoint` | Return the run-start rollback checkpoint recorded for a thread (or null). Not flag-gated — renders as "no checkpoint" instead of an error. |
| `agent_chat_restore_checkpoint` | Roll the workspace back to the thread's run-start checkpoint. Mutates the working tree; the UI confirms first. |

Provider errors are serialized as `SerializableProviderError` JSON so
the UI can inspect the error subtype (e.g.
`{"kind":"not_authenticated", ...}`) instead of parsing a free-form
string.

## Run checkpoints (issue #80)

Opt-in rollback point taken when a chat session starts. Off by
default; toggled via Settings → Agent → "Checkpoint before agent
runs" (`UserSettings.agent_chat.checkpoints_enabled`, synced
settings).

- **Background, never on the first-token path.** `agent_chat_start_session`
  spawns `tauri::async_runtime::spawn` + `spawn_blocking` AFTER the
  provider session is up; even the settings-cache read happens inside
  the spawned task.
- **Non-destructive snapshot.** `git_checkpoint_create`
  (`src-tauri/src/git.rs`) stages the worktree (tracked + untracked,
  `.gitignore` respected) into a temporary `GIT_INDEX_FILE`, writes a
  tree, and `commit-tree`s it onto HEAD — the user's index, worktree,
  and stash list are untouched and no hooks run. The commit is
  anchored at `refs/codemux/checkpoints/<sanitized-thread-id>` so gc
  can't reap it. Skipped silently for non-repos and unborn-HEAD repos.
- **Recorded per thread.** `agent_chat_checkpoints` table (FK →
  `agent_chat_sessions`, ON DELETE CASCADE) stores snapshot commit,
  HEAD commit, branch, repo path, ref name. On success the backend
  emits `agent_chat_checkpoint` (`{thread_id, checkpoint}`); the
  frontend hook `useAgentChatCheckpoint` (event + on-mount fetch)
  feeds the pane header's restore button (history icon, hover-reveal,
  hidden when no checkpoint, disabled mid-turn).
- **Restore** (`git_checkpoint_restore`): refuses on branch mismatch
  or pruned commits; takes a safety snapshot of the current state to
  `refs/codemux/pre-restore/<id>` first; then `read-tree --reset -u
  <snapshot>` + `clean -fd` + `reset --mixed <pre-run HEAD>`. Result:
  run commits undone, run-created files deleted (ignored files
  spared), pre-run changes back as unstaged edits / untracked files.
  Known loss: the pre-run staged/unstaged split flattens to unstaged.
- **Pruning.** Each create prunes both `refs/codemux/` namespaces to
  the 20 newest refs and drops the matching bookkeeping rows.

Design note: `docs/archive/agent-run-checkpoint.md`.

## Event bridge

Every registered provider's canonical event stream is forwarded to
the frontend per thread over a `tauri::ipc::Channel` (issue #75).
Payloads (`AgentChatEventPayload`) carry the originating `thread_id`
alongside the raw `ProviderRuntimeEvent`.

Transport split:

- **Thread-scoped events** — including the high-frequency streaming
  `content_delta` tokens — are routed to the `Channel` the owning
  pane registered via `attach_agent_chat_output(thread_id, channel)`
  (`AgentChatChannelRegistry` in `commands/agent_chat.rs`, mirroring
  the PTY `attach_pty_output` pattern). A pane only ever receives its
  own thread's events; nothing is broadcast app-wide. Tauri's event
  system is explicitly not designed for high-throughput streaming;
  Channels are the documented mechanism for it.
- **Thread-less events** (global `RuntimeWarning`s) keep the
  low-frequency `agent_chat_event` global event bus, with an empty
  `ThreadId`.

Replay split: the channel carries **live events only**. Transcript
events are persisted to SQLite by `forward_event` whether or not a
channel is attached, and a late-attaching / resumed pane rebuilds
history through the DB hydrate (`agent_chat_list_messages` →
`hydrateThread`). Non-persisted lifecycle notices that fire while no
pane is attached are dropped — the same outcome the event bus had
for unmounted panes.

Attach/detach lifecycle: `attach_agent_chat_output` returns a
generation token; `detach_agent_chat_output(thread_id, generation)`
only removes a matching generation, so a stale unmount can never tear
down the channel a newer pane just installed. The frontend hook
`useAgentChatEvents(threadId, handler)` in
`src/hooks/use-agent-chat-events.ts` owns this lifecycle: it attaches
a `Channel` per mounted thread and serializes its detach behind the
attach promise.

The bridge is a thin loop: one background Tokio task per provider,
each consuming the provider's `event_stream()` and routing each
event through `forward_event`. `broadcast::error::RecvError::Lagged`
is already swallowed by each provider's event-stream helper, so slow
subscribers never crash the loop — they just drop old events.

## Sidebar status indicators

Chat sessions publish into the same `pane_statuses` snapshot the left
sidebar reads, so a chat workspace shows the working spinner / red
needs-input pulse / green ready-for-review dot exactly like a terminal
agent (previously chat panes showed nothing). `forward_event` in
`commands/agent_chat.rs` maps each `ProviderRuntimeEvent` to a
`PaneStatus` (`map_event_to_pane_status`) and writes it through
`AppStateStore::set_pane_status_by_thread`, which resolves the
`thread_id` to its `AgentChat` pane (`find_agent_chat_pane_id`) — the
chat-side analogue of the terminal hooks path in `hooks.rs`. Mapping:
streaming deltas / committed items / a running session / a resolved
request → `Working`; an opened approval / plan / AskUserQuestion (or a
session parked on approval) → `Permission`; `TurnCompleted` → `Review`
(downgraded to `Idle` when the pane's workspace is already active,
mirroring `handle_lifecycle_event`); `RequestResponseFailed` → `Review`
(a request expired against a restarted provider session, so the thread
wants the user but no turn is running); session `Closed`/`Error` →
cleared.
A change-guard on `set_pane_status_by_thread` means the `content_delta`
token stream collapses to a single write per turn rather than one per
token. Because the write lands in `pane_statuses`, the sidebar rows,
collapsed-rail aggregate dot, hover flyout, and `PaneNode` borders all
update for free.

**Subagents keep the spinner alive past `TurnCompleted`.** The primary
chat turn can finish while subagents it spawned are still running, so the
mapping is stateful: `map_event_to_pane_status` takes the event *plus* a
per-thread `ThreadSubagentState` (running `subagent_id`s + a
`review_pending` flag), held in the Tauri-managed `SubagentTracker`
(`Mutex<HashMap<thread_id, _>>`, one lock per decision, registered in
`lib.rs` alongside `AgentChatChannelRegistry`). `SubagentUpdated` with a
`running`/`pending` status records the subagent; a terminal status
(`completed`/`failed`/`stopped`) drops it. `TurnCompleted` publishes
`Review` only when no subagents are tracked — otherwise it *holds
`Working`* and marks `review_pending`, and the deferred `Review` fires
when the last subagent goes terminal. The working indicator therefore
persists until the turn **and** all tracked subagents finish, matching the
still-running `SubagentsCard` in the drill-in.

**A new user turn resets the thread's tracker** (both `running` and
`review_pending`). The authoritative, provider-agnostic anchor is the
send-message command path: `agent_chat_send_turn` calls
`SubagentTracker::clear_thread` before dispatching the turn. This matters
because a provider can leave a subagent non-terminal — Claude's background
`async_launched` task stays `Running` until a later `task_notification`
that may never arrive — and without the reset that stale `running` id
would hold `Working` and suppress `Review` for *every* later turn until
session close. `SessionStateChanged::Running` also resets the tracker
(reliably per-turn for Codex's `turn/started` and OpenCode's
`session.status = busy`; Claude does not fire it per user turn, hence the
command-path clear). A genuinely-still-alive background subagent
re-inserts itself on its next `Running` snapshot — Claude re-emits these
via `task_progress` ticks — so clearing at turn start is safe. Crucially,
parent-scoped `content_delta`/`item_completed` (`subagent_id == None`) do
**not** clear `review_pending`: such output can trickle in after a deferred
`TurnCompleted`, and dropping the owed `Review` there would strand the
pane at `Working`. `review_pending` is cleared only by publishing the
owed/normal `Review`, a new-turn reset, session `Closed`/`Error`, or
`agent_chat_close_pane`.

**Background tasks are excluded from all of that.** Claude emits the same
`system.task_started` / `task_progress` / `task_updated` /
`task_notification` family for a background *tool* run — a
`Bash { run_in_background: true }` — keyed by the Bash `tool_use_id`.
Such a job can legitimately outlive the turn and never report a terminal
status (a dev server never exits, so its `task_notification` never
arrives), so tracking it as a subagent deferred the owed `Review`
**forever**: the sidebar stayed "Working" and, downstream,
`release_detached_agent_browser` never fired, leaving the background
browser chip on `LIVE` indefinitely. `SubagentSnapshot` therefore carries
an additive `background_task: bool` (`#[serde(default)]`, so old persisted
payloads and the other providers decode as `false`). The Claude adapter
stamps it from the exact discriminator it already has:
`SubagentDemux::is_top_level_launch` is true only for a `tool_use_id`
registered by a real `Agent`/`Task` launch, so `!is_top_level_launch(id)`
on a task event means "background job, not subagent". A flagged
`running`/`pending` snapshot is therefore **not tracked at all** and
returns `None` — even after the turn settled, so a progress tick cannot
resurrect `Working` — and it defensively drops the id in case an
unflagged variant inserted it earlier. Terminal flagged snapshots still
drop the id and leave the settled-status computation unchanged. Real
async `Task` launches are untouched: their deferred-`Review` flow above
is deliberate.

The one exception is a background job the SDK *also* labels a watch loop
(`task_kind`, below): that one is tracked, because the badge is the whole
point of it — but as a monitor, which never blocks the settle either. The
two classifications are stamped together by one
`stamp_task_classification` call per `translate_task_*` fn and folded
into a single `TaskClass` by the tracker; see
`docs/features/monitoring-status.md` § "Two classifications, not two
candidates".

**The stall watchdog force-settles an owed `Review` that goes silent.**
The rule above removes the known cause, but any *other* missed terminal
signal (a crashed notification, a provider quirk) would pin `Working` the
same way, and `RunStalled` is advisory only — it maps to pane-status
`None`. So the 30s sweep (`spawn_stall_watchdog`) has a second pass,
`force_settle_overdue_reviews`. `ThreadSubagentState` tracks
`review_owed_since`, stamped while a `Review` is owed and re-armed by
*any* tick from a real agent-class tracked entry (`refreshes_owed_review`
— a background-task snapshot and a watch-loop snapshot deliberately do
**not** count, so neither a never-ending shell job nor a chatty CI poll
can hold the clock open). Because any real tick re-arms it, the 600s
`STALL_THRESHOLD` means "every remaining blocker has been silent for ten
minutes", not "the thread has been quiet since the turn ended".

The sweep's scope is an invariant rather than a rule to remember:
`review_pending` is *derived* as `turn_settled && !forced_settled &&
has_agents()`, so a thread whose only live entries are watch loops (or
background rows, which are tracked nowhere) is invisible to it by
construction and can never be force-settled off its `Monitoring` badge.

**What that threshold cannot prove is that the run is dead.** A single
long tool call — a `cargo test`, a big build — emits nothing for longer
than ten minutes, so the sweep will sometimes fire on a subagent that is
perfectly alive. The forced path is therefore deliberately
*non-destructive*: it publishes a pane status and does nothing else.

- **It never releases the detached browser.** `apply_pane_status` takes a
  `SettleOrigin`; `publish_pane_status` passes `ProviderEvent` (a real
  transition is evidence the run reached that state, so the browser
  session may go) and the watchdog passes `ForcedBackstop`, which
  withholds the release. A prematurely settled dot is repainted by the
  next real event; a browser torn down under a live subagent is not
  recoverable. Everything else — the `Review`→`Idle` downgrade in the
  active workspace, the stamped `app-state-changed` emit — is shared.
- **It tombstones the tracker entry instead of dropping it.**
  `take_overdue_reviews` clears only what it publishes (the silence
  clock) and sets `ThreadSubagentState::forced_settled`, leaving the
  task map intact — which is also what makes the derived
  `review_pending` read false from then on. That keeps the settle
  take-once (a later sweep skips it), and a late-but-real terminal
  snapshot then drains the map through the ordinary path, publishes
  **nothing** (the `Review` was already announced, possibly already
  cleared by the user), and lets the now-clear entry be dropped.
  Removing the entry at settle time instead would leave that snapshot to
  recreate a husk that nothing ever collects. The tombstone is cleared by
  every genuine turn boundary — a new `SessionStatus::Running`, session
  teardown, or the send-message `clear_thread`.

The sweep still runs under the tracker lock, and the mutex is released
before any `AppStateStore` call. It can never cut a live *turn* short:
`review_pending` needs `turn_settled`, which only `TurnCompleted` sets.

Only *live* provider events reach the tracker: transcript hydration/resume
replays persisted events through the frontend reducer
(`agent_chat_list_messages` + `hydrateThread`), never through
`forward_event`, so there is no backend replay to guard against. Session
`Closed`/`Error` and explicit `agent_chat_close_pane` also clear the
thread's tracking so a stuck subagent can never pin the spinner after the
session is gone.

**A settled thread with a live watch loop reads `Monitoring`.** The tracker
classifies each live task as agent work or a watch loop (`monitor` /
`monitor_mcp` / `local_bash` / `shell`, from the SDK's optional
`task_started.task_type`). A watch loop **never defers the settle** — the turn
completes, the `Review` publishes, `run_finished` fires and the detached
browser is released exactly as it would with no monitor at all. All it changes
is *which* settled status is shown: `settled_status()` reads
`Working` → `Monitoring` → `Review` straight off the live sets, so a monitor
whose first snapshot lands after the turn settled still lights the badge, and
the pane falls back to `Review`/`Idle` the moment the last one ends. A watch
loop keeps its transcript card but is excluded from the `SubagentActivityBar`
roster. A docked `MonitoringBar` between transcript and composer carries the
Stop button (`agent_chat_stop_monitoring`, durable via a per-run
stop-blocklist), and any provider's agent can reach the same status through
`codemux monitor start`. Full rules, the combination choke point, and the
bare-session interrupt limitation: `docs/features/monitoring-status.md`.

## Follow-up queueing

Sending a chat message while the agent is mid-turn **queues** it instead
of rejecting it. Previously the frontend optimistically appended the user
bubble and then `agent_chat_send_turn` failed with
`ProviderError::ValidationError("session has an active turn …")`, leaving
an orphaned bubble + a toast + a message that never got answered. Now the
send is queued and auto-dispatched as the next turn.

**Backend-authoritative queue.** The queue lives in Rust session state
(`queued_turns: VecDeque<QueuedTurn>` on both `claude/session.rs` and
`codex/session.rs`), so it survives pane unmount/remount. The busy branch
of `send_turn` enqueues rather than erroring (`enqueue_or_send`); the
busy check re-runs under the state lock, so there is no TOCTOU with a
turn that finished between the frontend send and the backend enqueue.
`TurnStartResult` gained an optional `queued_id` — set when the send was
queued (its `turn_id` is then an empty placeholder). OpenCode has no busy
guard and no queue; `cancel_queued_turn` is a trait-default no-op there.
Pressing Send finalizes attachments and resolves exact skills before this
enqueue decision; the accepted queue item is an immutable input snapshot, so
later file edits or Settings changes do not silently rewrite a submitted turn.

**Draining.** On the turn-completion transition (Claude: the SDK `result`
message → Ready; Codex: `turn/completed` → Ready), and after an explicit
interrupt (Claude: `turn-interrupted` → Ready; Codex: the `turn/completed`
an interrupt produces), the session pops the next queued turn (FIFO) and
dispatches it through the same internal send path
(`drain_queue`). Dispatch happens **after** the completion handler
releases the session lock, so `do_send` can re-acquire it without
deadlocking. Draining only fires when the session is truly idle (Ready,
no active turn, no pending approval) — an interrupt clears the active turn
and auto-aborts any pending approval, so both preconditions hold. On
session error/close the whole queue is cancelled (`cancel_all_queued`); an
**explicit interrupt no longer counts as a close**, so the queue is
preserved and drained rather than cancelled. A spontaneous abort that ends
the session (Claude `session-ended {reason:"interrupted"}` not driven by
the interrupt RPC) still translates to a close and cancels the queue. A
failed dispatch cancels that item and continues — the queue never wedges.

**Events** (`ProviderRuntimeEvent`, forwarded through `forward_event`):

- `TurnQueued { thread_id, queued_id, client_nonce, text }` — a send was
  parked. `client_nonce` echoes the optimistic-send correlation token so
  the reducer greys the already-appended bubble instead of duplicating
  it; `text` lets a remounted pane reconstruct the bubble.
- `QueuedTurnDispatched { thread_id, queued_id, turn_id, text }` — the
  queued turn is now active. `forward_event` persists the user-message
  envelope **here** (not at enqueue time) so chat history reflects real
  turn order. The reducer promotes the greyed bubble to a normal user
  message and re-seqs it to the dispatch-time tail.
- `QueuedTurnCancelled { thread_id, queued_id }` — the queued turn was
  cancelled (by the user or by session close/error). The reducer removes
  the greyed bubble.

**Command.** `agent_chat_cancel_queued_turn(provider, thread_id,
queued_id)` cancels a queued turn (idempotent). Wrapper:
`agentChatCancelQueuedTurn` in `src/tauri/commands.ts`.

### Send now (steer)

The greyed queued bubble also exposes a hover-reveal **"Send now"** action
(a `CornerDownLeft` icon in the same pill row as Cancel) that dispatches
the queued message *immediately* instead of waiting for the active turn to
finish. This mirrors Claude Code's interrupt-and-resubmit steering path.

The command `agent_chat_send_queued_turn_now(provider, thread_id,
queued_id)` (wrapper `agentChatSendQueuedTurnNow`) routes through the trait
method `send_queued_turn_now` (default no-op for OpenCode) to the session's
`send_queued_now`. That method, under the state lock:

1. **Promote to front.** Locate `queued_id` in `queued_turns` and move it
   to the front (`promote_queued_to_front`) so it is the next item the
   drain dispatches. An unknown / already-dispatched / already-cancelled id
   is a silent no-op `Ok(())` — the same idempotency philosophy as
   `cancel_queued`, covering the race where the item dispatched between the
   UI click and this call.
2. **Interrupt + drain.** If a turn is active it calls the existing
   interrupt path. The active turn ends, but the session, the full
   transcript, and all on-disk work survive — **nothing** is discarded.
   - *Claude:* `interrupt(None)` drives the sidecar's explicit interrupt
     RPC, which awaits the SDK query iterator's exit and emits a
     `turn-interrupted` notification (suppressing the old
     `session-ended {reason:"interrupted"}` the SDK query would otherwise
     have thrown). Rust translates that to
     `TurnCompleted { status: Error { subtype: "interrupted" } }` plus
     `SessionStateChanged(Ready)` — the interrupted turn closes out, but the
     sidecar session object survives with the same thread id (**Ready**, not
     **Closed**). The **next** `send-turn` transparently rebuilds a resumed
     SDK query (fresh prompt queue, `resume` set to the last observed
     sdk-session-id, which is re-observed and re-emitted so the host's resume
     cursor stays current), so the promoted item dispatches onto that rebuilt
     query. Rust ends `interrupt()` with `drain_queue()` and the
     notifications task also drains when the `turn-interrupted` → `Ready`
     transition lands; a `dispatching` guard lets those two drains race
     without double-dispatch.
   - *Codex:* `interrupt_turn(None)` does not drain inline — the abort lands
     as a `turn/completed` notification that returns the session to `Ready`
     and the event loop drains there. If `interrupt_turn` returns the "no
     active turn" `ValidationError` (turn finished during the click→call
     race) it falls back to `drain_queue()` directly.
   - If the session is already idle (the turn finished before the call) it
     just `drain_queue()`s.
   - If the interrupt RPC itself fails, `send_queued_now` restores the
     promoted item to its original queue position rather than leaving the
     queue silently reordered (both the Claude and Codex paths do this).

The promoted message then dispatches through the identical
`QueuedTurnDispatched` path as a normal drain, so the reducer promotes the
greyed bubble and `forward_event` writes the deferred user-message envelope
(with any `pending_queued_images`) exactly as before — no persistence
changes were needed.

**Pending-approval behaviour.** `drain_queue` refuses to dispatch while
`pending_approvals` is non-empty, so an outstanding tool approval would
otherwise wedge a promoted send-now message behind a tool call that the
interrupt already killed. To avoid that, an interrupt now **auto-aborts**
any outstanding approval: the sidecar emits `request-resolved` (deny, with
message `"Tool request was aborted."`) for the dead tool call, and the Rust
side clears its `pending_approvals` entry on that notification. Because the
notifications task drains the queue after **every** notification, resolving
the approval immediately triggers dispatch — so the promoted message drains
right away instead of waiting for the user to resolve an approval whose tool
call no longer exists. (The approval card resolves as denied/aborted in the
UI.)

**Frontend.** `handleSendQueuedNow` in `AgentChatPane` calls the command
with no optimistic state change — the `queued_turn_dispatched` event
promotes the bubble and the interrupt's `ready`/`running` state events
settle the composer. Errors surface via a toast, like `handleCancelQueued`.
The `onSendQueuedNow` prop threads through
`ChatTranscript → MessageList → ItemRow → UserMessage`.

**Frontend.** `UserMessageItem` gained `queued?: { queuedId }` (greyed
render + "Queued" pill + hover-X) and `clientNonce?` (reconciliation +
error rollback). Queued items sort to the very bottom via a `QUEUED_SEQ_BASE`
offset so they stay below the streaming turn. The composer's `handleSubmit`
attaches a `client_nonce`, treats a queued result as success (the
`turn_queued` event greys the bubble), and on a genuine RPC failure rolls
the optimistic bubble back and restores the draft (fixes the orphan bug).
The Composer allows Enter-to-queue while streaming (blocked only during
the in-flight send RPC) and shows a subtle "Enter to queue" hint;
cancelling a queued item restores its text into the composer draft.

**Persistence/hydrate note.** The queue is in-memory session state; there
is no "get session status" command that carries a queued-turns snapshot,
so a pane that remounts mid-queue does NOT re-render the still-pending
queued items until they dispatch (they still drain correctly
backend-side). Adding the snapshot to a status command is a possible
follow-up.

**Out of scope (TODO):**

- **True inject-into-live-turn steering** (feeding the message into the
  *running* turn via the SDK's `streamInput` without interrupting).
  Remains out of scope pending SDK support. The interrupt-based **"Send
  now"** above (interrupt + promote-to-front + drain) is **done** and
  covers the steering use-case for now.
- **Editing a queued message in place** (cancel-restores-to-composer
  covers the common case).
- **Persisting the queue across app restart.**
- **OpenCode queue parity.**

## Feature flag

The new flag `enable_agent_chat` lives on the existing `FeatureFlags`
struct in `src-tauri/src/observability.rs`. It defaults to `false`;
the entire provider registry (Claude + Codex adapters) skips
initialisation while it's off, saving the memory.

Three ways to flip it on locally:

1. **UI:** once the chat pane ships in Step 4 the settings panel will
   expose this. For now, call the existing
   `update_feature_flags` Tauri command from the browser devtools:

   ```js
   await window.__TAURI__.invoke("update_feature_flags", {
     flags: {
       unstable_browser_automation: true,
       unstable_indexing: true,
       enable_agent_chat: true,
     },
   });
   ```

2. **Config file:** edit the per-build snapshot at
   `~/.local/share/codemux/observability.json` (release) or
   `~/.local/share/codemux-dev/observability.json` (debug build), set
   `feature_flags.enable_agent_chat: true`, restart.

3. **Fresh install:** the default store is persisted lazily, so a
   brand-new Codemux install has no file yet. Start the app once
   (to create the file), then edit it and restart.

After the flag is on, open the browser devtools and call:

```js
await window.__TAURI__.invoke("dev_agent_chat_spawn_test_pane");
```

to insert a stub chat pane in the active workspace.

## Dev affordances

A dev-only "Spawn chat pane" button lives in the window title bar in
debug builds when `enable_agent_chat` is on. It invokes
`dev_agent_chat_spawn_test_pane` to drop a chat pane into the active
workspace. Useful for quick manual testing without going through the
sidebar `+` flow.

Under `npm run dev` (plain-browser mock), the seeded
**agent-chat-demo** workspace carries an `agent_chat` pane bound to
`MOCK_CHAT_THREAD_ID`. The mock hydrates a ~790-row transcript through
the real reducer (`agent_chat_list_messages`), streams a simulated
reply on `agent_chat_send_turn`, and exposes
`window.__codemuxChatMock.streamReply()` for on-demand streaming —
the standing harness for transcript-virtualization and scroll-pinning
work. It also seeds one **subagent turn** (an "Implement" completed +
"Verify" running orchestration card) and exposes
`window.__codemuxChatMock.streamSubagents()` for a live two-subagent
lifecycle over the real Channel — the harness for the subagent view.

## Step 9 — Cross-provider MCP server runtime (shipped)

Codemux now hosts user-installed MCP servers as first-class
infrastructure: discovers configs across Codemux/Claude/Cursor paths,
spawns each server once, exposes their tools to Claude SDK sessions
through an in-process facade, and surfaces enable/disable controls in
both Settings and the composer's `+` popup. Stages 1–6 (config
discovery → backend runtime → SDK facade → polish → Codex spike →
cleanup) all shipped on this branch.

See `docs/features/mcp-server.md` for the canonical feature description
and `docs/research/step-9-mcp-servers.md` for the original research +
locked design decisions. The Stage 5 spike at
`docs/research/step-9-codex-mcp-spike.md` recommends Step 11 as the path
to extend MCP host support to Codex via an HTTP gateway.

## Roadmap (next steps)

Roadmap and build order live in `docs/core/PLAN.md` § "Cross-Cutting Steps"
(Steps 6–13 plus the Step 10.5 / Step 11 planned work) and § "Immediate
Priority Order". The "promote agent-chat from Beta to default-on" item that
used to sit in that priority list is **done** — PR #232 shipped it in
`v0.16.0`, and Step 13 is marked LANDED. This doc stays about current
behavior; the duplicate step list that used to sit here has been removed
rather than kept in sync in two places.

## Known follow-ups

Verified outstanding as of this pass. Items that were listed here and have
since shipped (Claude image attachments end-to-end, `respond-to-user-input`
plumbing / the AskUserQuestion answer flow, and the `supportedModels` /
`supportedCommands` RPCs) have been removed — see § "Attachments",
§ "AskUserQuestion answer reply (PR #165)", and § "Slash command popup".

- **Recoverable thread-resume snippets.** The substring list in
  `agent_provider/codex/protocol.rs`
  (`RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS`) is inferred from an upstream
  reference and has not been verified against real `codex app-server` error
  output. A mismatch degrades gracefully — the resume fails instead of
  falling back to a fresh start — but refining the list would improve UX.
- **Codex turn-start parameters still unplumbed.** `TurnStartParams` models
  `service_tier`, `effort`, and `collaboration_mode`. `send_turn`
  (`codex/session.rs`) now populates `model` and `effort`; `service_tier`
  and `collaboration_mode` are still hardcoded `None`. Note the Codex speed
  picker drives fast mode through the *session launch* settings, not through
  this per-turn field.
- **Unused SDK `Query` methods.** A number of SDK query methods are still
  not exposed as sidecar RPCs (`setMaxThinkingTokens`, `applyFlagSettings`,
  `supportedAgents`, `mcpServerStatus`, `getContextUsage`, `reloadPlugins`,
  `accountInfo`, `rewindFiles`, `seedReadState`, `reconnectMcpServer`,
  `toggleMcpServer`, `setMcpServers`, `streamInput`, `stopTask`). Add them
  piecemeal as UI features require. (`supportedModels` and
  `supportedCommands` were on this list and are now live as the
  `list-models` / `list-commands` RPCs.)
- **Claude dogfood testing.** The ignored `claude_real_session` test covers
  a real content-delta round-trip and needs a logged-in `claude` CLI. Keep
  it on the release checklist for user-facing Claude changes.
- **Fast mode is withheld for Claude.** `merge_sdk_with_maintained` clamps
  `supports_fast_mode` to `false` for every Claude model because the SDK
  flag is a capability advertisement, not an entitlement check. Re-enabling
  requires an entitlement-feedback loop first — see
  `docs/research/opus-5-agent-chat-support.md`.
