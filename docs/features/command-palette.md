# Command Palette

- Purpose: Describe the command palette — its result model, search rules, and available actions.
- Audience: Anyone adding commands or working on keyboard shortcuts.
- Authority: Canonical feature-level reality doc.
- Update when: Groups, ranking rules, or dispatch behavior changes.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/features/search.md`, `docs/features/sidebar.md`

## What This Feature Is

The command palette (Ctrl+K) is the app's one keyboard entry point. It is a
**unified switcher**, not just an action menu: one query searches workspaces,
projects, and commands together, and the resting (unsearched) list answers
"where was I?" rather than listing an alphabetical index.

Built from the `Command Palette.dc.html` design handoff.

## Current Model

Three groups, always in this order, each with a sticky header carrying a match
count:

1. **Workspaces** — avatar, title, mono sub-label, live status or idle age.
2. **Projects** — avatar, name, `~`-contracted path, active-workspace count.
3. **Commands** — glyph, label, resolved keybind hint.

The palette owns its filtering (`shouldFilter={false}` on cmdk) so the rules
below are the app's, not the library's. All of them live in
`command-palette-model.ts` and are unit-tested there.

### Search rules

- **Fuzzy, scored, one haystack per candidate** — via the shared
  `@/lib/fuzzy`, the same matcher the Thread Scope pickers use.
- **`/` switches to path mode.** A query with no separator matches names
  (workspace title + branch, project name); a query containing `/` matches
  **locations only** (`workspacePathText` — project root, worktree path, cwd;
  project path for a project row). Folding names and paths into one haystack
  is deliberately avoided in both directions: subsequence matching over long
  paths matches nearly everything, and a path haystack carrying the title
  lets an unrelated project's *name* satisfy a location query and outrank the
  real answer.
- **`>` switches to command mode.** Typing `>` first hides workspaces and
  projects, shows a `Commands` chip in the input row, and searches only
  commands (label + hidden `keywords` synonyms).
- Settled and snoozed workspaces stay indexed — they are live workspace
  records; parking is visual only.

### Resting order and caps

With an empty query, workspaces sort by `compareWorkspaceOrder`: **needs-you >
working > review > idle**, then most-recent activity, with parked (settled /
snoozed) work last. The list is capped (8 workspaces / 4 projects at rest, 24 /
8 while searching); a capped group header reads `8 of 77` rather than silently
under-reporting.

### Commands

A command is `{ label, icon, keywords?, actionId?, run?, requiresWorkspace? }`.
When it carries an `actionId`, it routes through the **same `dispatch()` the
global keyboard handler uses** — so a shortcut and its palette row can never
drift apart, and the keybind hint comes from the same registry entry. `run` is
only for actions with no keybind (Toggle preset bar, Open browser, Focus
next/previous pane, Regenerate MCP config). `requiresWorkspace` rows are hidden
outright when nothing is active, instead of being shown as silent no-ops.

Current catalogue: New agent · New workspace in this project · Create new
workspace · Open project… · Run dev command · Next/Previous workspace · Find
file by name · Search in files · Split pane right/down · Close pane · New
terminal tab · Close tab · Focus next/previous pane · Open browser · Toggle
sidebar · Toggle right panel · Toggle preset bar · Keyboard shortcuts ·
Settings · Regenerate MCP config.

### Selection behavior

- **Workspace** — clears the active chat draft, then activates (mirrors the
  inbox card, so a draft surface can't override the jump).
- **Project** — scopes the sidebar inbox filter to that project, then lands on
  its top-ranked workspace. "Go to this project", one keystroke.
- **Command** — dispatches, then closes.

## What Works Today

- Fuzzy scored matching with `>` command mode and `/` path mode
- Live status dots + labels (Working / Needs you / Done · review) from the
  shared `pane-status` maps, idle age from the sidebar's `formatElapsed`
- Project avatars (custom color + image) via the shared appearance store
- Per-row keybind hints, swapped for a `↵` badge on the selected row
- Sticky group headers with honest counts, a footer hint bar and result count
- Empty state naming the failed query and pointing at `>`
- Query changes scroll the list back to the top (cmdk only auto-scrolls on
  arrow navigation)

## Current Constraints

- No user-defined custom commands
- No recently-used / frecency ranking — resting order is status + activity
- No nested submenus within groups
- Project rows come from grouped workspaces, so a project with no workspaces
  is not listed

## Important Touch Points

- `src/components/overlays/command-palette.tsx` — palette UI + command catalogue
- `src/components/overlays/command-palette-model.ts` — pure query/rank/label model
- `src/components/overlays/command-palette-model.test.ts` — its unit tests
- `src/lib/fuzzy.ts` — shared scorer
- `src/lib/pane-status.ts` — `STATUS_LABEL` / `STATUS_TEXT_CLASS` / `STATUS_DOT_CLASS` / `statusRank`
- `src/lib/shorten-path.ts` — `~` contraction shared with the workspace hover card
- `src/hooks/use-keyboard-shortcuts.ts` — `dispatch()`, the shared action runner
- `src/hooks/use-resolved-keybinds.ts`, `src/lib/keybind-registry.ts` — keybind hints
- `src/stores/ui-store.ts` — palette open/close state
