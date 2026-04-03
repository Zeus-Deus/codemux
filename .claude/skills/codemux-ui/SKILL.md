---
name: codemux-ui
description: Use when building, modifying, or improving any user-visible part of Codemux — React components, Tailwind, shadcn, theming, pane layouts, sidebar, notifications, OpenFlow UI, or implementing new ADE features. Also use when the task involves visual design decisions, component patterns, or color/theme work.
---

# Codemux UI & ADE Feature Standards

Standards and patterns for building excellent UI in Codemux. This file is about HOW to build things well. For WHAT the project is and WHERE things currently stand, always read `WORKFLOW.md` and `docs/INDEX.md` first.

Apply these standards to new code and incremental improvements. Do not mass-refactor existing components to match unless explicitly asked.

---

## Design Philosophy

Codemux is a Linux-first ADE (Agentic Development Environment). The UI wraps terminal panes where CLI agents do the real work. The shell (sidebar, pane headers, status indicators, notifications) should feel professional, clean, and calm — like a well-designed control room.

Reference products for the visual feel: modern ADE terminals and developer tools. These apps use neutral dark backgrounds with small, intentional color accents.

Principles:
- **neutral base, accent pops** — the app shell is always dark neutral; theme colors appear only in small, intentional places
- **information density without clutter** — show useful data (agent status, git branch, task progress) but keep visual noise low
- **keyboard-first** — every action reachable by keybind; mouse supported but not required
- **progressive disclosure** — hide secondary actions behind hover or expand; show primary state at a glance
- **the terminal is the star** — everything around the terminal serves it, not the other way around
- **right panel for auxiliary views** — file tree, changes/diff, and other secondary views live in the collapsible right sidebar panel. The main workspace area is reserved for terminals and browser panes. New auxiliary views should be added as tabs in the right panel, not as separate panels.

---

## Theming Rules

### shadcn Preset Color Model

The frontend uses shadcn with preset `b3kIbNYVW` (zinc base, oklch color space, 0.45rem radius). All colors are defined as CSS variables in `src/globals.css` with `:root` (light) and `.dark` selectors. The app defaults to dark mode via `class="dark"` on `<html>`.

#### Core shadcn tokens (from globals.css)

```
--background    App background (oklch dark)
--foreground    Primary text
--card          Elevated surface
--muted         Muted surface
--muted-foreground  Secondary text
--accent        Interactive surface
--primary       Accent color (blue)
--destructive   Error/delete
--border        Border color
```

#### Custom semantic tokens (added for Codemux)

```
--success       Git additions, agent done (green)
--danger        Git deletions, errors (red)
--warning       Notifications, attention (amber)
```

Use Tailwind classes: `text-success`, `text-danger`, `text-warning`, `bg-success`, etc.

#### Terminal Colors

Terminal bg/fg/cursor/selection read dynamically from shadcn CSS variables via `buildThemeFromCSS()` in `TerminalPane.tsx`. ANSI colors (color0-color15) use a static palette. A MutationObserver on `<html>` re-applies the theme when the preset changes.

Omarchy theme integration is commented out but preserved for future "system theme" setting.

### Where Accent Colors Appear

USE `primary` for: focused pane border, active workspace left bar, active workspace row bg, status badges, interactive hover states, notification badges, focused input borders.

NEVER use `primary` for: sidebar background, pane header background, large surface areas, body text, borders in resting state.

### The Golden Rule

All colors come from shadcn CSS variables. Never hardcode hex values in components. Use Tailwind semantic classes (`bg-background`, `text-foreground`, `border-border`, `text-success`, `text-danger`, `text-warning`). The only hardcoded colors allowed are in `src/tauri/types.ts` (data constants from Rust) and the ANSI palette in `TerminalPane.tsx`.

---

## Component Visual Standards

### Pane Headers

- Height: 28-34px, compact. The terminal needs the space.
- Background: `color-mix(in srgb, var(--ui-layer-1) 80%, transparent 20%)` inactive, add ~5% accent tint for active.
- Prefer showing contextual info (agent name, git branch, working directory) over generic labels when data is available.
- Action buttons: hidden by default, visible on hover with slide-in transition. Exception: close button on browser panes can stay visible.
- Typography: title in `--ui-text-primary` at 0.8rem, subtitle in `--ui-text-muted` at 0.7rem.

### Sidebar

- Background: `var(--ui-layer-1)`.
- Workspace rows: active gets accent left bar (3px) + accent bg tint (~10%). Inactive rows are flat.
- Section headers (ALERTS, OPENFLOW, MEMORY): uppercase, `--ui-text-secondary`, 0.76rem, 600 weight, letter-spacing 0.04em. Chevron for collapse.
- Dividers: 1px `var(--ui-border-soft)`.

### Status Indicators

Consistent system for all agent and run states:

| State | Token | Visual |
|-------|-------|--------|
| Running / Active | `--ui-accent` | Filled dot, subtle pulse animation |
| Success / Done | `--ui-success` | Filled dot, static |
| Needs Attention | `--ui-attention` | Filled dot + count badge |
| Error / Failed | `--ui-danger` | Filled dot, static |
| Idle / Waiting | `--ui-text-muted` | Hollow or dim filled dot |

Dot size: 6px sidebar items, 8px dashboard/graph. Count badges: 18px pill, dark text on colored bg.

### Badges and Pills

```css
/* Standard badge pattern */
padding: 2px 8px;
border-radius: 10px;
font-size: 0.65-0.75rem;
font-weight: 600;
text-transform: uppercase;
letter-spacing: 0.03em;
background: color-mix(in srgb, var(--status-color) 15%, transparent);
border: 1px solid color-mix(in srgb, var(--status-color) 30%, transparent);
color: var(--status-color);
```

### Buttons

- Primary: solid accent background, white text, subtle shadow. Hover: lift + deeper shadow.
- Secondary: `var(--ui-layer-2)` bg, `--ui-text-primary` text, soft border. Hover: layer-3 + slight lift.
- Danger: secondary style, hover reveals danger color on text and border.
- Icon: transparent bg, `--ui-text-muted` icon. Hover: layer-2 bg + primary text.

#### Overlay Button Color Rule

- Buttons inside overlays, wizards, splash screens, dialogs, and onboarding flows use neutral inverted styling: `bg-foreground text-background hover:bg-foreground/90`
- This keeps overlays calm and professional regardless of which shadcn preset is active, since `--foreground` and `--background` are always neutral in every preset
- Buttons in the main app chrome (sidebar, preset bar, pane headers) may use `bg-primary` for accent emphasis
- Secondary/cancel buttons: `variant="outline"` or `variant="ghost"`
- Destructive buttons: `variant="destructive"`
- Never hardcode hex colors on buttons — always use shadcn CSS variables via variants or semantic classes

### List & Row Interactions

These principles apply to any list-like UI: sidebar rows, file lists, branch pickers, port pills, commit lists, settings rows. Apply to new components and incremental improvements.

#### Hover-Reveal Actions

The default interaction model for list rows in Codemux. At rest, a row shows data. On hover, secondary actions appear — optionally replacing less-important metadata.

Two variants exist in the codebase:

**Opacity swap** — the most common pattern. Metadata and action occupy the same space via CSS Grid or absolute positioning. Used in sidebar workspace rows, changes panel file rows, tab close buttons, pane header actions, port pills.

```tsx
{/* Container needs `group` (or `group/{name}` for nested groups) */}
<div className="group flex items-center ...">
  {/* Metadata: fades out */}
  <span className="transition-opacity group-hover:opacity-0">+42 −3</span>
  
  {/* Actions: fades in, overlaid in the same grid cell */}
  <Button className="opacity-0 group-hover:opacity-100 transition-opacity">
    <X className="h-3.5 w-3.5" />
  </Button>
</div>
```

**Show/hide** — used when the action buttons have different width from the metadata they replace. The timestamp disappears entirely and action buttons take its space. Used in branch picker rows.

```tsx
<div className="group/row flex items-center ...">
  <span className="group-hover/row:hidden text-muted-foreground">4d</span>
  <span className="hidden group-hover/row:flex gap-1">
    <button>Open ↵</button>
  </span>
</div>
```

Use named groups (`group/row`, `group/pane`, `group/pill`) when rows are nested inside another `group` container to avoid hover conflicts.

**When NOT to hover-reveal:** Primary actions that users need to see at all times — dialog CTAs, always-visible close buttons, status toggles. Hover-reveal is for secondary actions on items in a list.

#### Row Density

Row heights across the app:

| Context | Height | Padding |
|---------|--------|---------|
| Sidebar workspace rows | `py-1.5` (~36px with content) | `pl-3 pr-2` |
| Changes panel file rows | `py-1` (~28-30px) | `px-1.5` |
| Picker items (branch, project) | `h-9` (36px) | `px-2` |
| Tab bar tabs | `h-8` (32px) | `px-2.5` |

The principle: generous enough to click comfortably and accommodate icons + metadata, dense enough that 10-15 items are visible without scrolling. Picker rows and sidebar rows are on the generous end (~36px). File lists and inline controls are denser (~28-32px). Match the density of the nearest existing component rather than inventing a new row height.

#### Row Anatomy

A consistent left-to-right layout used across all list rows:

1. **Icon** (left, fixed-width) — `size-3.5` to `size-4`, `text-muted-foreground`. Represents item type or state, not decoration. Subtle — supports the label, doesn't replace it.
2. **Label** (fills remaining space) — `flex-1 min-w-0 truncate`. Use `font-mono` for code-related items (branches, file paths, ports). Regular font for everything else. `text-xs` to `text-sm` depending on density.
3. **Inline badges** (optional, after label) — small metadata pills for categorical info (status, type). `text-[9px]` to `text-[10px]`, muted styling. These stay visible on hover.
4. **Right-aligned metadata** — timestamps, counts, diff stats. `text-muted-foreground`, `text-[10px]` to `text-xs`, `tabular-nums` for numbers. Visible at rest, may be swapped on hover.
5. **Right-aligned actions** — buttons that appear on hover, replacing or overlaying the metadata. `text-muted-foreground hover:text-foreground`, small icon buttons or text buttons with keyboard hints.

#### Empty States

Every list needs one. Keep it minimal:

```tsx
<div className="py-6 text-center text-sm text-muted-foreground">
  No matching branches
</div>
```

Short message, centered, `text-muted-foreground`. No walls of text, no illustrations. If there's a clear call-to-action (e.g., "Create a workspace to start"), add it as a single line below the message.

### Motion

```
--ui-motion-fast:  120ms ease-out   /* hover, button feedback */
--ui-motion-base:  160ms ease-out   /* panel reveals, focus */
--ui-motion-slow:  240ms ease-out   /* view transitions, drawers */
```

Subtle always. No bounces, springs, or attention-seeking motion. `ease-out` for everything.

### Spacing

- Base unit: 4px. All spacings multiples of 4: 4, 8, 12, 16, 20, 24, 32.
- Pane gap: 2px (CSS Grid gap).
- Pane container padding: 2px.
- Pane border-radius: `var(--ui-radius-lg)` (8px).
- Radius tokens: `--ui-radius-sm: 4px`, `--ui-radius-md: 6px`, `--ui-radius-lg: 8px`.
- Compact elements: 8-12px padding. Spacious sections: 16-24px.

### Typography

- Shell chrome: system UI font stack, 0.7-0.9rem.
- Terminal: `--shell-font-family` or monospace fallback, 13px.
- Nothing larger than 1.1rem in the app shell except OpenFlow config panel headers.

---

## Compound Picker Pattern

The default pattern for any selection UI where the user picks from a dynamic list. Use this for branches, projects, issues, models, presets, files — anything with 5+ items or that benefits from search and keyboard nav.

**Do NOT use** `DropdownMenu` or `Select` for lists that could grow beyond ~5 static items. Those components lack search, keyboard navigation, and space for contextual metadata. Use this pattern instead.

### Structure

Always: `Popover` > `Command` (cmdk). This gives arrow-key navigation, Enter to select, Escape to close, and type-to-filter — all for free, with zero custom keyboard handling.

Reference implementations:
- `src/components/overlays/branch-picker.tsx` — tabs, timestamps, hover-reveal action buttons
- `src/components/overlays/project-picker.tsx` — grouped lists, color avatars, footer actions

### Anatomy (top to bottom)

```
Popover
└── PopoverContent (w-[360px]-[420px], p-0)
    └── Command (shouldFilter={false} if you filter manually)
        ├── CommandInput — search, auto-focused, placeholder "Search {things}..."
        ├── Tab bar (optional) — category filter, BELOW search so typing filters immediately
        ├── CommandList (max-h-[340px]-[420px])
        │   ├── CommandEmpty — short message ("No branches match")
        │   └── CommandGroup
        │       └── CommandItem (per row, h-9 to h-10)
        └── Footer actions (optional) — e.g. "Open project", "New project"
```

### Tab Bar (optional)

For filtering into categories (e.g., "All" vs "Worktrees"). Place below search, above the list.

```tsx
<div className="flex items-center gap-0.5 mx-2 mt-1 mb-1 rounded-md bg-muted/40 p-0.5">
  <button
    className={cn(
      "flex-1 px-2 py-1 text-xs rounded-md transition-colors",
      active ? "bg-background text-foreground shadow-sm"
             : "text-muted-foreground hover:text-foreground",
    )}
  >
    All <span className="text-[10px] opacity-60">{count}</span>
  </button>
</div>
```

Each tab shows its item count. When the popover closes, reset the active tab to the default.

### Row Anatomy (left to right)

| Element | Sizing | Notes |
|---------|--------|-------|
| Icon | `size-3.5`, `text-muted-foreground` | Represents item type or state. Lucide icons. Keep subtle. |
| Label | `text-xs`, `truncate`, `flex-1 min-w-0` | Use `font-mono` for code-related items (branches, files, paths). Regular font for everything else. |
| Badges | `text-[9px] px-1 py-0` | Optional metadata pills: "default", "PR", status. Muted styling via `Badge variant="secondary"`. |
| Metadata | `text-[11px] text-muted-foreground/60` | Right-aligned. Timestamps, counts, etc. **Visible by default, hidden on hover.** |
| Actions | `text-[10px] font-medium` | Right-aligned. **Hidden by default, visible on hover.** Replace metadata when shown. |

### Hover-Reveal Pattern

This is how rows stay clean at rest but expose actions on interaction. The row needs a `group/{name}` class:

```tsx
<CommandItem className="h-9 text-xs gap-2 group/row">
  <GitBranch className="size-3.5 text-muted-foreground" />
  <span className="flex-1 min-w-0 truncate font-mono">{name}</span>

  {/* Metadata: visible by default, hidden on hover */}
  <span className="text-[11px] text-muted-foreground/60 group-hover/row:hidden">
    {timestamp}
  </span>

  {/* Actions: hidden by default, visible on hover */}
  <span className="hidden group-hover/row:flex items-center gap-1">
    <button className="rounded px-1.5 py-0.5 text-[10px] font-medium
      text-muted-foreground hover:bg-accent hover:text-accent-foreground">
      Open <kbd className="opacity-50">↵</kbd>
    </button>
  </span>
</CommandItem>
```

### Popover Trigger

Use the same pill-button style across all pickers for visual consistency in the dialog footer:

```tsx
<PopoverTrigger asChild>
  <button className="inline-flex items-center gap-1.5 rounded-full bg-muted/60
    px-2.5 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground">
    <Icon className="h-3 w-3" />
    <span className="max-w-[120px] truncate">{selectedLabel}</span>
    <ChevronDown className="h-2.5 w-2.5 opacity-40" />
  </button>
</PopoverTrigger>
```

### Sizing

- **Popover width:** 340-420px. Wide enough that names don't truncate aggressively.
- **Row height:** `h-9` to `h-10` (36-40px). Generous, not cramped.
- **Max list height:** 340-420px with scroll.

### States

- **Loading:** Spinner + "Loading {things}..." centered in the list area.
- **Empty (search):** `CommandEmpty` with "No {things} match".
- **Empty (tab):** Contextual message, e.g., "No active worktrees".
- **Single item:** Just show the one row. No special treatment needed.

### Keyboard Shortcuts

cmdk provides these automatically — do not reimplement:

| Key | Action |
|-----|--------|
| Arrow Up/Down | Navigate items |
| Enter | Primary action on highlighted row |
| Escape | Close popover (NOT parent dialog) |
| Typing | Filters via search input |

Add these manually when the picker has dual actions:

| Key | Action |
|-----|--------|
| Ctrl+Enter | Secondary action (e.g., "Create" when "Open" is primary) |

### Popover-Inside-Dialog

Pickers often live inside a `Dialog` (e.g., new workspace dialog). This works safely because:
- Radix `Popover` portals to `<body>` by default (z-50), above the Dialog layer
- Click-outside closes the popover but NOT the parent dialog — Radix handles layered dismissal
- No custom z-index or click-outside handling needed

### What NOT to Do

- Don't use `DropdownMenu` or `Select` for dynamic lists — no search, no keyboard nav
- Don't use `Dialog` for pickers — Popover keeps the parent form visible
- Don't show action buttons at rest — hover-reveal keeps rows scannable
- Don't skip `CommandInput` — even short lists benefit from type-to-filter
- Don't build custom keyboard handling — cmdk provides it free
- Don't use `shouldFilter={true}` (cmdk default) when you have tabs or manual filtering — set `shouldFilter={false}` and filter in a `useMemo`

---

## Overlay and Modal Rules

Codemux uses a global overlay manager (src/stores/overlay.ts) that ensures only ONE overlay is visible at a time. Any full-screen or centered overlay (command palette, search dialogs, modals, confirmation dialogs) MUST use this system.

Rules for adding new overlays:
- Register the overlay kind in the OverlayKind type in src/stores/overlay.ts
- Open/close via toggleOverlay(kind) and closeOverlay() — never manage visibility in the component itself
- Render the overlay conditionally in App.tsx based on active overlay state
- All overlays: centered, same z-index (100), same backdrop (rgba(0,0,0,0.4)), same border-radius and shadow
- Escape always closes the active overlay
- Opening a new overlay auto-closes the previous one
- Overlays do NOT stack — there is never more than one visible

This prevents: overlapping dialogs, z-index wars, inconsistent positioning, and escape key conflicts.

---

## ADE Feature Patterns

Patterns for features that Codemux should implement, drawn from what users love in competing ADEs.

### Pane-Level Attention Indicators

When a specific pane's agent needs attention, that pane's border/header should signal it visually. Use the notification data's `pane_id` field. Pane border transitions to `--ui-attention` glow. Clear when user focuses the pane.

### Built-in Diff Review

When an agent finishes work, show a syntax-highlighted diff view inside Codemux. Could be a new pane type (`kind: 'diff'`). Green additions (`--ui-success`), red deletions (`--ui-danger`). Monospace font. Approve/reject actions that map to git operations.

### Best-of-N Comparison

Run the same task on N agents in separate worktrees. When all finish, show a comparison view with each agent's diff against the base. User picks the winner, that worktree merges. Builds on existing worktree isolation.

### Keybind Command Palette

Ctrl+K overlay for fast actions: switch workspace, focus pane, create workspace, split, open browser, start OpenFlow run. Fuzzy-match filter. Centered modal, `var(--ui-layer-2)` bg, strong border and shadow. Keyboard navigation. One of the highest-impact UX features for keyboard-first users.

### Session Persistence

Reopen the app and find everything where you left it. Persist terminal scrollback to disk. Persist OpenFlow comm logs and state. For dead PTY sessions, restore layout and show "session ended — restart?" prompt.

### Notification → Pane Focus

Clicking a notification jumps to the correct workspace AND focuses the specific pane. Flash the pane border briefly on arrival. For Hyprland desktop notifications, deep-link back to the pane after window focus.

### OpenFlow Intelligent Routing

Unique to Codemux. Instead of manual agent assignment, analyze the task and suggest/auto-assign the best agent per role. Complex architecture → stronger model, quick fixes → cheaper model. Always keep manual override available.

---

## CSS Rules

### Use Tailwind + shadcn tokens

```tsx
{/* Correct — use Tailwind semantic classes */}
<div className="bg-card text-muted-foreground border-border rounded-md" />

{/* Correct — use custom semantic tokens */}
<span className="text-success">+42</span>
<span className="text-danger">-17</span>

{/* Wrong — hardcoded Tailwind palette colors */}
<span className="text-green-500">+42</span>
<span className="bg-gray-800" />
```

### No separate CSS files per component

All styling via Tailwind classes. The only CSS file is `src/globals.css` which defines shadcn tokens, the `@theme inline` block, and a few global styles (`.pane-drop-overlay`). No CSS modules, no styled-components.

### Use `cn()` for conditional classes

Import from `@/lib/utils`. Combines `clsx` + `tailwind-merge`.

### Flex/grid overflow prevention

Always set `min-w-0` and `min-h-0` on flex and grid children to prevent content overflow.

---

## New Component Checklist

Before considering a component done:

1. All colors reference `--ui-*` or `--theme-*` tokens, no hardcoded hex/rgba
2. All spacing is a multiple of 4px
3. Transitions use `--ui-motion-*` tokens
4. Interactive elements have hover states
5. Works at narrow widths (small pane splits, collapsed sidebar)
6. Uses the status indicator color system consistently
7. Secondary actions are hidden by default where appropriate
8. Text uses three-level hierarchy (primary/secondary/muted)
9. `min-width: 0` and `min-height: 0` on flex/grid children
10. Clear empty state, loading state, and error state exist

---

## Do Not

- Add separate CSS files per component — use Tailwind classes only
- Hardcode hex/rgba colors in components — use shadcn tokens via Tailwind
- Import UI libraries besides shadcn — use shadcn primitives from `src/components/ui/`
- Use `px` font sizes in shell chrome — use `rem`
- Add attention-seeking animations to the chrome
- Use `--theme-background` or `--theme-foreground` in shell chrome — use `--ui-*` tokens
- Hardcode `rgba()` for accent-derived colors — use `color-mix()` with `var(--ui-accent)` etc.
