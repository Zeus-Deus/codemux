# Design System (tokens & theming)

- Purpose: The durable reference for Codemux's color tokens, theming layers, and the "no hardcoded colors" rule.
- Audience: Anyone doing UI, component, or visual-design work.
- Authority: Canonical theming reference. The runtime source of truth is `src/globals.css`.
- Update when: Token names, the base shadcn preset, palette variants, or the density scale change.
- Read next: `docs/features/settings.md` (appearance), `docs/reference/ARCHITECTURE.md`

> This reference exists because PR #115 tokenized the whole app and removed
> the `.claude/skills/codemux-ui` skill that previously held these rules.
> When in doubt, read `src/globals.css` — its inline comments are authoritative.

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

### Intentionally-hardcoded exceptions

These are **not** theme colors and are left literal on purpose:

- terminal ANSI palettes (`TerminalPane`, `use-terminal-theme-sync`,
  `use-theme-colors`) — xterm's `ITheme` needs concrete colors.
- the project-avatar color-picker hex list + dynamic per-project `rgba()`.
- inspector highlight CSS injected into the foreign inspected page.
- `BrowserPane` canvas `fillStyle` (`#000`).

## Token Layers

`src/globals.css` is organized so each concern re-skins independently:

1. **shadcn preset tokens** (`:root` + `.dark`) — the standard shadcn role
   tokens (`--background`, `--foreground`, `--primary`, `--card`, `--muted`,
   `--border`, `--sidebar*`, `--chart-*`, `--radius`, …). This block is
   **owned by the shadcn preset** and is rewritten wholesale when the preset
   is re-applied, so no project-specific overrides live here. Current base
   preset: **`b1HYEHloH`** (stone base, near-white `--primary`).
2. **CodeMux custom tokens** (`:root, .dark` block) — the brand layer the
   presets can't express. Because these are non-standard variable names,
   switching/re-applying the shadcn preset never touches them:
   - `--accent-ember` — primary accent (selection, links, small accents).
   - `--selection-background` / `--selection-foreground` — the stable DOM
     text-selection pair. The background aliases ember; the foreground is a
     dedicated dark ink that remains above 6:1 contrast across every palette,
     including the transparent Agent Chat composer textarea.
   - `--accent-violet` — secondary accent (PR merged, OpenFlow).
   - `--status-open` (green), `--status-working` (amber),
     `--status-attention` (red), `--status-remote` (sky) — status tones,
     **identical across all palettes**.
   - Legacy aliases `--success`/`--warning`/`--danger` map onto the status
     tokens as a single value source for existing `text-success` etc.
   - `--sidebar-primary` is overridden here to `--accent-ember`: the shadcn
     preset ships a saturated-blue sidebar accent (also the terminal cursor),
     but Codemux has no blue brand color, so it is routed to ember. This
     override survives a preset re-apply.
   - `--sidebar` is overridden per scheme (light/dark/warm) to sit **slightly
     darker than `--background`** — the sidebar recedes so the workspace-inbox
     cards can be flat/transparent at rest with lightness-only selection.
     Like all custom-layer overrides, it survives a preset re-apply.
3. **Radius scale** (`@theme inline`) — `--radius-sm..4xl` derive from the
   preset's `--radius`, so every rounded utility re-skins from one value.

## Palette Variants

- **Default** (`:root` / `.dark`) — stone-based neutral surfaces.
- **`.theme-warm`** — opt-in via a `.theme-warm` class on the root. Overrides
  only the surface/role tokens with warm-grey oklch values; `--primary` stays
  near-white (white buttons) and the status tokens are **not** overridden
  (shared across palettes). One component set, two token maps.

## Density Scale

Applied via the root `data-density` attribute; drives card padding, grid
gaps, and group rhythm on density-aware surfaces (e.g. the Workspaces
overview):

- **Comfortable** (default): `--cpad: 15px`, `--cgap: 13px`, `--rowgap: 20px`.
- **Compact** (`[data-density="compact"]`): `--cpad: 10px`, `--cgap: 8px`,
  `--rowgap: 13px`.

## Text Selection

Selectable Codemux-owned DOM text inherits one document-level highlight
contract from `:root::selection`: solid `--selection-background` with
`--selection-foreground`. Keep the selector root-scoped rather than using a
bare `::selection`; the CSS highlight inheritance model then provides a
predictable app default while preserving deliberate descendant overrides.

Always set the selection foreground and background together. Reusing the
active palette's `--foreground` makes the dark themes' near-white text fail
contrast against ember and leaves the layered Agent Chat composer's
transparent textarea glyphs without a reliable selected-text color.

This contract covers Codemux-owned DOM surfaces, including prose, links,
inline code, inputs, and textareas. Renderer-owned selections remain separate:
xterm.js and CodeMirror keep their own theme integrations, and pages displayed
inside the browser pane own their document styling.

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
- OpenFlow role palette stays distinct via tokens (e.g. debugger →
  `accent-ember`).
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
