---
name: codemux-ui
description: Use when building, modifying, or improving any user-visible part of Codemux — Svelte components, CSS, theming, pane layouts, sidebar, notifications, OpenFlow UI, or implementing new ADE features. Also use when the task involves visual design decisions, component patterns, or color/theme work.
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

---

## Theming Rules

### Shell vs Terminal Color Model

The shell uses a FIXED neutral dark palette. Omarchy theme colors only affect terminal content and accent tokens.

#### Fixed Shell Palette (never changes with theme)

```
--ui-layer-0: #0d0f11          Near-black base (app bg, terminal bg)
--ui-layer-1: #151719          Sidebar, pane headers
--ui-layer-2: #1c1e22          Elevated surfaces, cards, inputs
--ui-layer-3: #252830          Hover states, strong surfaces

--ui-border-soft:   rgba(255, 255, 255, 0.08)
--ui-border-strong: rgba(255, 255, 255, 0.14)

--ui-text-primary:   #e0e0e0
--ui-text-secondary: #9a9a9a
--ui-text-muted:     #636363
```

Three text levels max. Do not invent intermediate shades.

#### Theme-Reactive Accents (from Omarchy)

```
--ui-accent:         var(--theme-accent)
--ui-accent-soft:    color-mix(in srgb, var(--theme-accent) 18%, transparent 82%)
--ui-success:        var(--theme-color2)
--ui-danger:         var(--theme-color1)
--ui-attention:      var(--theme-color3)
--ui-attention-soft: color-mix(in srgb, var(--theme-color3) 14%, transparent 86%)
```

These are the ONLY theme-reactive tokens in the shell chrome.

#### Terminal Colors (from Omarchy)

Terminal text, cursor, selection, and ANSI palette (color0-color15) come from `--theme-*` vars via the `terminalTheme()` function in TerminalPane.svelte. Terminal background uses `--ui-layer-0` (fixed neutral), NOT `--theme-background`. This keeps all panes visually calm while terminal content is fully colorful.

### Where Accent Colors Appear

USE accent for: focused pane border glow, active workspace left bar, active workspace row background (~10% opacity), status badges (12-15% opacity bg), interactive hover states (~6% opacity), notification badges, focused input borders, OpenFlow active edge animations.

NEVER use accent for: sidebar background, pane header background, large surface areas, body text, borders in resting state.

### Omarchy Integration

When Omarchy is available, all colors come from `~/.config/omarchy/current/theme/colors.toml`. When unavailable, fall back to `ThemeColors::default()`. The derived layer system means even the fallback looks correct.

For Omarchy-specific integration questions, use the `omarchy-kb` MCP server which has complete Omarchy, Hyprland, and Arch Linux documentation.

### The Golden Rule

Shell tokens (`--ui-layer-*`, `--ui-border-*`, `--ui-text-*`) are fixed hex values — they never reference `--theme-*`. Accent tokens (`--ui-accent`, `--ui-success`, `--ui-danger`, `--ui-attention`) use `var(--theme-*)`. Terminal colors use `var(--theme-*)`. Never use `--theme-background` or `--theme-foreground` for shell chrome.

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

### Motion

```
--ui-motion-fast:  120ms ease-out   /* hover, button feedback */
--ui-motion-base:  160ms ease-out   /* panel reveals, focus */
--ui-motion-slow:  240ms ease-out   /* view transitions, drawers */
```

Subtle always. No bounces, springs, or attention-seeking motion. `ease-out` for everything.

### Spacing

- Base unit: 4px. All spacings multiples of 4: 4, 8, 12, 16, 20, 24, 32.
- Pane gap: 4px (CSS Grid gap).
- Pane container padding: 4px.
- Pane border-radius: `var(--ui-radius-lg)` (10px).
- Compact elements: 8-12px padding. Spacious sections: 16-24px.

### Typography

- Shell chrome: system UI font stack, 0.7-0.9rem.
- Terminal: `--shell-font-family` or monospace fallback, 13px.
- Nothing larger than 1.1rem in the app shell except OpenFlow config panel headers.

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

### Always use tokens

```css
/* Correct — use --ui-* tokens for shell chrome */
background: var(--ui-layer-2);
color: var(--ui-text-secondary);
border-radius: var(--ui-radius-md);
transition: all var(--ui-motion-fast);

/* Correct — use color-mix with accent tokens for tinted surfaces */
background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);

/* Wrong — raw hex/rgba instead of tokens */
background: #1d2231;
color: rgba(122, 162, 247, 0.1);
```

### Scoped styles only

Every component uses `<style>` with Svelte scoping. No `:global()` except in App.svelte for token definitions. Pass parent styles to children via CSS custom properties or props.

### Flex/grid overflow prevention

Always set `min-width: 0` and `min-height: 0` on flex and grid children to prevent content overflow.

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

- Add Tailwind, SCSS, or CSS modules — plain CSS with custom properties only
- Add global CSS files — tokens live in App.svelte `:root`
- Import component libraries — keep it custom and lightweight
- Use `px` font sizes in shell chrome — use `rem`
- Add attention-seeking animations to the chrome
- Use `--theme-background` or `--theme-foreground` in shell chrome — use `--ui-*` tokens
- Hardcode `rgba()` for accent-derived colors — use `color-mix()` with `var(--ui-accent)` etc.
