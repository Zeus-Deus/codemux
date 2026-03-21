# Superset UI Analysis — Codemux Redesign Reference

Deep analysis of the Superset desktop app (superset.sh) compared to Codemux. For each area: what Superset does, what Codemux currently has, and what needs to change.

**Source**: `/tmp/superset-ref/apps/desktop/src/` (React/TypeScript/Tailwind)
**Target**: `/home/zeus/Documents/projects/codemux/src/` (Svelte/TypeScript/CSS custom properties)

---

## 1. SIDEBAR

### Superset's Approach

**Components:**
- `renderer/screens/main/components/WorkspaceSidebar/WorkspaceSidebar.tsx`
- `WorkspaceSidebarHeader/WorkspaceSidebarHeader.tsx`
- `WorkspaceSidebarFooter.tsx`

**Structure (top to bottom):**
1. **Header** — "Workspaces" / "Tasks" toggle buttons, "New Workspace" button (Cmd+Shift+N)
2. **Project Sections** — Collapsible groups that organize workspaces by project. Each project has: thumbnail, name, workspace count, color-coded left border (2px solid)
3. **Workspace List** — Per-workspace rows inside each project section
4. **Ports List** — Active detected ports (only when expanded, only when ports exist)
5. **Setup Script Card** — Informational
6. **Footer** — "Add repository" button with dropdown (Open project, New project)

**Workspace Row Data (per row):**
- Icon: `LuFolderGit2` (worktree) or `LuLaptop` (branch)
- Workspace name (text-[13px], font-medium when active)
- Branch name (text-[11px], font-mono, secondary color)
- Status indicator (working spinner, review badge, unread dot)
- Ahead/behind counts (emerald-500 for ahead, amber-500 for behind)
- Diff stats (+additions in emerald-500/90, -deletions in red-400/90)
- PR status badge (if applicable)
- Close button (visible on hover)

**Row Styling:**
- Padding: `pl-3 pr-2`, height `py-1.5` (with branch) or `py-2`
- Active: `bg-muted` + `text-foreground font-medium` + left accent bar (0.5px `bg-primary rounded-r`)
- Inactive: `text-foreground/80`
- Selected (multi-select): `bg-primary/10 ring-1 ring-inset ring-primary/30`
- Hover: `hover:bg-muted/50`
- Gap: `gap-1.5`

**Collapse/Expand:**
- Two modes: expanded (full labels) and collapsed (icon-only, 32x32 buttons)
- Collapsed width: ~80px, expanded: ~280px
- Framer Motion animations for section collapse (duration 0.15s, easeOut)

**Right-Click Menus:**
- Workspace: Rename, Open in Finder, Copy Path, Set Unread, Reset Status, Move to Section
- Section: Rename, Set Color, Delete Section
- Double-click section header to rename

**Section Headers:**
- Font: text-[11px], font-medium, uppercase, tracking-wider
- Color: text-muted-foreground
- Hover: hover:bg-muted/50
- Chevron: LuChevronRight, rotates 90° when expanded
- Color-coded left border (2px solid, custom per section)

**Container:**
- Background: `bg-muted/45` (dark: `bg-muted/35`)
- Full height flex column

### Codemux Current State

**Components:** `Sidebar.svelte`, `WorkspaceRow.svelte`

**Structure:**
1. Header — Diamond brand mark + "Codemux" text, new workspace button
2. Active workspace name/path display
3. Workspace list (flat, no project grouping)
4. Three expandable sections: Notifications, OpenFlow, Memory
5. Footer — Sound toggle, test alert button

**Workspace Row:**
- Title + metadata (git branch, path)
- Left accent bar (2px, accent color when active, attention color when unread)
- Notification count badge
- Close button on hover
- Min-height: 50px, border-radius: 6px

**Styling:**
- Width: 240px fixed
- Background: `var(--ui-layer-1)` (#151719)
- Border-right: `1px solid var(--ui-border-soft)`

### What Codemux Needs to Change

1. **Add project grouping** — Group workspaces by project/repo with collapsible sections, color-coded left borders, workspace counts
2. **Enrich workspace rows** — Add branch name (monospace, small), diff stats (+/-), ahead/behind counts, status indicators (working/review/idle)
3. **Add sidebar collapse mode** — Icon-only mode (~60-80px) with square icon buttons
4. **Add right-click context menus** — Rename, move to section, copy path, set unread
5. **Add "Add repository" footer** — Replace sound toggle with project-management actions
6. **Match spacing** — Tighten row padding (`pl-3 pr-2` instead of current larger values), reduce font sizes (13px name, 11px branch)
7. **Add section headers** — 11px uppercase tracking-wider for section dividers
8. **Add drag-and-drop** — Section reordering support

---

## 2. WORKSPACE CREATION FLOW

### Superset's Approach

**Components:**
- `renderer/components/NewWorkspaceModal/NewWorkspaceModal.tsx`
- `NewWorkspaceModalContent.tsx`
- `PromptGroup.tsx`
- `NewWorkspaceModalDraftContext.tsx`

**Modal Specs:**
- Max width: 560px
- Max height: min(70vh, 600px)
- Background: `bg-popover` (#201E1C)
- No close button visible
- Padding: 0 (content handles its own padding)
- Centered positioning

**Flow:**
1. Project selection (recent projects dropdown)
2. Branch selection (search/browse, supports PR checkout)
3. Workspace name input (auto-generated from branch, max 64 chars)
4. Options: Link PR, Link GitHub Issue, Agent selection
5. Create button

**Key Details:**
- Linked issue/PR shown as pills (22px height, blue tint, X to remove)
- Agent selection remembers last choice via localStorage
- Branch search with real-time filtering
- Input styling: border-[0.5px], bg-foreground/[0.04]

**Keyboard:** Cmd+Shift+N to open

### Codemux Current State

**Component:** `NewWorkspaceLauncher.svelte`

**Modal Specs:**
- Max width: 860px, max height: 760px
- Background: `var(--ui-layer-1)` (#151719)
- Backdrop: rgba(5, 7, 12, 0.72)
- Border-radius: 12px
- Box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35)

**Flow (3-step wizard):**
1. Kind selection (Codemux workspace, Open folder, OpenFlow run)
2. Layout selection (1/2/4/6/8 panes, shell+browser combo)
3. Details form

**Key Details:**
- Step indicator with progress bar
- Layout preview grid showing pane arrangements
- OpenFlow skips layout step
- Folder picker dialog integration

### What Codemux Needs to Change

1. **Add git branch integration** — Branch search/selection (new branch, existing, PR checkout)
2. **Add project context** — Show which project/repo the workspace belongs to
3. **Simplify the modal** — Superset uses a single clean form instead of a multi-step wizard. Consider collapsing to one screen with expandable sections
4. **Shrink the modal** — 560px is more focused than 860px. Reduce max-width
5. **Add linked context** — PR link, issue link, agent selection as optional pill attachments
6. **Match input styling** — Thinner borders (0.5px), subtle foreground-tinted backgrounds
7. **Keep layout selection** — This is a Codemux differentiator (Superset doesn't have pane layout presets)

---

## 3. TAB BAR / WORKSPACE NAVIGATION

### Superset's Approach

**Components:**
- `ContentView/TabsContent/GroupStrip/GroupStrip.tsx`
- `GroupStrip/GroupItem.tsx`
- `GroupStrip/components/AddTabButton/AddTabButton.tsx`

**Tab Bar:**
- Height: 40px (h-10)
- Horizontal scroll with hidden scrollbar
- Each tab: 160px fixed width
- Border-right between tabs: `border-border`

**Tab Content:**
- Active tab: `text-foreground bg-border/30`
- Inactive: `text-muted-foreground/70`, hover: `hover:bg-tertiary/20`
- Font: text-sm, truncated
- Close button: absolute positioned right, hidden until hover (group-hover:flex), 24px square

**Tab Types:** Terminal, Chat, Browser, File Viewer, Diff View

**Tab Context Menu:** Rename, Mark as Unread, Close
**Edit:** Double-click to rename, max 64 chars

**Add Tab Button:**
- Compact mode: `+` icon (28px square), dropdown on click
- Full mode: Terminal | Chat | Browser buttons side-by-side + dropdown chevron
- Dropdown: Terminal, Chat, Browser, separator, Presets (submenu), separator, Show Preset Bar, Use Compact Button
- Button styling: h-7, text-xs, border-border/60, bg-muted/30, hover:bg-accent/60

**Overflow:** When tabs overflow, add button becomes sticky right with bg-background/95

**Keyboard:**
- Cmd+T: New terminal tab
- Cmd+Shift+C: New chat tab
- Cmd+Shift+B: New browser tab
- Tab switching with number keys (1-9)
- Cmd+W: Close tab

### Codemux Current State

**No tab bar exists.** Workspaces switch via sidebar. Within a workspace, all panes are visible simultaneously in the split layout. There's no concept of tabs within a workspace.

### What Codemux Needs to Add

1. **Add a tab bar** — 40px horizontal strip above the pane area. Each tab represents a different "view" or "group" within a workspace
2. **Tab types** — At minimum: Terminal (current pane layout), Changes/Diff, Browser. Future: Chat, File viewer
3. **Fixed tab width** — 160px per tab, horizontal scroll on overflow
4. **Add tab button** — Terminal/Browser/Diff quick-create, dropdown for presets
5. **Tab state** — Active tab shows content, inactive tabs are hidden but preserved
6. **Keyboard shortcuts** — Cmd+T for new tab, Cmd+W to close, number keys to switch
7. **Context menus** — Right-click: rename, close
8. **Match styling** — 40px height, text-sm, soft background states, border-right dividers

---

## 4. DIFF VIEWER

### Superset's Approach

**Components:**
- `ChangesContent/components/FileDiffSection/FileDiffSection.tsx` (560 lines)
- `FileDiffSection/components/FileDiffHeader/FileDiffHeader.tsx` (215 lines)
- `ChangesContent/components/LightDiffViewer/LightDiffViewer.tsx`
- `ContentView/TabsContent/TabView/FileViewerPane/FileViewerPane.tsx` (672 lines)
- `FileViewerPane/components/FileViewerToolbar/FileViewerToolbar.tsx` (194 lines)

**Two Implementations:**
1. **Changes Panel** (right sidebar) — File list + inline diffs for staging/unstaging
2. **File Viewer Pane** (main content tab) — Full diff view with toolbar

**File List Entry (FileDiffHeader):**
- Sticky header (top: 0, z-index: 10)
- Background: `bg-muted`
- Layout: expand button, status badge (color-coded), file path (font-mono, text-xs, clickable), copy path button, edit toggle, stats (+green/-red), stage/unstage/discard buttons
- Padding: px-3 py-1.5
- Additions: `text-green-600 dark:text-green-500`
- Deletions: `text-red-600 dark:text-red-400`

**Diff Rendering:**
- Library: `@pierre/diffs/react` with `MultiFileDiff` component
- Syntax highlighting: Shiki
- Two modes: side-by-side (split) and inline (unified)
- Toggle: hide/show unchanged regions
- CSS variables for diff colors:
  - Addition: `#4ade80` (green)
  - Deletion: `#f87171` (red)
  - Modified: `#fbbf24` (amber)
  - Line numbers: gutterForeground color
  - Font size: 12px, line height: 18px (1.5x)

**Diff Toolbar:**
- Sticky top, z-index 30
- Background: `bg-background`, border-bottom + border-right
- Contents: viewed count (X/Y), file stats, push/pull indicators, focus mode toggle, view mode toggle, hide unchanged toggle, prev/next navigation

**Edit Mode:**
- Toggle per file in the diff header
- Opens CodeEditor with max-height 70vh, min-height 240px
- Save conflict detection (file changed on disk while editing)

**Focus Mode:**
- Expanded single-file diff view
- Navigation between files (prev/next + dropdown)

### Codemux Current State

**No diff viewer exists.** Git branch is displayed in workspace rows but there's no way to view diffs, stage changes, or commit.

### What Codemux Needs to Add

1. **File diff component** — File list with status badges (added/modified/deleted/renamed), stats (+/-), stage/unstage controls
2. **Diff rendering** — Either use a library like `@pierre/diffs` or build a simple unified diff viewer. Minimum: line numbers, +/- coloring, syntax highlighting
3. **Diff color system** — Addition: green (#4ade80 or --ui-success), Deletion: red (#f87171 or --ui-danger), Modified: amber (--ui-attention)
4. **View modes** — Side-by-side and inline/unified
5. **Tab integration** — Diff view as a tab type in the new tab bar
6. **Stage/unstage/commit controls** — Per-file and bulk actions
7. **Focus mode** — Expand single file diff to full view
8. **Font** — Monospace, 12px, 1.5x line height for diff content

---

## 5. TERMINAL AREA

### Superset's Approach

**Components:**
- `ContentView/TabsContent/Terminal/Terminal.tsx` (468 lines)
- `Terminal/TerminalSearch/TerminalSearch.tsx` (192 lines)

**Terminal Library:** xterm.js v6+ with FitAddon and SearchAddon

**Container:**
- Full width/height with relative positioning
- Padding: `p-2` (8px) around terminal element
- Background: dynamic from terminal theme
- Supports drag-and-drop (files into terminal)

**Terminal Search Bar:**
- Position: absolute top-1 right-1, z-index 10
- Background: `bg-popover/95` with `backdrop-blur`
- Ring: `ring-1 ring-border/40`
- Shadow: `shadow-lg`
- Input: h-6, w-28, text-sm, transparent bg
- Match counter: text-xs, muted color
- Buttons: case toggle (PiTextAa), prev (chevron up), next (chevron down), close (X)
- Search decorations: match bg #515c6a, active match border #ffd33d (gold)

**Scroll to Bottom Button:**
- Floats above terminal when scrolled up
- Click to jump to bottom

**Terminal States:**
- Session killed overlay with restart button
- Restored mode overlay (cold restore)
- Connection error handling

**Multiple Terminals:** Appear as separate panes in the mosaic layout (not terminal-internal tabs)

**Pane Toolbar:** 28px height with file path/label, pin button, split options (h/v/auto), close button

### Codemux Current State

**Components:** `PaneNode.svelte` (pane shell + header), `TerminalPane.svelte` (xterm wrapper)

**Terminal:** xterm.js with FitAddon, WebglAddon, ClipboardAddon, SearchAddon
- Padding: 6px 8px 8px
- Background: `var(--ui-layer-0, #0d0f11)` (fixed neutral)
- Theme colors from `--theme-*` vars (foreground, cursor, ANSI colors)
- Status overlay for starting/failed states

**Pane Header:**
- Background: `color-mix(in srgb, var(--ui-layer-1) 80%, transparent 20%)`
- Active: 5% accent tint
- Buttons: split h/v, open browser, close (24px, hidden until hover)
- Drag handle for pane swapping

### What Codemux Needs to Change

1. **Add terminal search** — Floating search bar (top-right), input + match counter + prev/next/close. Superset's design: popover bg with backdrop-blur, ring border, shadow
2. **Add scroll-to-bottom button** — Floating button when scrolled up
3. **Add drag-and-drop file support** — Drop files into terminal to paste path
4. **Add terminal presets** — Quick-launch bar for common terminal configurations (from Superset's preset system)
5. **Match search decoration colors** — Match background: #515c6a, active match border: #ffd33d
6. **Keep existing strengths** — Kitty protocol support, WebGL renderer, clipboard addon are already competitive

---

## 6. PORT PANEL

### Superset's Approach

**Components:**
- `WorkspaceSidebar/PortsList/PortsList.tsx`
- `PortsList/components/WorkspacePortGroup/WorkspacePortGroup.tsx`
- `PortsList/components/MergedPortBadge/MergedPortBadge.tsx`

**Location:** Sidebar, below workspace list. Only visible when ports detected (`totalPortCount > 0`).

**Header:**
- Font: 11px, uppercase, tracking-wider, muted color
- Icon: LuRadioTower before "Ports"
- Chevron for collapse/expand
- Help icon (LuCircleHelp) visible on hover
- Count badge: text-[10px]
- Top border separator: `border-t border-border`

**Expanded View:**
- Max height: 288px (max-h-72)
- Scrollable with hidden scrollbar
- Grouped by workspace

**Workspace Port Group:**
- Header: workspace name (text-xs, clickable), close-all button on hover
- Port badges container: `flex flex-wrap gap-1 px-3`

**Individual Port Badge:**
- Inline pill: `rounded-md text-xs`
- Background: `bg-primary/10`, hover: `bg-primary/20`
- Text: `text-primary`
- Content: port label + port number (font-mono, muted) + open-browser icon (hover) + close icon
- Click: navigates to workspace + focuses spawning terminal
- Tooltip: label, localhost:PORT, process name (pid), "Click to open workspace"

### Codemux Current State

**No port panel exists.** No port detection or display.

### What Codemux Needs to Add

1. **Port detection backend** — Detect listening ports per workspace (scan `/proc/net/tcp` or use `ss`/`lsof`)
2. **Ports section in sidebar** — Below workspace list, collapsible, only visible when ports exist
3. **Port badge pills** — Inline flex-wrap layout, primary/10 background, port number in monospace
4. **Actions per port** — Open in browser (BrowserPane), click to focus spawning terminal, close/kill
5. **Group by workspace** — Show which workspace each port belongs to
6. **Match styling** — 11px uppercase header, text-xs badges, rounded-md, primary color tint

---

## 7. BROWSER PANE

### Superset's Approach

**Components:**
- `ContentView/TabsContent/TabView/BrowserPane/BrowserPane.tsx` (139 lines)
- `BrowserPane/components/BrowserToolbar/BrowserToolbar.tsx` (221 lines)
- `BrowserErrorOverlay.tsx`
- `UrlSuggestions.tsx`

**Toolbar Layout:**
- Height: inherited from pane toolbar (~28px)
- Padding: px-2
- Sections: [Back | Forward | Reload] | separator | [URL bar] | separator | [DevTools] | [overflow menu]

**Navigation Buttons:**
- Icon size: 14px (size-3.5)
- Padding: p-1 (rounded)
- Color: muted-foreground/60, hover: muted-foreground
- Disabled: opacity-30, pointer-events-none

**Separator:** Vertical line, mx-1.5, h-3.5, w-px, bg-muted-foreground/60

**URL Bar:**
- Display mode: full width, text-xs, muted URL color, page title fades on hover
- Edit mode: input h-[22px], border-ring, rounded-sm, text-xs
- Placeholder: "Enter URL or search..."
- URL suggestions dropdown below input

**Blank Page:**
- Globe icon (40px, very muted), "Browser" label, "Enter a URL above, or instruct an agent to navigate" hint

**DevTools Button:**
- Icon: TbDeviceDesktop (14px)
- Color: muted-foreground/60, hover: muted-foreground
- Tooltip: "Open DevTools"

### Codemux Current State

**Component:** `BrowserPane.svelte`

**Toolbar:**
- Height: ~36px (larger than Superset)
- Address bar: 26px height, border-radius: 5px, 0.78rem font
- Buttons: home, refresh, external link (26x26px)
- No back/forward navigation
- No DevTools toggle

**Content:**
- Screenshot-based rendering (base64 PNG data URI)
- 1-second refresh interval
- Aspect ratio correction on clicks
- Crosshair cursor on viewport
- Status banner for errors

### What Codemux Needs to Change

1. **Add back/forward navigation** — Browser history with back/forward buttons
2. **Add DevTools toggle** — Button to open/close DevTools panel
3. **Improve URL bar** — Show page title alongside URL (Superset pattern), URL autocomplete/suggestions
4. **Add separator pattern** — Vertical line dividers between button groups
5. **Shrink toolbar** — Match 28px pane toolbar height instead of current ~36px
6. **Match button sizing** — 14px icons with p-1 padding, muted colors
7. **Add blank page state** — Globe icon + helpful text instead of empty white
8. **Add overflow menu** — Additional actions (print, open externally, etc.)

---

## 8. OVERALL STYLING PATTERNS

### Superset's Color Palette (Dark/Ember Theme)

**Backgrounds:**
| Token | Value | Usage |
|-------|-------|-------|
| background | `#151110` | Base app background |
| tertiary | `#1a1716` | Sidebar, panels |
| muted | `#2a2827` | Elevated surfaces, hover states |
| card | `#201E1C` | Cards |
| popover | `#201E1C` | Popovers, modals |
| tertiary-active | `#252220` | Active panel states |

**Text:**
| Token | Value | Usage |
|-------|-------|-------|
| foreground | `#eae8e6` | Primary text |
| muted-foreground | `#a8a5a3` | Secondary text |
| (opacity variants) | foreground/60, /80 | Muted, very muted |

**Borders:**
| Token | Value |
|-------|-------|
| border | `#2a2827` |
| ring | `#3a3837` |

**Accent (Sidebar Primary):**
| Token | Value | Usage |
|-------|-------|-------|
| sidebar-primary | `#e07850` | Primary accent color (orange) |
| chart-1 | `#e07850` | Same orange |
| chart-2 | `#50a878` | Green |
| chart-3 | `#d4a84b` | Gold |
| chart-4 | `#7b68ee` | Purple |
| chart-5 | `#dc6b6b` | Red |

**Status Colors:**
| State | Color |
|-------|-------|
| Additions | `text-emerald-500/90` (#10b981) |
| Deletions | `text-red-400/90` (#f87171) |
| Ahead | `text-emerald-500` |
| Behind | `text-amber-500` |
| Unread dot | `bg-blue-500` |
| Needs input | `bg-red-500` (pulsing) |
| Agent working | `bg-amber-500` (pulsing) |
| Ready for review | `bg-green-500` (static) |

**Destructive:** `#cc4444` (warm red)

### Codemux's Color Palette (Current)

**Backgrounds:**
| Token | Value |
|-------|-------|
| --ui-layer-0 | `#0d0f11` |
| --ui-layer-1 | `#151719` |
| --ui-layer-2 | `#1c1e22` |
| --ui-layer-3 | `#252830` |

**Text:**
| Token | Value |
|-------|-------|
| --ui-text-primary | `#e0e0e0` |
| --ui-text-secondary | `#9a9a9a` |
| --ui-text-muted | `#636363` |

**Borders:**
| Token | Value |
|-------|-------|
| --ui-border-soft | `rgba(255, 255, 255, 0.08)` |
| --ui-border-strong | `rgba(255, 255, 255, 0.14)` |

**Accents (theme-reactive):**
| Token | Default |
|-------|---------|
| --ui-accent | `#7aa2f7` (blue) |
| --ui-success | `#9ece6a` (green) |
| --ui-danger | `#f7768e` (red) |
| --ui-attention | `#e0af68` (orange) |

### Comparison & What to Match

**Palette character:** Superset uses warm browns/tans (#151110, #2a2827, #eae8e6). Codemux uses cool neutral grays (#0d0f11, #151719, #e0e0e0). Codemux's palette is already more neutral and professional — keep it.

**Font Stack:**
- Superset: SF Mono (macOS) with system fallback. Sans-serif for UI text.
- Codemux: JetBrainsMono Nerd Font, monospace everywhere.
- **Change needed:** Consider using a sans-serif font for shell chrome (sidebar, headers, badges) and keep monospace only for terminal content. This is a major polish differentiator.

**Font Sizes:**
- Superset: 11px section headers, 13px workspace names, 12px (text-xs) for small text, 14px (text-sm) for normal
- Codemux: 13px base, 0.76rem (~10px) headers, 0.8rem names, 0.72rem metadata
- Codemux is close but slightly smaller in some places.

**Border Radius:**
- Superset: Base 10px (--radius: 0.625rem), sm: 6px, md: 8px, lg: 10px, xl: 14px
- Codemux: --ui-radius-sm: 6px, --ui-radius-md: 8px, --ui-radius-lg: 10px
- **Already matched.**

**Spacing:**
- Superset: Tailwind 4px base (pl-2=8px, pl-3=12px, py-1.5=6px, py-2=8px, gap-1.5=6px)
- Codemux: Similar 4px-based system
- **Already close.** Minor tightening in some components.

**Transitions:**
- Superset: `transition-all` (Tailwind default 150ms), `transition-colors`, 200ms for dialogs
- Codemux: --ui-motion-fast: 120ms, --ui-motion-base: 160ms, --ui-motion-slow: 240ms
- **Already comparable.**

**Icon System:**
- Superset: Heroicons (HiOutline, HiMini, HiSolid) + Lucide React + Tabler Icons + GitHub Octicons
- Codemux: Inline SVGs in components
- **Consider:** A consistent icon set (Lucide or Tabler) would improve consistency

**Scrollbar Styling:**
- Superset: 12px standard / 8px thin, transparent track, semi-transparent gray thumb with hover/active states
- Codemux: No custom scrollbar styling
- **Add:** Custom scrollbar CSS for consistency

---

## 9. SETTINGS / CONFIGURATION UI

### Superset's Approach

**Route:** `/_authenticated/settings`
**Layout:** `routes/_authenticated/settings/layout.tsx`

**Structure:**
- Left sidebar with searchable section list
- Right content panel (bg-background, rounded corners, overflow-auto, p-6, max-w-4xl)
- Drag bar at top (88px left padding on macOS, 16px on Windows)

**Settings Sections (15 total):**
1. Account
2. Appearance (themes, fonts, markdown styles)
3. Ringtones (notification sounds)
4. Keyboard (hotkeys)
5. Behavior
6. Git
7. Terminal
8. Models (AI)
9. Organization
10. Integrations
11. Billing
12. Devices
13. API Keys
14. Permissions
15. Project

**Appearance Section Detail:**
- Theme cards: 2px border, rounded-lg, 28px preview height with fake terminal output
- Selected theme: `border-primary ring-2 ring-primary/20`
- Color strip: 6 ANSI accent color swatches
- Info: theme name, author, checkmark when selected
- Import/Export buttons for custom themes
- Markdown style selector (Default, Tufte)
- Font settings: family selector (Combobox), live preview

**Settings Search:** Semantic search across all sections, auto-navigates to matches

### Codemux Current State

**No settings panel.** Only a sound toggle in the sidebar footer. Theme comes from Omarchy system theme.

### What Codemux Needs to Add

1. **Settings panel** — Full settings screen accessible via sidebar or keyboard shortcut
2. **Minimum sections:** Appearance (shell font, terminal font), Keyboard (hotkeys), Terminal (scrollback, cursor style), Git (default branch, author)
3. **Layout:** Left nav + right content panel (Superset pattern), or simpler drawer/modal for fewer settings
4. **Font picker** — Shell font and terminal font as separate settings
5. **Keybinding editor** — Display and customize keyboard shortcuts
6. **Match styling:** p-6 content padding, max-w-4xl, rounded panel on bg-background

---

## 10. NOTIFICATIONS

### Superset's Approach

**Library:** Sonner (React toast library)
**Component:** `components/ThemedToaster/ThemedToaster.tsx`

**Toast Styling:**
- Background: `var(--popover)` (#201E1C)
- Text: `var(--popover-foreground)` (#eae8e6)
- Border: `var(--border)` (#2a2827)
- Border-radius: `var(--radius)` (10px)
- Max height: 80dvh
- Position: top-right (Sonner default)
- Text selectable

**Icons (per type):**
- Success: CircleCheckIcon (green)
- Info: InfoIcon (blue)
- Warning: TriangleAlertIcon (yellow)
- Error: OctagonXIcon (red)
- Loading: Loader2Icon (spinning)
- Size: 16px

**Update Toast (custom):**
- Container: `flex flex-col gap-3`, bg-popover, rounded-lg, border, p-4, shadow-lg, min-w-[340px]
- Close button: absolute top-2 right-2, 24px
- Title: font-medium text-sm (or text-destructive for errors)
- Subtitle: text-sm text-muted-foreground
- Tertiary: text-xs text-muted-foreground/70
- Actions: "See changes" (secondary) + "Install" (primary) buttons

**Animation:** Sonner built-in (slide in/out, fade)

### Codemux Current State

**Component:** Toast system in `App.svelte` (`.global-notice`)

**Current Implementation:**
- Position: bottom-right (right: 18px, bottom: 18px)
- Max width: min(520px, calc(100vw - 36px))
- Background: `color-mix(in srgb, var(--ui-layer-2) 92%, black 8%)`
- Border: accent-tinted (26% for info, 42% for error)
- Border-radius: 10px
- Shadow: 0 12px 30px rgba(0, 0, 0, 0.28)
- Close button: 22px circle
- Font: 0.8rem
- Error state: danger-tinted border and background

### What Codemux Needs to Change

1. **Add notification types** — Success (green icon), info (blue), warning (yellow), error (red), loading (spinner)
2. **Add icons** — Type-specific icons next to notification text
3. **Consider top-right positioning** — Superset (and most apps) use top-right for toasts. Bottom-right works but is less conventional.
4. **Add action buttons** — Some notifications should have action buttons (e.g., "Restart", "View diff")
5. **Add auto-dismiss** — Configurable duration (3-5s for success, longer for errors)
6. **Add notification stacking** — Multiple simultaneous notifications

---

## PRIORITY RANKING

Based on impact and effort, recommended implementation order:

### High Priority (Visual Polish)
1. **Tab bar** — Biggest structural gap. Required for diff viewer and multi-view workspaces.
2. **Sidebar enrichment** — Branch info, diff stats, project grouping. Highest visibility improvement.
3. **Font system** — Sans-serif for shell chrome, monospace for terminals only. Major polish signal.
4. **Terminal search** — Expected feature, moderate effort.

### Medium Priority (Feature Parity)
5. **Diff viewer** — Core developer workflow. Depends on tab bar.
6. **Settings panel** — Expected in any serious app. Font picker, keybindings, terminal options.
7. **Port detection** — Useful for web dev workflows. Sidebar section.
8. **Browser pane improvements** — Back/forward, DevTools toggle, URL suggestions.

### Lower Priority (Polish)
9. **Notification system upgrade** — Icons, types, action buttons, stacking.
10. **Sidebar collapse mode** — Nice to have, saves horizontal space.
11. **Custom scrollbars** — Small but noticeable polish.
12. **Workspace creation simplification** — Current wizard works, could be cleaner.

---

## CLEANUP

```
rm -rf /tmp/superset-ref
```
