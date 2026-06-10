# Project Avatars

- Purpose: Describe the current capability and constraints of sidebar project avatar customization (image + accent color).
- Audience: Anyone working on sidebar project identity or the project picker UI.
- Authority: Canonical feature-level reality doc.
- Update when: Avatar resolution, persistence keys, or picker behavior changes.
- Read next: `docs/features/workspace-creation.md` (project onboarding), `docs/reference/FEATURES.md`

## What This Feature Is

Every project group in the left sidebar renders a small avatar (`ProjectAvatar`). By default it shows the project's first letter; users can customize it per project with an image (logo or website favicon) and/or an accent color from the project group's context menu. The same avatar component renders in the project picker overlay.

## Current Model

- Right-click a sidebar project group → the context menu offers a fixed 12-color palette (Red, Orange, Yellow, Lime, Green, Teal, Cyan, Blue, Indigo, Purple, Pink, Slate — `PROJECT_COLORS` in `sidebar-project-group.tsx`) plus an image entry that opens `ProjectImageDialog`.
- The dialog accepts three input shapes, classified by `resolveImageUrl` in `src/lib/project-image.ts`:
  - a **direct image URL** (recognized by extension: png/jpe?g/gif/svg/webp/ico/avif/bmp) — passed through untouched
  - a **`data:` URL** — passed through untouched
  - a **website URL or bare domain** — routed through Google's favicon service (`https://www.google.com/s2/favicons?domain=<domain>&sz=128`), which handles redirects, multiple favicon sizes, and sites without a root `/favicon.ico`
- **Favicon cache-busting** (PR #71, shipped `v0.8.0`): each save stamps a `Date.now()` version persisted next to the image input; `resolveImageUrl(input, cacheBust)` appends it to *derived favicon URLs only* as `&v=`, so a site whose favicon changed visibly refreshes instead of the WebView serving the same cached bytes forever. Direct/data URLs are never modified (never corrupt a signed or already-complete URL).
- **Persistence** is per-project UI state in the desktop SQLite DB via `dbSetUiState`/`dbGetUiState`, keyed by project path: `project.image:<path>`, `project.image.v:<path>` (the cache-bust token), `project.color:<path>`. Clearing writes an empty string. Device-local only — not part of synced settings.
- **Render precedence** in `ProjectAvatar`: image → color disc → first-letter fallback. A failed image load falls back to the letter, and the failure flag resets whenever the resolved URL changes (new image or fresh cache-bust token) so one transient failure doesn't pin the fallback. Supports `sm`/`md`/`lg` sizes and circle/square shapes.

## What Works Today

- Per-project custom image via direct URL, data URL, or domain-derived favicon
- Per-project accent color from a 12-color palette; color also tints the letter fallback
- Re-saving (or re-opening the picker for) a website image re-fetches the favicon via the `&v=` cache-bust token
- Clear/reset back to the letter fallback from the same context menu
- Avatar renders consistently in the sidebar project group header and the project picker overlay

## Current Constraints

- Favicon derivation depends on Google's public favicon service — needs network, and the service controls quality/size (requested at 128px)
- The color palette is fixed (no custom hex input)
- Persistence is device-local UI state; avatars do not sync across devices
- No local image file picker — input is URL/domain text only (data URLs work but must be pasted)

## Important Touch Points

- `src/lib/project-image.ts` — `resolveImageUrl(input, cacheBust)` classification + favicon derivation
- `src/components/ui/project-avatar.tsx` — `ProjectAvatar` render component (image/color/letter precedence, retry-on-URL-change)
- `src/components/layout/sidebar-project-group.tsx` — context menu, `PROJECT_COLORS`, save/clear handlers, `dbGetUiState`/`dbSetUiState` persistence
- `src/components/overlays/project-image-dialog.tsx` — image/URL input dialog with live preview
- `src/components/overlays/project-picker.tsx` — second render surface for the avatar

## Notes

- Keep this file about current truth, not future plans.
