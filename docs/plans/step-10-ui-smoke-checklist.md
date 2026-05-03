# Step 10 — Manual UI Smoke Checklist

- Purpose: Operator-driven walkthrough that exercises the visible surface of skills sync end-to-end. Run after a fresh `npm run dev` build.
- Audience: Anyone validating a Step 10 release or debugging a regression report.
- Authority: Manual verification protocol. Programmatic coverage lives in the Vitest suite + `examples/skills_smoke.rs` + `examples/stage5_smoke.rs`; this checklist is the human-driven complement.
- Update when: A user-facing flow changes shape (new button, removed step, renamed surface).
- Read next: `docs/features/skills-sync.md` for the canonical behavior, `docs/plans/step-10-skills-sync.md` for per-stage history.

## Prerequisites

- A test or low-stakes Codemux account. **Don't test the reset flow on a real account.**
- A clean `~/.codemux/skills/` directory (or willingness to add a temporary skill there).
- Network access — the manual sync flows go through `api.codemux.org`.

## Setup

- [ ] `npm run dev` builds and launches the Tauri shell.
- [ ] Sign in (GitHub OAuth or email/password).
- [ ] Open Settings → Account.
- [ ] Confirm a "Skills sync" subsection is visible between the user info and the Sign-out button.

## Sync status indicator

- [ ] Sync section shows the current state ("Sync ready" / "Syncing…" / "Sync error") with the matching icon (green check / spinner / red triangle).
- [ ] When idle and at least one sync has run, "Last synced N {minutes,hours,days} ago" appears below the state label.
- [ ] Wait 60 seconds while the Settings panel stays open. The relative-time string ticks forward (e.g. "just now" → "1 minute ago") without re-fetching.

## Push trigger (file watcher)

- [ ] Create `~/.codemux/skills/test-sync/SKILL.md` with arbitrary content (`mkdir -p ~/.codemux/skills/test-sync && cat > ~/.codemux/skills/test-sync/SKILL.md`).
- [ ] Within ~3 seconds (300 ms watcher debounce + 1.5 s frontend debounce + sync time), the status flips Idle → Syncing → Idle.
- [ ] "Last synced just now" shows immediately after the cycle completes.

## Manual sync

- [ ] Click "Sync now".
- [ ] The button disables immediately (optimistic state) and the icon swaps to a spinner.
- [ ] When the cycle completes, the button re-enables and the relative-time updates to "just now".

## Error handling

- [ ] Disconnect the network (toggle Wi-Fi or pull the cable).
- [ ] Click "Sync now". Status flips to "Sync error" with a red banner explaining the failure.
- [ ] The button text changes to "Retry".
- [ ] Reconnect the network and click "Retry". Status returns to Idle.

## Export

- [ ] In the "Sync ready" row, click "Export skills locally".
- [ ] The OS save dialog opens with a default filename like `codemux-skills-export-2026-04-29.json`.
- [ ] Save to a chosen path. A status line appears: "Exported N skills to /chosen/path".
- [ ] Open the JSON file in a text editor. Verify:
  - [ ] Top-level fields: `version`, `exported_at`, `user_email`, `product` (= "codemux"), `skill_count`, `skills`.
  - [ ] At least one entry in `skills` with `name`, `content`, `provider`, `scope`, `updated_at`.
  - [ ] Cancel the dialog instead of saving — the action silently no-ops (no toast).

## Import

- [ ] Delete `~/.codemux/skills/test-sync/` (`rm -rf ~/.codemux/skills/test-sync`). Wait briefly so the file watcher's "deleted" event passes (note: Step 10 v1 does not auto-wipe the remote on local delete; the server still has the encrypted blob).
- [ ] Click "Import skills from backup". Pick the JSON file from the previous export.
- [ ] Status line: "Re-pushed N skill(s)".
- [ ] After the next sync cycle (manual click or wait for periodic), `~/.codemux/skills/test-sync/SKILL.md` reappears with the original content.

## Email-mismatch warning

- [ ] If you have access to two test accounts, export from account A, sign out, sign in as account B, then import the file. The status line includes a "(note: backup belongs to a different account)" qualifier. The import still proceeds — the warning is soft.

## Reset password (do NOT actually reset on a real account)

- [ ] Click "Forgot your sync password?". The multi-step dialog opens.
- [ ] Step 1 (warn) shows: "permanently delete every synced skill from the server" copy + three buttons (Cancel / I don't need a backup / Export skills first).
- [ ] Click "Export skills first". OS save dialog → confirm export → "Backup saved" step.
- [ ] At "Backup saved", click Cancel — the dialog closes without wiping.
- [ ] Re-open the dialog and click "I don't need a backup" instead. Step 2b shows the acknowledgment checkbox; the destructive button stays disabled until checked.
- [ ] Click Cancel from the skip-confirm step.
- [ ] (Only on a test account) walk through to completion: click the destructive button → status flips to "Wiping…" → "Check your email" with the user's email address shown → close.
- [ ] (Only on a test account) verify a reset email arrived. Open the link in a browser, set a new password via the existing reset-password page, then sign back into Codemux with the new password. Open Settings → Sync → "Import skills from backup" and pick the export from earlier; skills are re-pushed under the new key.

## Sign-out cleanup

- [ ] Sign out from Settings.
- [ ] Verify `~/.local/share/codemux/sync-key.enc` no longer exists (`ls ~/.local/share/codemux/sync-key.enc` → "No such file").
- [ ] Verify `~/.codemux/skills/` files are NOT deleted — sign-out wipes the encryption key but preserves the user's local skills.

## Periodic sync (long-running observation)

- [ ] With the app foreground, leave Settings → Sync open and idle for 5 minutes. The relative-time string keeps advancing; no error appears.
- [ ] (Optional verification) On another device or via curl, push an `/api/skills` change. Within 5 minutes, the foreground app should automatically pull it. Verify the local file appears.
- [ ] Background the app for >5 minutes (`alt-tab` away or minimize). The periodic timer pauses (visibility-gated). When you return focus, the next interval picks up where it left off.

## Cleanup

- [ ] Remove the test skill: `rm -rf ~/.codemux/skills/test-sync`.
- [ ] If you used a temporary test account, sign out and email `support@codemux.org` to delete the account (no in-app full-erasure flow yet — that's a Stage 6+ polish).
