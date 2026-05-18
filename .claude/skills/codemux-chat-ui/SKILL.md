---
name: codemux-chat-ui
description: Use when building, modifying, or reviewing the agent-chat pane, chat-home empty state, composer, approval affordances, streaming indicators, or any other UI surface that renders a conversation with a CLI coding agent. Extends `codemux-ui` (which owns tokens, compound picker, hover-reveal, overlay manager, terminal/browser pane chrome) with chat-specific density, typography, and color rules.
---

# Codemux Chat UI Standards

Project-specific delta for the chat pane. Read `codemux-ui` for shell-wide rules (tokens, compound picker, hover-reveal, overlay manager). This skill is contrarian on purpose — it inverts industry-standard chat-app conventions.

---

## Philosophy

The chat pane is a **calm, text-forward transcript**. Reads like a conversation log, not a chat app. Four commitments:

- **Prose leads.** Assistant text is the dominant visual element.
- **One continuous column.** No alternating sides, no avatars, no timestamps. Top-to-bottom as one voice with occasional user interjections.
- **Structure inside the flow, not beside it.** Tool calls are dim one-line markers in the transcript stream; output sits in minimal blocks immediately below. No sidebar, no floating panels.
- **No accent color inside the conversation.** Accents belong in the app shell — not where a person is reading.

---

## Three-Tier Density Model

Every visual element belongs to exactly one tier. The tier determines typography, color, padding, breathing room.

### Tier 1 — Prose
Body of assistant and user text. Most generous tier. Comfortable line-height, reading-column width capped (column stays centered even when the pane is wide), ample vertical space between messages. Full-strength foreground color. No decorative chrome on assistant messages — plain paragraphs.

### Tier 2 — Status lines
One-line tool markers inside the transcript flow (e.g. "Read `src/lib.rs`", "Edit `main.ts`", "Ran bash").

Shape: **verb + target + dim argument**. Verb names the action in natural English ("Read", "Edit", "Ran", "Fetched", "Wrote", "Searched") in muted tier. Target is the primary object (path/URL/command), `font-mono` when it's a path/URL/command. Optional argument ("+14 −2", "200 OK", "exit 0") in the dimmer tier on the same line.

Tighter vertical space than prose. Single line that wraps gracefully when long. **No background, no border, no icon, no card chrome.** An icon raises the density above the prose tier and pulls the eye.

A status line with no output (e.g. bash that produced no stdout) still appears — absence of a block is signal.

### Tier 3 — Content blocks
Optional payload below a status line (diff for an Edit, stdout for bash, file contents for a Read).

Monospace. Subtle container — slightly recessed surface OR soft left border, pick one and stick with it. No header bar, no tab strip, no "Output:" label, no shadows or gradients. Status line above supplies the context.

Large blocks collapse to a leading preview with a muted one-line expand affordance ("Show 42 more lines"). Short blocks don't get a disclosure triangle.

Code blocks inside assistant prose use the same style as tool-output blocks — consistency keeps the pane quiet.

**Diffs are neutral.** Two neutral shades plus muted gutter; the `+`/`−` in the gutter carries the meaning. No green/red inside the chat pane. (The dedicated diff viewer elsewhere in the app uses color; the chat pane does not.)

---

## Typography Rhythm

One prose scale. Don't mix sizes across messages.

- Prose: system UI, reading-comfortable — noticeably larger than chrome text, smaller than marquee headlines.
- Status lines: same family as prose, dimmer tier, slightly tighter rhythm.
- Content blocks: monospace. Never monospace inside prose (inline code refs stay in prose font with subtle background).
- Chat-home empty-state headline is the only place that exceeds prose size.
- No auto-bolding. Only honor explicit markdown bold from source text.

---

## Color Restraint

Three text tiers from shadcn's neutral scale:
- **Foreground** — prose body, user-message pill text
- **Muted** — status-line verbs/targets, secondary copy, content-block chrome
- **Dimmer** — status-line arguments, metadata, token counters

That's it. No `--primary`, `--success`, `--warning`, `--danger` inside a chat pane's conversation or composer.

Small exceptions:
- Destructive confirmations may use `--danger` via `variant="destructive"`
- Errors inside a tool result may render the error label in `--danger`; surrounding block stays neutral
- Streaming indicator stays neutral, never colored

---

## Message Anatomy

No avatars, no role labels, no timestamps.

- **User messages** — filled neutral pill, slightly recessed surface, rounded, right-aligned. Width caps at the reading column. Authorship comes from position, not labels.
- **Assistant messages** — plain prose. No container, no border, no fill. Left-aligned in the reading column. Visually identical to any other prose the pane emits.

Consecutive messages from the same role share vertical rhythm — no hard separator beyond natural paragraph spacing.

---

## Composer

Pinned to the bottom of the pane. Three stacked zones.

### Zone 1 — Target picker
Narrow strip above the composer indicating what the turn will act on (working dir, active context, attached file). Hidden when nothing to show. Uses the compound-picker pattern from `codemux-ui`.

### Zone 2 — Textarea
Generous internal padding. Placeholder in dimmer tier. Grows vertically up to a cap, then scrolls. No border at rest; subtle focus ring when active.

### Zone 3 — Footer row
One line spanning the composer's full width. Left: compound-picker pills for provider, model, effort, permission mode — all neutral, pill-shaped, identical to `codemux-ui`'s picker triggers. Right: submit button (neutral unless screenshots dictate otherwise). No colored backgrounds.

---

## Chat-Home Empty State

When a thread has no messages, the pane shows only the composer, centered, with a single large marquee headline above it. This headline is the one place in the chat feature that exceeds prose size; keep it short.

No grid of example prompts, no quick-action buttons, no marketing copy. The composer is the invitation.

Once the first user turn is sent, the headline disappears and the composer drops to the bottom pin.

---

## Pane Header

Richer than terminal/browser pane headers because it hosts provider + model + effort + permission controls. The 28–34px rule from `codemux-ui` does not apply.

- Height: grows as needed, ≤ 40px. If pickers don't fit, collapse lower-priority ones into a `…` menu rather than grow the header.
- Left: thread title (may be generated from first user message), single line, truncated, muted when empty.
- Right: same compound-picker triggers that appear in the composer footer, same pill scale.
- No accent background.

---

## Approval Prompts

When the agent requests a tool approval, the prompt appears **inline in the transcript**, never as a modal.

- Sits as a content block immediately after the status line that triggered it.
- Contains just enough context: tool name, proposed input (as a Tier-3 content block), affordances.
- Affordances: Allow / Deny / Allow-for-session. Deny carries an optional reason textarea that reveals inline on click. Allow is primary. None use accent color.
- Once resolved, collapses into a short status line ("Allowed", "Denied: …") so the transcript stays readable when scrolled.
- While pending, the pane keeps focus on the prompt but doesn't block the scrolling transcript.

---

## Streaming Indicators

Subtle, inline, not attention-seeking.

- During streaming, the trailing cursor shows a small neutral indicator (steady dot or gentle pulse). Never a spinner, never a color change, never a "typing..." label.
- Token / cost meters live in the composer footer or pane header in the dimmer tier. They update live but don't animate attention.
- The interrupt affordance replaces the submit button during streaming. Same pill shape, neutral, labeled "Stop" or ⎔.
- Nothing on-screen should move more than a few pixels during streaming. No layout shifts, no accordion expansions, no scrolling unless the user was already pinned to the tail.

---

## Do Not

- **No timestamps.** Anywhere in the transcript.
- **No avatars.** Not for user, agent, or tools.
- **No role labels.** Never "User:", "Assistant:", "System:" in chat content.
- **No borders on user messages beyond the fill.** The recessed fill IS the boundary.
- **No browser-style markdown headers in prose.** Markdown `# Heading` renders as emphasized prose, not as `<h1>`. Size hierarchy is reserved for the empty-state headline.
- **No accent colors in the conversation.** Not for streaming, not for tool cards, not for the user message fill, not for the submit button.
- **No cards around status lines.** No backgrounds, no borders, no icons, no dividers.
- **No attention-seeking animation.** No bouncing, no pulsing beyond the streaming cursor, no color flashes.
- **No modal approvals.** Approvals are inline.
- **No alternating message sides beyond the user-message right-align.** Assistant prose is always left-aligned.
- **No sidebar inside the pane.** Tool output, file references, approvals — everything in the single transcript column.
- **No emoji decoration** in pane chrome, placeholders, or status lines.
