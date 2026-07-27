# Project Avatars

- Purpose: Describe the current capability and constraints of sidebar project avatar customization (image + accent color).
- Audience: Anyone working on sidebar project identity or the project picker UI.
- Authority: Canonical feature-level reality doc.
- Update when: Avatar resolution, persistence keys, or picker behavior changes.
- Read next: `docs/features/workspace-creation.md` (project onboarding), `docs/reference/FEATURES.md`

## What This Feature Is

Every workspace row in the left sidebar renders a small project avatar (`ProjectAvatar`). By default it shows the project's first letter; users can customize it per project with an image (logo or website favicon) and/or an accent color from the **project section of any workspace's right-click menu**. The same avatar component renders in the project picker overlay.

## Current Model

- Right-click **any workspace** in the sidebar inbox (active card or settled row) → a `Project "<name>"` submenu, sitting between the workspace actions and the device actions. It offers an image entry that opens `ProjectImageDialog` plus a fixed 12-color palette (Red, Orange, Yellow, Lime, Green, Teal, Cyan, Blue, Indigo, Purple, Pink, Slate — `PROJECT_COLORS` in `project-appearance-menu.tsx`).
  - The submenu is labeled with the owning project's display name, because the setting applies to the **whole project**, not the one workspace that was right-clicked. The project is resolved from the `repo` (`{name, path}`) each inbox row already carries for its avatar.
  - This replaced the original entry point, the project-group header context menu, which went unreachable when PR #198 swapped the nested project tree for the flat workspace inbox.
- The dialog accepts three input shapes, classified by `resolveImageUrl` in `src/lib/project-image.ts`:
  - a **direct image URL** (recognized by extension: png/jpe?g/gif/svg/webp/ico/avif/bmp) — passed through untouched
  - a **`data:` URL** — passed through untouched
  - a **website URL or bare domain** — routed through Google's favicon service (`https://www.google.com/s2/favicons?domain=<domain>&sz=128`), which handles redirects, multiple favicon sizes, and sites without a root `/favicon.ico`
- **Favicon cache-busting** (PR #71, shipped `v0.8.0`): each save stamps a `Date.now()` version persisted next to the image input; `resolveImageUrl(input, cacheBust)` appends it to *derived favicon URLs only* as `&v=`, so a site whose favicon changed visibly refreshes instead of the WebView serving the same cached bytes forever. Direct/data URLs are never modified (never corrupt a signed or already-complete URL).
- **Persistence** is per-project UI state in the desktop SQLite DB via `dbSetUiState`/`dbGetUiState`, keyed by project path: `project.image:<path>`, `project.image.v:<path>` (the cache-bust token), `project.color:<path>`. Clearing writes an empty string. Device-local only — not part of synced settings.
- **State lives in `src/stores/project-appearance-store.ts`**, shared by the one writer (the context-menu project section) and every reader. A save therefore repaints all surfaces of that project *immediately* — the previously read-only `useProjectAppearance` hook cached per component, so a write only showed up on the next remount. The store also dedupes the read, so a project costs one DB round-trip no matter how many of its avatars are mounted, and an in-flight read never clobbers a value the user just picked.
- **Render precedence** in `ProjectAvatar`: image → color disc → first-letter fallback. A failed image load falls back to the letter, and the failure flag resets whenever the resolved URL changes (new image or fresh cache-bust token) so one transient failure doesn't pin the fallback. Supports `sm`/`md`/`lg` sizes and circle/square shapes.

## What Works Today

- Rendering of already-saved avatars (image → color disc → letter) on every surface
- Per-project custom image via direct URL, data URL, or domain-derived favicon
- Per-project accent color from a 12-color palette; color also tints the letter fallback
- Re-saving (or re-opening the picker for) a website image re-fetches the favicon via the `&v=` cache-bust token
- Clear/reset back to the letter fallback from the same context menu
- Reachable by right-clicking any workspace of the project — active inbox card or settled row
- A save repaints every surface of that project at once (all its cards, settled rows, the collapsed rail, the project filter dropdown)
- Avatar renders consistently across the inbox, the rail, and the project picker overlay

## Current Constraints

- Favicon derivation depends on Google's public favicon service — needs network, and the service controls quality/size (requested at 128px)
- The color palette is fixed (no custom hex input)
- Persistence is device-local UI state; avatars do not sync across devices
- No local image file picker — input is URL/domain text only (data URLs work but must be pasted)
- A project with no workspace in the inbox has no right-click surface, so its avatar can't be customized until one exists
- Persisted values are only re-read on mount; an external write to the `ui_state` table while the app runs is not observed

## Important Touch Points

- `src/lib/project-image.ts` — `resolveImageUrl(input, cacheBust)` classification + favicon derivation
- `src/components/ui/project-avatar.tsx` — `ProjectAvatar` render component (image/color/letter precedence, retry-on-URL-change)
- `src/stores/project-appearance-store.ts` — shared appearance state, deduped reads, write-through persistence
- `src/components/layout/project-appearance-menu.tsx` — the `Project "<name>"` submenu + `PROJECT_COLORS`
- `src/components/layout/workspace-inbox-menu.tsx` — wires the submenu into the workspace context menu and owns `ProjectImageDialog`
- `src/components/layout/use-project-appearance.ts` — read hook over the store, used by every avatar surface
- `src/components/overlays/project-image-dialog.tsx` — image/URL input dialog with live preview
- `src/components/overlays/project-picker.tsx` — second render surface for the avatar
- `src/components/layout/sidebar-project-group.tsx` — the original project-tree menu (**unmounted** since PR #198; still imports `PROJECT_COLORS` from the shared module so the palette can't drift)

## Notes

- Keep this file about current truth, not future plans.
