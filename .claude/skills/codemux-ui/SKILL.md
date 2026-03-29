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

Reference products for the visual feel: Superset (superset.sh), cmux (cmux.com), Conductor (conductor.build). These apps use neutral dark backgrounds with small, intentional color accents.

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
