# Auto-Update

- Purpose: Describe the in-app update checking and installation system.
- Audience: Anyone working on the update flow, release pipeline, or troubleshooting update issues.
- Authority: Canonical feature doc for auto-update.
- Update when: Update detection, download, installation, or notification behavior changes.
- Read next: `docs/core/STATUS.md`

## What This Feature Is

Codemux checks for new releases in the background and shows a toast notification when an update is available. On AppImage builds, users can download and install updates without leaving the app.

## Current Model

The `useUpdateChecker` hook runs a background check using Tauri's updater plugin. It checks on startup (after a 5-second delay) and every 4 hours. When an update is found, a persistent toast notification appears with version info and action buttons.

The flow depends on the package format:
- **AppImage**: full in-app update — download with progress bar, then restart to apply
- **Other formats**: directs the user to the GitHub releases page for manual download

Dismissals are per-version and deliberately **in-memory only** (a `useRef` in `use-update-checker.ts`, not localStorage), so dismissing v0.1.13 won't suppress v0.1.14 — and a dismissed toast reappears on the next app launch.

## What Works Today

- background update checking (4-hour interval, 5-second initial delay)
- persistent toast notification with version number
- download progress bar for AppImage builds
- restart-to-apply for AppImage builds
- external download link for non-AppImage builds
- per-version dismiss (dismissing one version does not suppress future versions)
- skipped in development mode (`import.meta.env.DEV`)

## Current Constraints

- AppImage is the only format supporting in-app auto-update
- no release notes shown in the notification (just version number)
- no rollback mechanism
- no update channel selection (stable only)

## Important Touch Points

- `src/hooks/use-update-checker.ts` — update check logic, download, relaunch
- `src/components/update/update-toast.tsx` — toast notification UI
- `src/tauri/commands.ts` — `getPackageFormat()` command wrapper
- `src-tauri/src/commands/update.rs` — package format detection
- `src-tauri/tauri.conf.json` — updater plugin configuration and endpoints
