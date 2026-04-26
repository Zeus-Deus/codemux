> Step 8 implementation research, completed 2026-04-26. See `docs/plans/` for index.

# Step 8 Research Deliverable — Attachments + Context System

This document is the locked research for the attachment system added in Step 8
(files, folders, GitHub issues / PRs, images via `+` button + `@` mentions). It
captures findings from external research (Cursor 3 reverse-engineering,
multimodal SDK formats) and from a survey of the existing Codemux scaffolding,
then proposes a 7-stage vertical-slice plan. All decisions below are locked.

---

## Task 1 — Cursor reverse-engineering

### Findings

**Files via `@filename` and `+` picker behave the same.** Both feed the same
context-assembly pipeline; the picker is a click-driven alternate trigger
([Cursor @-files docs](https://docs.cursor.com/context/@-symbols/@-files-and-folders)).
Content gets inlined into a single API request alongside system prompt, tool
schemas, and prior turns — Cursor doesn't farm out to a `read_file` tool for
`@`-mentions
([Munn, "Anatomy of a Cursor Prompt"](https://medium.com/@johnmunn/the-anatomy-of-a-cursor-prompt-f7146f9bdd4e)).

**But "inline" is heuristic.** Schmalbach's reverse-engineering of a 4000+ line
JS file showed Cursor often sends **outline (function names + line numbers) +
a small region around the cursor**, with the model itself reporting it didn't
have full content
([vincentschmalbach.com](https://www.vincentschmalbach.com/force-full-context-in-cursor-ide-workaround-for-large-files/)).
No public hard threshold; behavior is opaque.

**Folders default to tree + bounded file content.** A `Full Folder Content`
setting tries to inline every file; large folders fall back to outline-only
with a tooltip indicating how many files were actually included.
`.cursorignore` and `.gitignore` are honored
([@-folders](https://docs.cursor.com/context/@-symbols/@-folders)).

**Large/binary files** are truncated via outline + locality slice. No public
refusal list — relies on indexing heuristics.

**Images** are supported since Cursor 0.17.0, with persistent papercuts:
clipboard paste broken in Agent chat on some builds
([forum 156818](https://forum.cursor.com/t/agent-chat-clipboard-image-paste-ctrl-v-context-menu-does-not-work-drag-and-drop-file-works/156818));
Linux/Windows CLI lack paste
([forum 148471](https://forum.cursor.com/t/image-pasting-support-on-linux-in-cursor-cli/148471)).
Wire format is the underlying provider's standard image content block.

**No native `@PR` / `@issue`.** GitHub goes through MCP; users have an open
feature request specifically asking for it
([forum 147968](https://forum.cursor.com/t/proposal-commands-for-github-pr-comments/147968)).
**This is a clear differentiation lane for Codemux.**

### Proposed approach for Codemux

| Type | Strategy |
|---|---|
| Small files (≤ 200 KB / ≤ 1500 lines) | Full content inline + path header + line count |
| Large files | Path + line count + first 50 lines + outline (regex over `function`/`class`/`export`/`impl`/`fn`/`def`) + "use Read tool to see the rest" hint |
| Folders | Tree only (depth-bounded) — agent uses Read/Grep on demand. Don't repeat Cursor's "Full Folder Content" footgun by default. |
| Binary files | Reject with chip-level error ("binary file not supported") except images |
| Issues / PRs | Full title + state + body + comments (first 20 + count) inline. Issue body already truncated to 50 KB in `github.rs:366-374`. PRs additionally get `gh pr diff --name-only` by default; full unified diff on demand. |
| Images | Native multimodal content block (Task 7) |

**Differentiation play vs Cursor:** be transparent about truncation in the
chip itself ("Composer.tsx · 421 lines · first 50 inlined"), and ship native
`@pr` / `@issue` autocomplete via existing `gh` CLI integration. Both are
documented Cursor papercuts.

---

## Task 2 — `+` button design

### Findings

Codemux already has the scaffolding. The slash popup at
`src/components/chat/SlashCommandPopup.tsx` is a generic component
(`SlashCommandItem[]` with `id`, `label`, `description`, `command`, `icon`,
`onSelect`, `group`). Composer integrates it via cursor-aware detection
(`Composer.tsx:317-355`), with grouped rendering, cmdk filtering, mouse-down
preventDefault to retain textarea focus, and keyboard nav.

The current chip pattern is `ModePill`
(`src/components/chat/pickers/ModePill.tsx:73-108`) —
`rounded-full px-2.5 py-1 text-xs`, `bg-{role}/15` + `text-{role}` color
rules, icon + label + X removal. Per the chat-ui skill rules, that 15% fill
is the canonical chip token.

`ComposerFooter.tsx:94-109` already conditionally swaps a `+ Mode` dropdown
trigger for a `ModePill` when `mode !== "default"`. **The same dropdown
pattern already exists for mode** — `+` for attachments mirrors it.

### Proposed approach

**Reuse `SlashCommandPopup` as-is.** Add a new top-level grouping schema and
triggering origin. Don't fork.

**`+` button placement: persistent, left side of composer footer**, alongside
the existing `+ Mode` dropdown trigger. Don't replace the send button — that's
confusing when the user has typed text and also wants to attach. Cursor 3
keeps the `+` persistent and Codemux should match.

**Menu structure** (single popup, grouped, no submenus — keeps keyboard nav
clean):

```
ATTACH
  📄  File…              Pick file from project
  📁  Folder…            Pick folder from project
  🐙  GitHub Issue…       Pick from open issues
  🔀  GitHub PR…          Pick from open PRs
  🖼  Image…              Pick image from disk

NAVIGATION (just links — close popup, refocus textarea pre-loaded)
  /   Slash commands     Type /
  @   Mention            Type @
```

Picking "File…" pushes the popup into a *file-search submode* (same component,
same keyboard model, query is the search string). This avoids cascading
submenus and reuses the cmdk filtering already in place.

**Modes and Skills do NOT appear in `+`.** They live on `/`. Cursor merges
them; Codemux should keep them separate to make the mental model crisp:
`/` = behaviors, `+`/`@` = context. The "Navigation" section above is just a
discoverability nudge, not a duplicate command surface.

---

## Task 3 — `@` mention popup design

### Findings

Per the codebase exploration: Composer detection for `/` uses
`findSlashAtCursor()` (regex anchored at start-of-line or after whitespace;
`Composer.tsx:317-355`). The same primitive can detect `@` with one regex
parameter change.

Skills today are **inline tokens parsed at send time**
(`src/lib/agent-chat/skill-tokens.ts:28`, `:76-90`), with `resolveSkillBodies()`
matching `(?<=^|\s)\/([A-Za-z0-9_-]+)` against the active skills registry,
deduping, and joining bodies. This is a viable pattern for `@`-tokens but does
*not* generalize to images, and it makes the "fetched at send-time" semantics
for issues/PRs surprising. So `@` behavior should diverge from `/` behavior
here.

### Proposed approach

**Trigger:** bare `@` at start-of-line or after whitespace, exactly like `/`.

**Scope: typed prefix narrows category.**

| Typed | Shows |
|---|---|
| `@` | All categories interleaved by relevance: top 5 files, top 5 issues, top 5 PRs |
| `@<query>` | Files matching `<query>` (most common case) |
| `@file:<query>` | Files only |
| `@folder:<query>` | Folders only |
| `@issue:<query>` | Issues only — numeric query treated as direct fetch |
| `@pr:<query>` | PRs only — numeric query treated as direct fetch |

The bare-`@` interleaved view is for discoverability; once users learn `@pr:`,
they can be specific. This matches Cursor's `@docs`, `@code`, `@web` pattern
but with PR/issue prefixes Cursor doesn't have.

**Selection behavior — the divergence call.** Picking from `@` should:
1. **Strip the typed `@<query>` from the textarea** (don't leave `@filename`
   behind).
2. **Add a chip** to `stagedAttachments` (Task 9 state).

This unifies the UX with the `+` button: both produce chips, neither leaves
textarea tokens. The reason to diverge from skills is that issues/PRs have
*fetched* state (title, status, body) and images have *binary* state — neither
can be reconstructed from a textarea token. Doing it uniformly across all
attachment types keeps the mental model simple.

**Reuse `SlashCommandPopup`.** Pass it `@`-grouped items. The component already
supports grouping (`groupSlashItems` at `SlashCommandPopup.tsx:84`). Add an
optional footer note slot to surface "Loading issues…" while `gh` is fetching.

---

## Task 4 — Backend: file/folder discovery

### Findings

- `src-tauri/src/commands/files.rs:24-95` has `list_directory(path, show_hidden)` — single-level walk, uses `git check-ignore` per entry (slow at scale), filters `.git` and common build dirs. **No recursive walk, no fuzzy match, no caching.**
- `commands/files.rs:286-315` has `search_file_names(path, query)` — runs `fd` (or falls back to `find`), substring-match only, no scoring, no caching. Closer fit but still misses fuzzy ranking + cache.
- `src-tauri/src/indexing.rs` exposes `search_project_index` — this is a **lexical content index** (chunks files into 40-line blocks, scores by term-count; see `search_snapshot` at `:474-536`). NOT suitable for `@filename` autocomplete.
- The skills store uses `Zustand persist` + 60s TTL (`skills-store.ts:48`) — good frontend-side cache pattern. Backend caching would benefit from the same TTL.

### Decision: new command

`search_project_index` is content search, not filename search. `search_file_names`
is filename search but lacks fuzzy ranking, scoring, and caching. **Add a
new `list_project_files` command alongside both.** Don't fork either existing
command.

### Proposed approach

**New Tauri command:** `list_project_files(cwd: PathBuf, query: Option<String>, limit: usize) -> Vec<FileMatch>`.

Implementation:
1. **Walker:** the `ignore` crate (the foundation of `ripgrep`). Respects `.gitignore`, `.ignore`, `.git/info/exclude` automatically.
2. **Cache:** in-memory `Mutex<HashMap<PathBuf, FileIndex>>` keyed by `cwd`. `FileIndex { paths: Vec<PathBuf>, scanned_at: Instant }`. TTL of 60 s.
3. **Fuzzy match:** the `nucleo-matcher` crate (used by Helix, lightweight). Score paths against query; return top `limit`.
4. **Performance budget:** at 50K files, in-memory match should be < 50 ms after first scan. First scan can be background-warmed when the chat pane mounts (fire-and-forget).

**Folders** are derivable from `paths` — group by parent directories, return distinct prefixes. Don't maintain a separate folder index.

---

## Task 5 — Backend: GitHub issues/PRs

### Findings

`src-tauri/src/github.rs` already exports a rich set of commands:

- `check_gh_status` (`:109-140`) — auth-state probe, three-valued enum
- `list_github_issues(repo_path, search)` (`:293-342`) — `gh issue list --search ... --state all --limit 20 --json` with 10s timeout
- `get_github_issue(number)` (`:344-379`) — returns body, **truncated to 50 KB at char boundary** (`:366-374`)
- `list_pull_requests(state)` (`:495-511`)
- `get_pr_review_comments`, `get_pr_inline_comments`, `get_pr_checks`, `get_pr_deployments` etc.
- **No caching anywhere.** Every call hits `gh`.

Frontend has *no current invocation* of any of these from the chat composer.

### Proposed approach

**Reuse heavily; add three thin commands and one cache layer.**

New / extended commands:
- Add `view_github_pr(number) -> PullRequestDetail` — title, body, state, comments, file list. Mirrors `get_github_issue`'s 50KB body truncation.
- Add `get_github_pr_diff(number, name_only: bool) -> String` — wraps `gh pr diff --name-only` or `gh pr diff`.
- Existing `list_github_issues` has a `search` param — sufficient for `@issue:<query>` autocomplete. Numeric queries treated as direct fetch.

**Caching layer (new, shared across all `github.rs` commands):**
- `Mutex<HashMap<CacheKey, CacheEntry>>` keyed on `(command_name, args_tuple)`.
- TTL: 60 s for list calls, 5 min for detail calls.
- Invalidation event: when the user's `gh` auth changes, or when they explicitly refresh.

**Auth degradation** uses the existing `check_gh_status` — if `NotAuthenticated`, the `@issue:` and `@pr:` paths in the popup return an empty list with a footer hint pointing to "Run `gh auth login`".

**Comment limit:** 20 comments + thread length count. Default cut at 20.

---

## Task 6 — File summarization

### Findings

External research: Cursor uses heuristic + first-N-lines + outline for large files; no LLM summarization at attach time (would be too slow and expensive). User-perceived behavior: "small files inline, big files truncated, sometimes silently."

### Proposed approach

**Tiered, all heuristic, zero LLM at attach time.**

```
For an attached file:
  1. Read file. Detect text/binary by null-byte sniff in first 8KB.
  2. If binary and not image: reject with chip-level error.
  3. Compute: lines, bytes, language (by extension).
  4. If lines ≤ 1500 AND bytes ≤ 200KB:
       inject FULL CONTENT, fenced.
  5. Else:
       inject FIRST 50 LINES + LANGUAGE-AWARE OUTLINE
       outline rules:
         .ts/.tsx/.js/.jsx: regex /^(export\s+)?(async\s+)?(function|class|const|let|var|interface|type|enum)\s+(\w+)/
         .rs:               regex /^(pub\s+)?(async\s+)?(fn|struct|enum|trait|impl|mod)\s+(\w+)/
         .py:               regex /^(async\s+)?(def|class)\s+(\w+)/
         .go:               regex /^(func|type)\s+(\w+)/
         .md:               regex /^(#{1,3})\s+(.+)/  → headings
         .json:             top-level keys (single-line if compact, parsed if pretty)
         (other):           none — just first-50 + truncation note
       Append: "Full file via Read tool: <path>"
```

The chip surfaces this transparently: "Composer.tsx · 421 lines · first 50 + outline (52 declarations)".

Don't LLM-summarize. The token + latency cost isn't worth it; the agent can read the file if it needs more.

---

## Task 7 — Image attachment

### Findings

**Anthropic** ([platform.claude.com vision](https://platform.claude.com/docs/en/build-with-claude/vision)): content block `{type: "image", source: {type: "base64"|"url"|"file", media_type, data|url|file_id}}`. Supported: jpeg, png, gif (non-animated), webp. Max 8000×8000 px (drops to 2000×2000 if > 20 images per request). Up to 100 images per 200K-context request. **Request size cap is 32 MB** — large images must use the Files API.

**OpenAI/Codex** ([developers.openai.com images-vision](https://developers.openai.com/api/docs/guides/images-vision)): two surfaces. Chat Completions: `{type: "image_url", image_url: {url: "..."}}`. Responses API: `{type: "input_image", image_url: "..."}`. URL accepts HTTPS or `data:image/png;base64,...`. 512 MB total; up to 1500 images. `detail: low|high|original|auto`.

**OpenRouter** normalizes on OpenAI shape; underlying provider determines support.

**Codemux today:** `agent_chat_send_turn` takes `SendTurnInput.text: String` only. No image plumbing in the adapter modules. The textarea has no `onPaste` or `onDrop` handlers.

### Proposed approach

**Internal representation (provider-agnostic):**

```rust
// in src-tauri/src/agent_chat.rs
pub struct ImageAttachment {
    pub mime: String,           // "image/png" etc.
    pub bytes: Vec<u8>,         // decoded
    pub source_label: String,   // for chip display: filename or "pasted image"
}

pub struct SendTurnInput {
    pub thread_id: String,
    pub text: String,                          // existing
    pub images: Vec<ImageAttachment>,          // NEW
    // ... existing model/effort overrides
}
```

**Adapter translation:**

- Claude adapter: build content blocks with `type: "image", source: {type: "base64", media_type, data: b64encode(bytes)}`. For images > ~5 MB, route through Files API (separate command).
- OpenAI/Codex adapter: `type: "image_url", image_url: {url: "data:{mime};base64,{b64}"}`.
- OpenRouter: same as OpenAI.

**Frontend handlers (new in Composer.tsx):**

- `onPaste`: synchronous `paste` event — `e.clipboardData.files` for files, iterate `.items` for `kind === 'file'`. No permission gate.
- `onDrop`: `e.dataTransfer.files`, branch on `file.type.startsWith("image/")`. `onDragOver` calls `e.preventDefault()` to enable drop.
- File → bytes: `await file.arrayBuffer()` → `Uint8Array` → ship over IPC. **Don't double-base64** in JS; let Rust encode.

**Capability detection:**

- New helper `provider.supportsImages(model)` reading from `list_chat_provider_capabilities`. If false, image attachments produce a disabled chip with tooltip "model X doesn't support images".
- Soft warning at composer level when total image bytes > 5 MB.

---

## Task 8 — Attachment chip UX

### Findings

Existing pattern at `ModePill.tsx:73-108`:

```tsx
<div className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs"
     // bg-{role}/15 text-{role}
>
  <Icon className="h-3 w-3" />
  <span>{label}</span>
  <button onClick={onRemove} className="ml-0.5 rounded p-0.5 hover:bg-foreground/10">
    <X className="h-2.5 w-2.5" />
  </button>
</div>
```

15% opacity fill is the canonical chat-ui chip token. Note: the `codemux-chat-ui` skill's "no accent in chat pane" rule technically excludes accents from the conversation; ModePill is the in-codebase precedent for a colored chip in the composer area, and AttachmentChip mirrors it.

### Proposed approach

**Component: `AttachmentChip`** (mirrors ModePill exactly, parameterized by attachment type).

| Type | Icon | Color | Label |
|---|---|---|---|
| File | `FileIcon` (lucide) | neutral (`bg-foreground/10 text-foreground`) | filename · `Nl` (line count) |
| Folder | `FolderOpen` | neutral | foldername · `N items` |
| Issue | `CircleDot` | warning if open, success if closed | `#1234` · title (truncated) |
| PR | `GitPullRequest` | primary (open) / muted (merged/closed) | `#1234` · title (truncated) |
| Image | `Image` | accent | filename or `pasted-image-{ts}.png` |

**Layout:**
- New container above the textarea inside the composer card, `flex flex-wrap gap-1.5 px-3 pt-2` — wraps when many.
- Empty when no attachments.
- ModePill stays in the footer (existing). AttachmentChips go above textarea.

**Limit:** soft cap at 20 attachments, soft warning at 10.

---

## Task 9 — Slice / draft state

```ts
interface Attachment {
  id: string;                          // uuid for chip dedup + removal
  kind: "file" | "folder" | "issue" | "pr" | "image";
  ref: string;                         // path | "#1234" | "image:<id>"
  metadata: {
    label: string;                     // chip display
    lineCount?: number;                // file
    bytes?: number;                    // file/image
    state?: "open" | "closed" | "merged";  // issue/pr
    fetchedAt?: number;
    isLoading?: boolean;
    error?: string;
  };
  resolvedContent?: string;            // text content
  resolvedImage?: { mime: string; bytes: Uint8Array };
}

interface ChatThreadSlice extends ChatThreadState {
  // ... existing fields
  stagedAttachments: Attachment[];     // NEW
}
```

Actions: `addStagedAttachment`, `updateStagedAttachment`, `removeStagedAttachment`, `clearStagedAttachments` (called on send).

Per-turn semantics: clears on send. Persistence: metadata only via existing thread-state persistence; resolved bytes/content not persisted (re-resolved on rehydrate).

---

## Task 10 — Composer composition order

Current pipeline (`src/lib/agent-chat/mode-prefix.ts:88-98`):

```ts
applyAllPrefixes(text, mode, effort, stagedSkillBody?: string | null): string
```

**Extend signature:**

```ts
applyAllPrefixes(
  text: string,
  mode: ChatMode,
  effort: string | null | undefined,
  stagedSkillBody?: string | null,
  attachmentBlock?: string | null,        // NEW
): string
```

**Order (text portion):**

```
[Ultrathink prefix]                  ← outermost, at top
[Mode wrapper open]
  [Skill body]
  ---
  === Attached context ===
  [File 1: path, summary, content]
  [File 2: ...]
  [Folder: path + tree]
  [Issue #1234: title + body + comments]
  [PR #5678: title + body + diff]
  === End context ===
  ---
  [User text]
[Mode wrapper close]
```

**Image content blocks live at the SDK message layer**, not in the text. Adapter weaves them into provider-specific content arrays. Canonical user turn:
`{role: "user", content: [{type: "image", ...}, {type: "image", ...}, {type: "text", text: <prefixed text>}]}` — images first, text last.

---

## Task 11 — Master staging proposal

**Stage 1 — File walk backend + state foundation.**
- Add `list_project_files` Tauri command.
- Add `stagedAttachments` slice + actions.
- Add `AttachmentChip` component (renders for `kind: file` only; other kinds rejected at runtime).
- Wire chip strip above textarea.
- *Acceptance:* devs can stuff an attachment into the slice and see a chip, click X, it's gone.

**Stage 2 — `@`-mention popup for files (vertical slice).**
- Extend `findSlashAtCursor` → `findMentionAtCursor`.
- Composer wires `@` detection; popup shows file matches via the new backend command.
- Selection inserts a chip and strips the typed `@<query>`.
- Extend `applyAllPrefixes` to accept attachment block; build file block per Task 6 tier rules.
- *Acceptance:* user types `@composer`, picks `Composer.tsx`, sees chip; on send, agent sees file content prefixed.

**Stage 3 — `+` button + folder support.**
- Wire `+` button in composer footer (left of mode dropdown).
- Reuse `SlashCommandPopup` with the attach-categories grouping.
- Submode for "File…" reuses Stage 2 popup; add "Folder…" submode.
- Folder injection: tree-only (depth-bounded).

**Stage 4 — GitHub issues end-to-end.**
- Add `gh` caching layer to `github.rs`.
- Wire `is_github_repo` preflight to gate the `+ → Issue…` and `@issue:` paths.
- Issue chip with state-color, title preview; injection per Task 5.

**Stage 5 — GitHub PRs (extends Stage 4).**
- Add `view_github_pr` and `get_github_pr_diff` commands.
- PR chip with state color; diff defaults to `--name-only`, click-to-expand for full diff.

**Stage 6 — Image attachment.**
- Extend `SendTurnInput` with `images: Vec<ImageAttachment>`.
- Adapter translation in Claude/Codex/OpenRouter paths.
- Composer onPaste / onDrop handlers; image chips.
- Capability detection: disable image attachment for non-multimodal models.

**Stage 7 — Polish.**
- Drag-drop visual feedback.
- Token cost preview in chip tooltip.
- Soft warning at 10 attachments / 5 MB image size.
- Truncation indicators on chips.
- Tree-sitter outlines (replacing regex outline) for major languages.
- Re-fetch logic for stale issue/PR fetches.

### Estimated complexity vs Step 6/7

- **Step 6 (Debug mode pill + marker grep)** — small. ~3 days of polished work.
- **Step 7 (cross-provider skills + watcher + popup integration)** — medium-large.
- **Step 8 (attachments)** — **larger than Step 7, ≈ 1.5–2× the work.**

---

## Open questions resolved

1. **`@file` and `+ → file picker` — same or diverge?** Same. Both produce identical chips and identical injection.
2. **Images for non-multimodal models?** Capability-detect; disable the image entry in the menu and show tooltip on chip if added.
3. **File modified between attach and send?** Re-read at send time. Chip stores metadata only.
4. **Attachment expiry / re-fetch?** Files: re-read at send. Folders: same. Issues/PRs: re-fetch at send if `fetchedAt` older than 60 s. Images: byte-stable, no re-fetch.
5. **Token cost visibility on chip?** Stage 7 polish. Tooltip with estimate.
