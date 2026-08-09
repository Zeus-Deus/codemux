# Application Theming

- Purpose: Describe Codemux's runtime application-theme system, custom-theme workflow, and editor/terminal integration.
- Audience: Anyone changing appearance settings, color roles, syntax colors, or theme persistence.
- Authority: Canonical feature-level reality doc.
- Update when: Theme roles, built-ins, import/export, generation, persistence, or syntax consumers change.
- Read next: `docs/reference/DESIGN-SYSTEM.md`, `docs/features/command-palette.md`, `docs/features/settings.md`, `docs/features/settings-sync.md`

## What This Feature Is

Codemux has one dark application-theme model that colors the full product: React surfaces, sidebar and chrome, CodeMirror, Shiki chat blocks, and xterm. Five built-in themes ship with the app, and users can generate or import custom themes.

**Themes are picked in the command palette**, not on a settings page: ⌘K → "theme" lists them, and highlighting a row repaints the whole app so the choice is made by looking at the product rather than at a swatch grid. Settings → Appearance keeps a single row stating what is on.

## Current Model

`src/lib/themes.ts` owns the schema and engine. A theme contains 26 semantic product roles, a complete 16-slot ANSI palette, metadata, and an optional radius. Managed built-ins are Graphite (`default`), Warm Stone, Ember, Abyss, and Iris.

Applying a theme writes namespaced `--cm-theme-*` variables and a `data-theme-id` attribute to the root in one transition-suppressed operation. `src/globals.css` bridges those variables into the existing shadcn and Codemux tokens, so component code continues consuming semantic utilities. An inline `index.html` boot script applies the last validated theme shadow before CSS and React load; invalid or incomplete data falls back to Graphite.

The active theme and custom theme payloads are synced settings. A machine-local terminal preference can choose either the app palette or the operating-system terminal palette. One module-level syntax-theme store feeds CodeMirror, Shiki, and xterm, avoiding duplicate native theme listeners.

### Activation waits for the stores

`app-shell.tsx` applies the synced theme only once **both** settings stores have
loaded. Before that, the synced store still holds its `DEFAULT_SETTINGS`
placeholder (`appearance.theme: "default"`), and applying it would both repaint
away from what the boot script just painted and — since `applyTheme` persists by
default — overwrite the boot shadow with Graphite, so a crash or a failed load
during that window would carry the wrong theme into the next launch. Doing
nothing is correct: the boot script has already painted. Density is a layout
attribute rather than a color and is written unconditionally.

The legacy machine-local Cool/Warm axis migrates on the same gate, and exactly
once: an explicitly stored `appearance.palette: "warm"` is mapped to the synced
`warm` theme, and the local key is rewritten to `cool` as soon as that write
lands. Retiring the key is what ends the migration — it also retires the
`default → warm` display mapping, so a legacy-warm user who later picks Graphite
keeps it instead of being rewritten back to Warm Stone on every render.

### Where the UI lives

- **Picking** — the command palette's Themes group. Highlight previews
  (`applyTheme(theme, { persist: false })`), Enter promotes and writes the
  synced `appearance.theme` field, Esc/close reverts. Search-only, so the
  resting palette stays a "where was I?" list. Full behavior in
  `docs/features/command-palette.md` § "The theme picker".
- **Stating** — `theme-settings.tsx`, the one row that replaced the swatch
  grid in Settings → Appearance's Theme subsection. Everything else on that
  page is untouched.
  **Change** reopens the palette on the theme query (leaving Settings, since
  Settings is a full-screen surface the palette does not mount inside — and
  previewing against a settings page would prove nothing). **Customize** opens
  the studio on the current theme, or starts a new one when a built-in is
  applied.
- **Creating / importing / role-editing** — `theme-studio.tsx`, an
  **app-level modal** driven by `useUIStore.themeStudio` and mounted in
  `App.tsx`. It is not a child of the Appearance page: its doors (the
  palette's last two rows, Appearance's Customize) live on surfaces that
  replace each other. See § The studio below.

## The studio

A centered 1000px modal over whatever you were on, with two tabs —
**Generate** and **Import** — a 330px control column, and a live preview
occupying the rest.

Three properties are deliberate:

- **The surface behind stays mounted.** Opened from Settings ▸ Appearance,
  Esc returns you to Appearance, not to the home screen. `SettingsView`
  mounts its own `CommandPalette` for the same reason, and
  `openCommandPaletteWith` no longer closes Settings — so Change → palette →
  "Make a theme…" keeps you on the page the whole way.
- **It does not touch the running app.** The candidate is painted by
  `ThemePreviewShell` — a miniature app with a sidebar, a chat exchange, a
  diff card and a terminal strip, every color an inline style read off the
  candidate. Cancelling therefore has nothing to undo, and the preview shows
  the surfaces a palette is actually judged on. (The *palette* picker still
  previews against the real app; that is turn 3's argument and is unchanged.)
- **Role editing is a link, not a third tab.** As a peer of Generate and
  Import it turned a two-way choice into a three-way one. A theme with no
  seeds — imported or hand-edited — reopens directly in it, because there is
  nothing else to reopen.

### Generate

Two seeds, Background and Accent. The column shows the solved role swatches,
the token count, and the **contrast it actually achieved** (body text and
accent-on-surface), so the generator's claim is checkable rather than a
promise.

Surfaces and text are derived from the **background's** hue; only genuinely
accent-colored roles take the accent's, and surface chroma is bounded by the
canvas's own so a neutral grey background yields a neutral grey ramp. The two
seeds are "the room" and "the highlight" — deriving every surface from the
accent hue meant a blue-black canvas with a warm accent produced a *brown*
app, with the chosen background surviving only in the role literally named
`background`.

### Import

Three named sources, because "import a theme" is a question about a file and
"where is it from?" is one you can answer:

- **VS Code** — search the Marketplace by name. `vscode_marketplace_search`
  queries the public gallery `extensionquery` endpoint; picking a result runs
  `vscode_marketplace_fetch_themes`, which streams the `.vsix` (a ZIP, capped
  at 60 MB), reads `extension/package.json`, and returns the raw JSONC of
  every **dark** theme it contributes. A single dark variant is taken
  outright; two or more are listed. Nobody opens a `.vsix`.
- **shadcn / Tailwind** — paste the `:root`/`.dark` block.
- **A .codemux-theme file** — paste or drop it.

Whatever the source, the text goes through one parser. There is **no Parse
button**: `importThemeDetailed` runs on every change and returns the format,
the roles the source carried, and the roles Codemux had to solve. The panel
states the first two as a sentence and *lists* the third — "22 of 26" doesn't
tell you which four are ours, and those are the ones worth a look before
applying.

The VS Code accent heuristic is ordered by how deliberately a theme author
picks each token, with `focusBorder` **last**: it is very often a translucent
grey (Tokyo Night's is `#545c7e33`), which flattened over a dark canvas
produced a mud accent for a theme everyone recognises as blue. A candidate
that can't clear 3:1 against the background is skipped whatever its rank.

## What Works Today

- Palette theme selection with live full-app preview on the highlighted row, Enter-to-keep, Esc-to-revert, and synced persistence.
- Two-color custom generation from background and accent, using OKLCH gamut mapping and minimum contrast solving, with surfaces held in the background's hue family.
- VS Code Marketplace search and `.vsix` theme extraction, dark variants only.
- Codemux versioned JSON import/export with forward-safe parsing and completion of missing roles.
- shadcn CSS-variable import, including optional radius.
- VS Code JSON/JSONC import for workbench colors and complete eight-base-color ANSI palettes; bright slots are imported or derived. Token scopes are intentionally ignored.
- Paste and local-file import. Imported alpha colors are flattened over the resolved theme background because the runtime stores opaque colors.
- Custom-theme rename-by-label, live preview, and save/apply from the studio; export and delete sit in the studio footer and appear only once a theme exists on disk. Deleting the applied theme falls back to Graphite. Generated themes reopen from their original seeds; imported and hand-edited themes reopen in a complete role-and-ANSI editor.
- App-theme parity across DOM chrome, terminal ANSI, editor syntax, and chat syntax.
- A source guard rejects raw Tailwind palette utilities in production components.

## Current Constraints

- Application themes are dark-only. Light-theme generation is rejected explicitly.
- The only online source is the VS Code Marketplace; there is no Codemux theme marketplace and no URL installer. Pasting a Marketplace *link* is not supported — search by name instead.
- Import deliberately maps semantic workbench colors, not VS Code `tokenColors`; Codemux owns one syntax mapping across all code surfaces.
- Custom themes are embedded in the synced settings blob rather than stored as separate server objects.

## Important Touch Points

- `src/lib/themes.ts` — schema, built-ins, generation, import/export, runtime application, boot shadow
- `src/components/overlays/command-palette.tsx` — the picker, its preview/commit/revert wiring
- `src/components/overlays/command-palette-model.ts` — `rankThemeGroup`, `themeRowValue`, `previewedThemeId`
- `src/components/settings/theme-settings.tsx` — the Appearance theme row
- `src/components/settings/theme-studio.tsx` — the Generate / Import modal
- `src/components/settings/theme-preview-shell.tsx` — the in-modal miniature app
- `src/components/settings/theme-import-sources.tsx` — the three named sources
- `src/components/settings/theme-marketplace.tsx` — Marketplace search + variant picker
- `src-tauri/src/vscode_marketplace.rs` — gallery query + `.vsix` extraction
- `src/components/settings/theme-swatches.tsx` — shared miniature, coins, ANSI dots
- `src/hooks/use-theme-colors.ts` — shared app/system syntax-theme store
- `src/lib/xterm-theme.ts` — terminal adapter
- `src/components/layout/app-shell.tsx` — synced theme activation and legacy warm-palette migration
- `index.html` — no-flash pre-React theme boot
- `src/globals.css` — runtime role bridge
- `src/stores/synced-settings-store.ts` and `src-tauri/src/settings_sync.rs` — synced schema
- `src/lib/themes.test.ts` and `src/lib/theme-color-contract.test.ts` — engine and design-contract tests
- `src/components/overlays/command-palette.test.tsx` — the preview / keep / revert contract

## Notes

- Add new visual meaning as a semantic role; do not reach around the theme with a component-local color.
- Built-ins and generated themes must always contain every role and ANSI slot.
