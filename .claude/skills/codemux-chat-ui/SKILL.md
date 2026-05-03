---
name: codemux-chat-ui
description: Use when building, modifying, or reviewing the agent-chat pane, chat-home empty state, composer, approval affordances, streaming indicators, or any other UI surface that renders a conversation with a CLI coding agent. Extends `codemux-ui` (which owns tokens, compound picker, hover-reveal, overlay manager, terminal/browser pane chrome) with chat-specific density, typography, and color rules.
---

# Codemux Chat UI Standards

Principles, not pixel values. The visual target is the set of upstream reference screenshots the author attaches to chat-UI prompts; this skill extracts the rules those screenshots embody and restates them so they can be applied consistently across the agent-chat feature surface.

For shell-wide standards (color tokens, compound picker pattern, hover-reveal, overlay manager, row anatomy, motion, ADE feature patterns), read the `codemux-ui` skill. That skill still owns terminal and browser pane chrome. This skill owns everything inside a chat pane plus the chat-home empty state.

---

## Philosophy

The chat pane is a **calm, text-forward transcript**. It reads like a conversation log, not a chat app. Nothing in the interior competes with the prose. Secondary affordances — tool invocations, file references, approvals — sit inside the flow at a lower density than the prose itself, not beside it in decorated cards.

Four commitments follow from this:

- **Prose leads.** Assistant text is the dominant visual element. Everything else is quieter.
- **One continuous column.** Messages do not alternate sides, wear avatars, or get timestamps. The transcript reads top-to-bottom as one voice with occasional user interjections.
- **Structure inside the flow, not beside it.** Tool calls appear as dim one-line status markers inside the transcript stream; their output appears in minimal content blocks immediately below. No sidebar, no floating panels.
- **Accent nowhere in the conversation.** Theme colors belong in the app shell (sidebar active row, pane-level attention indicators, notifications). Inside a chat pane they add noise without carrying meaning.

---

## Three-Tier Density Model

Every visual element in a chat pane belongs to exactly one of three tiers. The tier determines its typography, color, padding, and how much breathing room it claims.

### Tier 1 — Prose

The body of assistant text and user text. The most generous tier.

- Comfortable line-height for reading.
- Reading-column width capped so long lines don't fatigue the eye — the column stays centered even when the pane is wide.
- Ample vertical space between consecutive messages so the transcript breathes.
- Uses the foreground text color at full strength.
- No decorative chrome on assistant messages — they're plain paragraphs.

### Tier 2 — Status lines

One-line tool markers inside the transcript flow. Examples: "Read `src/lib.rs`", "Edit `main.ts`", "Ran bash".

- Noticeably tighter vertical space than prose.
- Single line that wraps gracefully to a second line only when the argument is long.
- Format is consistent: **verb + target + dim argument**. The verb names the tool action in natural English ("Read", "Edit", "Ran", "Fetched"), the target is the primary object (file path, URL, command), and any secondary argument is rendered in the dimmer text tier.
- No background fill, no border, no card chrome. Just inline typography with color distinguishing the parts.
- Appears directly in the transcript stream between messages — NOT in a separate panel.

### Tier 3 — Content blocks

The optional payload attached to a status line. Examples: the diff for an Edit, the stdout for a bash command, the rendered content for a file read.

- Monospace typography.
- Contained in a subtle surface (slightly recessed or softly bordered), minimal chrome, no header bar, no tab strip.
- Generous enough padding to read the code, tight enough that the block doesn't dominate the viewport.
- Collapsible by default once the block is large — show a leading preview with "Show more" affordance. Use restraint: a short block shouldn't have a disclosure triangle.
- Zero accent color. Syntax highlighting (when present) stays neutral — foreground, muted, and dimmer only.

The three tiers compose: a tool invocation is a status line (Tier 2), and if the tool produced meaningful output, a content block (Tier 3) sits immediately below it. Assistant prose before and after (Tier 1) is unaffected by the nested structure.

---

## Typography Rhythm

One prose scale. Do not mix type sizes across messages.

- Prose uses the system UI font at a reading-comfortable size — noticeably larger than chrome text, smaller than marquee headlines.
- Status lines use the same font family as prose but the dimmer text color and slightly tighter vertical rhythm.
- Content blocks use monospace. Never use monospace inside prose — code references in the middle of a sentence are inline-styled (subtle background + slightly reduced size) but stay in the prose font if the reference screenshots show them in prose.
- Chat-home empty-state headline is the single place in this pane that exceeds prose size. It's a generous marquee heading, not a card title.
- No bolding for emphasis inside assistant prose unless the source text explicitly asks for it (markdown). Do not auto-bold names, terms, or section titles.

---

## Color Restraint

The entire conversation uses exactly three text tiers derived from shadcn's neutral scale:

- **Foreground** — prose body, user-message pill text.
- **Muted** — status-line verbs and targets, secondary copy, content-block chrome.
- **Dimmer** — status-line arguments, metadata, token counters, auxiliary labels.

That's it. No `--primary`, no `--success`, no `--warning`, no `--danger` inside a chat pane's conversation or composer. Accents belong in the app shell (sidebar active row, pane-level attention signal, desktop notifications) — not inside the pane where a person is reading.

Exceptions — small, deliberate:

- Destructive confirmations (a hard-delete button on a message if that ever ships) may use `--danger`, styled as `variant="destructive"` per `codemux-ui`.
- Errors inside a tool result may render the error label in `--danger`, but the surrounding content block stays neutral.
- Streaming indicator (see §11) is subtle and neutral, not colored.

---

## Message Anatomy

Two message shapes. No avatars, no role labels, no timestamps.

### User messages

A filled pill. Background is a slightly recessed neutral surface, foreground text, rounded. The pill is right-aligned to signal authorship by position only — no "You" label, no avatar, no timestamp beside it. The pill is as wide as its content up to the reading column cap; long messages wrap.

### Assistant messages

Plain prose. No container, no border, no background fill, no avatar, no "Assistant" label. Assistant text flows left-aligned in the reading column and is visually identical to any other prose the pane emits. The authorship comes from position and context, not labels.

Consecutive messages from the same role group visually by sharing a vertical rhythm — no hard separator between them beyond the natural paragraph spacing.

---

## Status Lines

One canonical shape: **verb + target + argument**.

- **Verb** — a short natural-English action ("Read", "Edit", "Ran", "Fetched", "Wrote", "Searched"). Muted tier.
- **Target** — the primary object the tool acted on. Usually a file path, URL, or command name. Muted tier, monospace when it's a path/URL/command, prose font otherwise.
- **Argument** — optional detail rendered in the dimmer tier ("+14 −2", "200 OK", "exit 0"). Sits after the target on the same line.

A line remains single-line whenever possible. On a narrow pane, it wraps to a second line without extra chrome. Status lines never get a background, border, or icon. An icon is tempting and wrong — it raises the density of the line above the prose tier and pulls the eye.

A status line that has no associated content block (e.g., a bash that produced no stdout) still appears in the transcript; the absence of a block is signal.

A status line that has output renders its block immediately below, with no arrow, no "Output:" label, no tab strip. Block and status belong together visually.

---

## Content Blocks

Monospace. Subtle container. Minimal chrome.

- The container is one of: a very slightly recessed surface with rounded corners, OR a soft left border. Pick one per-product and stick with it. No shadows, no gradients, no tab strips.
- No header bar. The status line above the block supplies the context.
- Code blocks inside assistant prose use the same style as tool-output blocks — consistency across sources keeps the pane quiet.
- Large blocks collapse to a leading preview with a one-line affordance to expand. The affordance lives at the bottom of the preview, muted tier, natural language ("Show 42 more lines").
- Diff blocks use two neutral shades plus the muted tier for line-number gutter. No green/red for additions/deletions inside the chat pane — the sign in the left gutter (+/−) and the neutral shade shift carry the meaning. This is the single sharpest departure from industry-standard diff styling and is deliberate: the chat pane is where conversation happens, the dedicated diff viewer is where colored diffs live.

---

## Composer

Pinned to the bottom of the pane. Has three distinct zones stacked vertically.

### Zone 1 — Target picker (above the composer)

A narrow strip just above the composer that indicates what the turn will act on — the target working directory, active context, or attached file. Compact; can be hidden when there's nothing to show. The picker itself uses the compound-picker pattern from `codemux-ui` (Popover + Command).

### Zone 2 — Textarea

The input surface. Generous internal padding. Placeholder in the dimmer tier. Grows vertically as the user types up to a reasonable cap, then scrolls. No border at rest; subtle focus ring when active. No send-on-enter toggle in the composer UI; keyboard behavior is implementation-level.

### Zone 3 — Footer row

One line of controls at the bottom of the composer, spanning its full width. Left side carries the compound pickers for provider, model, effort, and permission mode. Each is a pill-style trigger identical in shape to the triggers documented in `codemux-ui`'s Compound Picker Pattern; clicking opens a Popover + Command picker. Right side carries the submit button.

Every control in the footer follows the restrained color model — pills are neutral, submit is neutral unless the screenshots show otherwise. No colored backgrounds.

---

## Chat-Home Empty State

When a chat thread has no messages, the pane shows only the composer, centered.

- No sidebar chrome beyond what `codemux-ui` already supplies.
- A single large marquee headline above the composer. This is the one place in the chat feature that exceeds prose size. Keep it short.
- Composer (all three zones) renders identically to its in-conversation form. Same width cap as the reading column, centered.
- No grid of example prompts, no quick-action buttons, no marketing copy. The composer is the invitation.

Once the first user turn is sent, the pane transitions into the transcript view; the headline disappears and the composer drops to the bottom pin.

---

## Pane Header

Chat panes have a richer header than terminal or browser panes because they host the provider + model + effort + permission controls. `codemux-ui`'s 28-34px terminal-pane header rule does not apply here.

- Height grows as needed to hold the controls, but stays ≤ 40px. If the compound pickers don't fit, collapse lower-priority pickers into a `…` menu rather than grow the header.
- Left side: thread title (may be generated from the first user message). Single line, truncated. Muted when nothing is typed yet.
- Right side: the same compound picker triggers that appear in the composer footer, rendered at the same pill scale.
- No accent background. No border on the active pane beyond what `codemux-ui` already prescribes.

---

## Approval Prompts

When the agent requests a tool approval (command, file-change, file-read, user-input), the prompt appears **inline in the conversation flow**, not as a modal or overlay.

- Structurally: sits as a distinct content block immediately after the status line that triggered it.
- The block contains just enough context for a decision: the tool name, the proposed input (rendered as a Tier-3 content block), and the affordances.
- Affordances: Allow / Deny / Allow-for-session, with Deny carrying an optional reason textarea that reveals inline on click. Allow is the primary action. None of the buttons use accent color.
- Once resolved, the prompt collapses into a short status line acknowledging the decision ("Allowed", "Denied: …") so the transcript stays readable when scrolled.
- While pending, the pane retains focus on the prompt but does not block the scrolling transcript.

---

## Streaming Indicators

Subtle. Inline. Not attention-seeking.

- While an assistant message is streaming, the trailing cursor position shows a small neutral indicator (a steady dot or a gentle pulse). Never a spinner, never a color change, never a "typing..." label.
- Token / cost meters, when shown, live in the composer footer or pane header in the dimmer tier. They update live but do not animate attention — just a changing number.
- The interrupt affordance (when a turn can be halted) replaces the submit button during streaming. It's the same pill shape, neutral, labeled "Stop" or an ⎔ icon.
- Nothing on-screen should move by more than a few pixels during streaming. No layout shifts, no accordion expansions, no scrolling unless the user was already pinned to the tail.

---

## Do Not

- **No timestamps.** Not on messages, not on status lines, not anywhere in the transcript.
- **No avatars.** Not for the user, not for the agent, not for tools.
- **No role labels.** Never write "User:" or "Assistant:" or "System:" in the chat content.
- **No borders on user messages beyond the fill.** The recessed fill IS the boundary.
- **No rendered markdown headers in-line with prose.** If the SDK emits `# Heading` inside an assistant message, render it as emphasized prose, not as a browser-like `<h1>`. Size hierarchy inside a pane is reserved for the empty-state headline.
- **No accent colors in the conversation.** Not for streaming, not for tool cards, not for the user message fill, not for the submit button, not anywhere inside the pane chrome or the transcript.
- **No cards around status lines.** No backgrounds, no borders, no icons, no dividers. Status lines live inline in prose rhythm.
- **No attention-seeking animation.** No bouncing, no pulsing beyond the streaming cursor, no color flashes.
- **No modal approvals.** Approvals are inline in the transcript, not pop-ups.
- **No alternating message sides beyond the user-message right-align.** Assistant prose is always left-aligned; do not introduce middle or alternating layouts.
- **No sidebar inside the pane.** Tool output, file references, approvals — everything lives in the single vertical transcript column.
- **No emoji decoration** in pane chrome, placeholder copy, or status lines.

---

## Reference UIs

The upstream reference screenshots are the source of truth for the visual target. The author will attach them to chat-UI implementation prompts. Which screenshot demonstrates which pattern:

- **Screenshot 1 — Empty state.** Single composer centered, target picker visible above it, marquee headline. Everything this skill says about the chat-home empty state derives from this image.
- **Screenshot 2 — Mid-stream prose + status lines.** An in-progress conversation showing assistant prose flowing top-to-bottom, punctuated by Tier-2 status lines. The source for the verb+target+argument shape and the prose-leads density.
- **Screenshot 3 — Stacked tool progression.** Consecutive status lines (Read, Edit, Ran) with their content blocks nested below each. The source for how multiple tool calls compose without introducing panel chrome.
- **Screenshot 4 — Content block rendering.** A larger tool-output block, showing container treatment, monospace typography, and the collapsed-preview affordance. The source for the Tier-3 container style.
- **Screenshot 5 — Code / file display + permission-mode picker.** A diff or file block in the transcript together with the composer footer showing the compound pickers (model / effort / permission mode). The source for the composer footer layout and the in-transcript code/diff styling (neutral-only, no green/red).

When the upstream screenshots conflict with a rule in this skill, the screenshots win — tell the human and ask for the rule to be updated. When the screenshots are silent on a detail, fall back to the principles above, not to industry-standard chat-app conventions.
