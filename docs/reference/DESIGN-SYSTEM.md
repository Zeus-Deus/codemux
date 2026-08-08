# Design System (tokens & theming)

- Purpose: The durable reference for Codemux's color tokens, theming layers, and the "no hardcoded colors" rule.
- Audience: Anyone doing UI, component, or visual-design work.
- Authority: Canonical theming reference. Runtime sources of truth are `src/lib/themes.ts` and the bridge in `src/globals.css`.
- Update when: Token names, the base shadcn preset, palette variants, or the density scale change.
- Read next: `docs/features/settings.md` (appearance), `docs/reference/ARCHITECTURE.md`

> This reference exists because PR #115 tokenized the whole app and removed
> the `.claude/skills/codemux-ui` skill that previously held these rules.
> When in doubt, read `src/lib/themes.ts` for values and `src/globals.css` for how roles reach utilities.

## Core Rule

**Never hardcode a color in a component.** Every themeable color is a CSS
custom property in `src/globals.css`, surfaced as a Tailwind utility through
the `@theme inline` block. Consume the utilities (`text-status-open`,
`bg-status-working/15`, `border-status-attention`, `text-accent-ember`, …),
never a raw Tailwind palette class (`text-emerald-500`, `bg-red-600`, …).

PR #115 swept ~40 components clean (176 replacements): `emerald`/`green` →
`status-open`, `amber`/`yellow` → `status-working`, `red`/`rose` →
`status-attention`, `sky`/`cyan`/`blue` → `status-remote`,
`purple`/`violet` → `accent-violet`. Verification bar: zero
`{prefix}-{color}-{NNN}` className utilities remain in `src/` (tests excluded).

### Intentionally-concrete exceptions

These are **not** theme colors and are left literal on purpose:

- terminal/editor/chat adapters receive concrete colors from the active theme
  (`themes.ts` → `use-theme-colors.ts`); renderers cannot consume CSS variables.
- the project-avatar color-picker hex list + dynamic per-project `rgba()`.
- inspector highlight CSS injected into the foreign inspected page.
- `BrowserPane` canvas `fillStyle` (`#000`).

## Token Layers

The runtime engine and `src/globals.css` are organized so each concern re-skins independently:

1. **shadcn preset tokens** (`:root` + `.dark`) — the standard shadcn role
   tokens (`--background`, `--foreground`, `--primary`, `--card`, `--muted`,
   `--border`, `--sidebar*`, `--chart-*`, `--radius`, …). This block is
   **owned by the shadcn preset** and is rewritten wholesale when the preset
   is re-applied, so no project-specific overrides live here. Current base
   preset: **`b1HYEHloH`** (stone base, near-white `--primary`).
2. **Codemux runtime theme namespace** (`--cm-theme-*`) — `themes.ts`
   validates and writes all 26 role values and all 16 ANSI values atomically.
   A root `[data-theme-id]` bridge aliases the product roles into shadcn and
   Codemux variables. Components keep using semantic utilities while a theme
   can replace the full application palette.
3. **CodeMux custom tokens** (`:root, .dark` block) — product meanings the
   shadcn preset cannot express. Status colors remain stable semantic states;
   brand and surface roles are fed by the active theme:
   - `--accent-ember` — compatibility name for the active theme's brand accent
     (selection, links, small accents).
   - `--selection-background` / `--selection-foreground` — the stable DOM
     text-selection pair. The background aliases ember; the foreground is a
     dedicated dark ink that remains above 6:1 contrast across every palette,
     including the transparent Agent Chat composer textarea.
   - `--accent-violet` — secondary accent (for example, a merged PR).
   - `--status-open` (green), `--status-working` (amber),
     `--status-attention` (red), `--status-monitoring` (cyan),
     `--status-remote` (sky) — status tones, **identical across all
     palettes**. `--status-monitoring` marks an agent watching something in
     the background (see `docs/features/monitoring-status.md`); it is cool and
     low-chroma on purpose, held apart in hue from the sky `--status-remote`
     so a monitoring dot and a remote-host chip never read as the same thing.
     **Nothing that carries it animates** — the pulse is reserved for
     `--status-attention`, the one state genuinely blocked on a human.
   - Legacy aliases `--success`/`--warning`/`--danger` map onto the status
     tokens as a single value source for existing `text-success` etc.
   - `--sidebar-primary` is the active theme's sidebar accent and terminal cursor.
   - `--sidebar` is its own role so it can recede from the background without
     component-local color math.
4. **Radius scale** (`@theme inline`) — `--radius-sm..4xl` derive from the
   preset's `--radius`, so every rounded utility re-skins from one value.

## Application Themes

Managed built-ins are Graphite (`default`), Warm Stone, Ember, Abyss, and
Iris. Custom themes use the same complete role/ANSI schema. The active root is
identified by `data-theme-id`; the former `.theme-warm` class is legacy-only
and is migrated into the synced Warm Stone theme selection.

Theme switching adds `.no-transitions`, writes the complete namespace, updates
the boot shadow, and removes the guard after paint. `index.html` validates and
applies that shadow before CSS and React to avoid a Graphite flash. Incomplete
or invalid payloads fail closed to the built-in default.

**A preview is the same write minus the boot shadow.** The command palette's
theme picker calls `applyTheme(theme, { persist: false })` on the highlighted
row, so a preview repaints every consumer of the namespace — chrome, diff
tints, editor, terminal ANSI — while the persisted shadow keeps pointing at
the real choice. Any surface that previews a theme owes a revert on close;
`persist: false` is what makes the revert cheap and a crash mid-preview
harmless.

Radius, fonts, spacing, and the success/info status hues are **not** part of a
theme. A palette may not change the geometry or the meaning of a status color.

See `docs/features/theming.md` for generation, import/export, sync, and syntax
integration, and `docs/features/command-palette.md` for the picker.

## Density Scale

Applied via the root `data-density` attribute; drives card padding, grid
gaps, and group rhythm on density-aware surfaces (e.g. the Workspaces
overview):

- **Comfortable** (default): `--cpad: 15px`, `--cgap: 13px`, `--rowgap: 20px`.
- **Compact** (`[data-density="compact"]`): `--cpad: 10px`, `--cgap: 8px`,
  `--rowgap: 13px`.

## Text Selection

### Selectability: chrome is not selectable, content is

Codemux follows the desktop-app convention: drag-selection only ever grabs
real content, never UI chrome. Selectable-everything is a website default —
being able to rubber-band sidebar cards, settings labels, buttons, and layout
whitespace makes the app read as a web page. The document therefore opts out
once at the root (`body { user-select: none; cursor: default }` in
`src/globals.css` `@layer base`, with the `-webkit-` prefix WebKitGTK
requires) and content opts back in.

Two opt-in tiers, both in the same `@layer base` block:

1. **Automatic element/root selectors** — `input`, `textarea`,
   `[contenteditable]`, `pre`, `code`, `kbd`, `samp`, `.cm-content`,
   `.chat-markdown`, `.markdown-rendered`, and sonner toast title/description.
   Because `user-select` inherits, a rule on a content root re-enables its
   whole subtree, so most new content stays selectable without remembering a
   class.
2. **The `select-text` utility** for one-off prose/mono blocks that have no
   shared root (user message text, reasoning text, tool-output divs, diff pane
   content, review-thread comments). A base-layer companion rule gives
   `.select-text` elements `cursor: auto` so re-enabled text gets its I-beam
   back.

Rules of thumb:

- New chrome needs no class — it is unselectable by default.
- New content surfaces should either render under an existing content root or
  carry `select-text`.
- Chrome *inside* a content root (a code-block header bar, a copy button)
  opts out again with `select-none`; Tailwind utilities beat the base-layer
  opt-ins, so precedence works in both directions.
- Never re-disable selection on error text, paths, IDs, or anything a user
  might legitimately copy. Rendered diffs, file-path headers, grep matches,
  ssh targets, and inline error/notice lines all count as content.
- `cursor` inherits from the body opt-out too, so a base-layer `a { cursor:
  pointer }` keeps links clickable-looking; any other custom cursor still
  needs its own rule or utility.

### Highlight colors

Selectable Codemux-owned DOM text inherits one document-level highlight
contract from `:root::selection`: solid `--selection-background` with
`--selection-foreground`. Keep the selector root-scoped rather than using a
bare `::selection`; the CSS highlight inheritance model then provides a
predictable app default while preserving deliberate descendant overrides. That
inheritance is what makes a single rule reach the whole tree — on an engine
still using the legacy `::selection` matching model the rule simply matches
nothing below the root and selection falls back to the native highlight, which
is a cosmetic downgrade, not a broken surface. Both shipped WebViews
(WebKitGTK, WebView2) implement the inheritance model.

Always set the selection foreground and background together. Reusing the
active palette's `--foreground` makes the dark themes' near-white text fail
contrast against ember and leaves the layered Agent Chat composer's
transparent textarea glyphs without a reliable selected-text color.

This contract covers Codemux-owned DOM surfaces, including prose, links,
inline code, inputs, and textareas. Pages displayed inside the browser pane are
separate documents and own their styling.

Renderer-owned selections need explicit handling, because inheritance reaches
them too:

- **xterm.js** never uses the native highlight — `.xterm` is `user-select:
  none` and selection is painted by the renderer from the terminal theme's
  `selectionBackground`. Nothing to opt out of.
- **CodeMirror** paints its own selection layer (`.cm-selectionBackground`,
  from `--accent`) and suppresses the native highlight by resetting only its
  *background*. The foreground still inherits, so `src/lib/codemirror-theme.ts`
  resets `.cm-line` selected text to `currentColor` — selected code keeps its
  syntax colors instead of rendering as dark selection ink on the accent fill.
  The opt-out is scoped to document lines on purpose: the editor's own chrome
  (search panel inputs, tooltips) uses real native selection and should keep
  the app-wide pair.

Any future surface that draws its own selection layer needs the same
`currentColor` opt-out for the text it covers.

## Focus Treatment

Small controls signal focus with a ring or a border change. **Large surfaces do
not.** On a big rounded rectangle — the Agent Chat composer is the canonical
case — brightening the border draws a bright wireframe around the largest
element on screen, which reads as an error state rather than as focus.

The rule: for a large container, keep the resting and focused **border
identical** and carry focus with **surface tint plus elevation**. The composer
(`src/components/chat/Composer.tsx`) does this with
`focus-within:bg-muted/60` and a soft drop shadow, holding its
`border-border/80` constant across both states. The perceptual cue is the card
lifting toward the reader, not an outline switching on.

This does not relax the accessibility floor. It applies to the *container*
chrome only — the focusable control inside it keeps whatever visible focus
indicator it already had, and keyboard focus on buttons, inputs, and menu items
continues to use the standard ring. Do not extend the tint-and-elevation
treatment to controls small enough for a ring to read cleanly.

## Typography

- `--font-sans` — **DM Sans Variable** (UI + headings; `--font-heading`
  aliases it — headings are sans, not mono).
- `--font-mono` — **JetBrains Mono Variable**, reserved for code-like
  metadata only (branch names, counters, ids).
- UI font sizes use whole CSS pixels. Keep the compact scale on
  `10px` / `11px` / `12px` / `13px` / `14px`; half-pixel font sizes make
  glyphs noticeably softer in the Linux WebKitGTK desktop renderer.
- Primary chat prose and composer input use `text-sm leading-relaxed`
  (14px with a 1.625 line height). Supporting labels may be smaller, but
  should not compress the main reading surface.
- Font rasterization is platform-native. Do not apply global
  `-webkit-font-smoothing`, `-moz-osx-font-smoothing`, or `text-rendering`
  overrides: they thin DM Sans in WebKitGTK and render differently from the
  Chromium dev mock.

## Notes

- `.thin-scrollbar` (in `globals.css`) is the shared opt-in for overlay lists
  tall enough to always show a scrollbar — the command palette's result list.
  Thin, token-colored, transparent track. Prefer it over hiding the scrollbar
  (`no-scrollbar`) when the user needs to see how much list is left.
- Streamdown's compiled utility classes are registered as a Tailwind scan
  source (`@source "../node_modules/streamdown/dist/*.js"`) so the streaming
  markdown renderer's classes survive the Tailwind v4 tree-shake.
- Chat code blocks are syntax-highlighted by Shiki, which inlines concrete
  colors into `style` attributes and so cannot consume CSS variables. Their
  token colors are derived from the terminal ANSI palette
  (`src/lib/shiki-chat-theme.ts`) — the same documented ANSI exception that
  covers `TerminalPane` and the CodeMirror editor theme. `ChatCodeBlock.tsx`
  owns the file-aware header and wrap/copy controls; structural chrome
  (container, header, borders, inline-code pill) stays on design tokens under
  `.chat-markdown` in `globals.css`.
