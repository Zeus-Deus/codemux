---
name: codemux-ui
description: Use when building, modifying, or improving any user-visible part of Codemux — React components, Tailwind, shadcn, theming, pane layouts, sidebar, notifications, OpenFlow UI. Also use when the task involves visual design decisions, component patterns, or color/theme work. For new ADE feature ideation see `codemux-features`; for chat-pane specifics see `codemux-chat-ui`.
---

# Codemux UI Standards

Project-specific delta on top of generic frontend best practices. For chat-pane specifics, see `codemux-chat-ui`. For project context, read `WORKFLOW.md` and `docs/INDEX.md` first.

Apply to new code and incremental improvements. Don't mass-refactor existing components to match.

---

## Theming

The frontend uses shadcn with preset `b3kIbNYVW` (zinc base, oklch, 0.45rem radius). Colors are CSS variables in `src/globals.css` under `:root` (light) and `.dark`. The app defaults to dark mode via `class="dark"` on `<html>`.

### Core tokens
- `--background`, `--foreground`, `--card`, `--muted`, `--muted-foreground`, `--accent`, `--primary`, `--destructive`, `--border` — standard shadcn
- `--success` (green), `--danger` (red), `--warning` (amber) — Codemux custom, used via `text-success`, `bg-danger`, etc.

### Terminal colors
Read dynamically from shadcn CSS variables via `buildThemeFromCSS()` in `TerminalPane.tsx`. ANSI palette is static. A MutationObserver on `<html>` re-applies the theme when the preset changes.

### Where accent (`primary`) belongs
**Use for:** focused pane border, active workspace left bar, active workspace row bg, status badges, interactive hover states, notification badges, focused input borders.

**Never for:** sidebar background, pane header background, large surfaces, body text, resting-state borders.

### Golden rule
All colors come from shadcn CSS variables. No hardcoded hex/rgba in components. Exceptions: `src/tauri/types.ts` (Rust data constants) and the ANSI palette in `TerminalPane.tsx`.

---

## Overlay Button Color Rule

Buttons inside overlays, wizards, splash screens, dialogs, and onboarding flows use neutral inverted styling: `bg-foreground text-background hover:bg-foreground/90`. This keeps overlays calm regardless of which shadcn preset is active — `--foreground` and `--background` are always neutral.

Buttons in the main app chrome (sidebar, preset bar, pane headers) may use `bg-primary` for accent emphasis. Secondary/cancel: `variant="outline"` or `"ghost"`. Destructive: `variant="destructive"`.

---

## Status Indicators

Consistent dot system across all agent and run states:

| State | Token | Visual |
|-------|-------|--------|
| Running / Active | `--primary` | Filled dot, subtle pulse |
| Success / Done | `--success` | Filled dot, static |
| Needs Attention | `--warning` | Filled dot + count badge |
| Error / Failed | `--danger` | Filled dot, static |
| Idle / Waiting | `--muted-foreground` | Hollow or dim filled dot |

Dot size: 6px in sidebar items, 8px in dashboards. Count badges: 18px pill, dark text on colored bg.

---

## Hover-Reveal Pattern

The default interaction model for list rows. At rest the row shows data; on hover, secondary actions appear, optionally replacing less-important metadata.

Used in: sidebar workspace rows, changes panel file rows, tab close buttons, pane header actions, port pills, branch picker rows.

```tsx
<div className="group flex items-center ...">
  <span className="transition-opacity group-hover:opacity-0">+42 −3</span>
  <Button className="opacity-0 group-hover:opacity-100 transition-opacity">
    <X className="h-3.5 w-3.5" />
  </Button>
</div>
```

For rows nested inside another `group`, use named groups (`group/row`, `group/pane`, `group/pill`) to avoid hover conflicts.

**Don't hover-reveal:** primary actions users always need to see (dialog CTAs, always-visible close buttons, status toggles).

---

## Compound Picker Pattern

The default for any selection UI with 5+ items or that benefits from search and keyboard nav. **Do NOT use `DropdownMenu` or `Select` for dynamic lists** — they lack search, keyboard navigation, and metadata space.

Structure: `Popover` > `Command` (cmdk). cmdk provides arrow-key nav, Enter to select, Escape to close, type-to-filter — zero custom keyboard handling.

Reference implementations:
- `src/components/overlays/branch-picker.tsx` — tabs, timestamps, hover-reveal actions
- `src/components/overlays/project-picker.tsx` — grouped lists, color avatars, footer actions

### Row anatomy (left to right)

| Element | Sizing | Notes |
|---------|--------|-------|
| Icon | `size-3.5`, muted | Lucide icon for item type/state |
| Label | `text-xs`, `truncate`, `flex-1 min-w-0` | `font-mono` for paths/branches/ports |
| Badges | `text-[9px]` | Optional metadata pills, `Badge variant="secondary"` |
| Metadata | `text-[11px] text-muted-foreground/60` | Right-aligned. Visible at rest, hidden on hover |
| Actions | `text-[10px] font-medium` | Right-aligned. Hidden at rest, visible on hover |

### Sizing
- Popover width: 340–420px
- Row height: `h-9`–`h-10` (36–40px)
- Max list height: 340–420px with scroll

### Notes
- Set `shouldFilter={false}` when you have tabs or manual filtering — filter in a `useMemo` instead
- For tab bars, place below `CommandInput` so typing filters immediately
- Pickers can safely live inside `Dialog` — Radix `Popover` portals to `<body>` (z-50) above the Dialog layer; click-outside closes only the popover

### Trigger style

Pill-button shared across all pickers:

```tsx
<button className="inline-flex items-center gap-1.5 rounded-full bg-muted/60
  px-2.5 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground">
  <Icon className="h-3 w-3" />
  <span className="max-w-[120px] truncate">{selectedLabel}</span>
  <ChevronDown className="h-2.5 w-2.5 opacity-40" />
</button>
```

---

## Overlay Manager

All full-screen or centered overlays (command palette, search dialogs, modals, confirmation dialogs) MUST use the global overlay manager at `src/stores/overlay.ts`. Only one overlay is visible at a time.

Rules:
- Register the kind in `OverlayKind` type in `src/stores/overlay.ts`
- Open/close via `toggleOverlay(kind)` / `closeOverlay()` — never manage visibility in the component
- Render conditionally in `App.tsx` based on active overlay state
- All overlays: centered, z-index 100, backdrop `rgba(0,0,0,0.4)`, same border-radius and shadow
- Escape always closes the active overlay
- Opening a new overlay auto-closes the previous one
- Overlays do NOT stack

This prevents overlapping dialogs, z-index wars, inconsistent positioning, and escape-key conflicts.

---

## Right Panel for Auxiliary Views

The main workspace area hosts working surfaces (terminal, browser, chat). Non-working auxiliary views (file tree, changes/diff, etc.) live as tabs in the collapsible right sidebar panel, not as separate panels in the workspace area.

---

## Do Not

- Hardcode hex/rgba colors — use shadcn tokens via Tailwind
- Use `DropdownMenu` or `Select` for dynamic lists — use the Compound Picker pattern
- Add separate CSS files per component — Tailwind only, single global is `src/globals.css`
- Import UI libraries besides shadcn — use primitives from `src/components/ui/`
- Manage overlay visibility outside the overlay manager
- Use `bg-primary` on overlay buttons — use `bg-foreground text-background` instead
