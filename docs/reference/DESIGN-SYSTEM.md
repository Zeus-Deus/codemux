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

## Typography

- `--font-sans` — **DM Sans Variable** (UI + headings; `--font-heading`
  aliases it — headings are sans, not mono).
- `--font-mono` — **JetBrains Mono Variable**, reserved for code-like
  metadata only (branch names, counters, ids).

## Notes

- OpenFlow role palette stays distinct via tokens (e.g. debugger →
  `accent-ember`).
- Streamdown's compiled utility classes are registered as a Tailwind scan
  source (`@source "../node_modules/streamdown/dist/*.js"`) so the streaming
  markdown renderer's classes survive the Tailwind v4 tree-shake.
