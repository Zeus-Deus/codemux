# Step 13 — Agent Chat Beta Toggle: UI Smoke Checklist

- Purpose: Operator verification of Step 13's master Beta toggle. Run
  this end-to-end before declaring the merge-readiness gate green.
- Audience: The operator booting the dev build to verify the toggle.
- Authority: Follow this list verbatim before push to `main`.
- Read next: `docs/plans/step-13-beta-toggle-research.md` for the
  scoping rationale, `docs/plans/main-merge-plan.md` for the merge
  context this gate sits behind.

## Setup

1. Confirm working tree is on `feature/agent-chat` at the post-merge
   commit (run `git log -1 --format='%H %s'` — should show the most
   recent Step 13 implementation commit; merge commit was `d3bd336`).
2. Snapshot your current `~/.codemux/observability.json` (or
   `~/.local/share/codemux/observability.json` depending on platform)
   so you can restore your real flag state after smoke testing.
3. `npm run tauri dev` to launch the dev build.

## Path A — Fresh-install OFF default

Tests that a brand-new user (no persisted observability.json) lands
on the legacy main-branch experience.

1. Quit Codemux. `mv ~/.codemux/observability.json
   ~/.codemux/observability.json.bak` so the next launch sees no file.
2. `npm run tauri dev` again.
3. **Verify**:
   - The empty-state splash shows "Open Project / New Project"
     buttons — *not* the chat-home landing.
   - Sidebar header `+` button on a fresh workspace opens the legacy
     New Workspace dialog directly. No home draft is created.
   - Settings (`Cmd/Ctrl+,`) opens with these top-of-sidebar nav
     groups in order:
     - **BETA FEATURES** with one row: "Agent Chat" (Sparkles icon).
     - **PERSONAL** with Account / Appearance / Notifications /
       Shortcuts.
     - **EDITOR & WORKFLOW** with Editor / Terminal / Presets /
       Projects / Git / Agent / **Browser** / Session Restore.
   - **Permissions, Skills, MCP Servers rows are NOT visible** under
     Editor & Workflow.
   - Account section does not show a "Skills sync" subsection.
   - Click the "Agent Chat" row → BetaFeaturesSection renders:
     - Sparkles icon + "BETA FEATURES" warning-colored header.
     - Card with "Agent Chat" + amber "Beta" badge.
     - Switch is in the OFF state.
     - "What's included →" expands to a 8-bullet list (chat interface,
       MCP, skills sync, mode pills, attachments, model picker,
       permissions, home-screen chat).
4. Restart with the backup file: `mv ~/.codemux/observability.json.bak
   ~/.codemux/observability.json` to restore your previous state.

## Path B — Toggle ON

Verifies the toggle wires through correctly.

1. From the OFF state above, toggle the Switch ON.
2. **Verify**:
   - A success toast: "Agent Chat enabled — Codemux will close.
     Reopen to apply."
   - After ~600ms the Codemux window closes (the toast is visible
     long enough for the user to read it before the window goes).
   - The flag IS persisted to `~/.codemux/observability.json` —
     check the file: `enable_agent_chat: true`,
     `enable_lazy_workspace_creation: true`. (Even if the user kills
     the process before the scheduled close fires, the flag is safe
     on disk.)
3. Reopen Codemux (double-click the launcher / re-run
   `npm run tauri dev` / re-run the AppImage / etc.).
4. **Verify** on the next boot:
   - The empty-state is replaced by the chat-home landing (or the
     workspace draft surface depending on whether workspaces exist).
   - Sidebar `+` button on the workspace creates a chat draft instead
     of opening the legacy dialog.
   - Settings sidebar shows **Permissions, Skills, MCP Servers** under
     Editor & Workflow.
   - Account section shows the "Skills sync" subsection.
   - The chat-agent preset row is back in the preset bar.
5. Open a chat pane and send a turn — streaming UI and approvals
   should work as on the branch's flag-on path.

### Why a manual reopen instead of auto-restart?

We tried auto-restart (detached spawn via setsid + DETACHED_PROCESS
+ /dev/null stdio + control-socket teardown). It works in production
builds, but the dev-server WebView path can't survive the cargo
runner exiting (the new child loads into a dead Vite server and gets
a black screen). A "dev: warn / prod: auto-restart" split worked
mechanically but produced inconsistent UX between dev and prod, so
both modes ship the same plain-quit flow. The honest UX wins: every
user — dogfooder and shipping user — sees the same toast and
manually reopens the app, guaranteeing a clean fresh boot under the
new flag state with no edge cases.
   - Sidebar `+` button on the workspace now creates a chat draft
     instead of opening the legacy dialog (Shift+click still falls
     back to the dialog).
   - Settings sidebar now shows **Permissions, Skills, MCP Servers**
     under Editor & Workflow.
   - Account section now shows the "Skills sync" subsection.
   - The chat-agent preset row is back in the preset bar (preset bar
     is at top of any workspace view).
3. Open a chat pane via the sidebar header chat icon (or
   Shift+click a chat-agent preset). Send a turn — the streaming UI
   and approvals should work as they do today on the branch.

## Path C — Toggle OFF with persisted chat panes

Verifies data preservation + pane placeholder.

1. From the ON state above, with at least one open chat pane, toggle
   the Switch OFF in Settings → Beta Features.
2. **Verify**:
   - Toast: "Agent Chat disabled — Codemux will close. Reopen to apply."
   - App closes after ~600ms.
3. Reopen Codemux. After the boot:
   - The previously-open chat pane is replaced by a centered
     placeholder card: Sparkles icon, "Agent Chat is disabled" title,
     "Your data is preserved" copy, and an "Open Settings → Beta
     Features" button.
   - Clicking the placeholder's CTA opens Settings on the
     BetaFeaturesSection page directly.
4. Re-enable via the Switch, manually reopen. After the boot:
   - The chat pane re-mounts at the same `pane_id` with its prior
     `thread_id` intact. Session history (if any) is still in the
     SessionSelector dropdown.

## Path D — Existing user with persisted ON state

Verifies the merge-friendly behavior: dogfooders who flipped the
flag on during this session should keep their state across the
default-flip, with the toggle starting in ON.

1. Quit Codemux. Edit `~/.codemux/observability.json`: set both
   `enable_agent_chat` and `enable_lazy_workspace_creation` to
   `true` under `feature_flags`.
2. Relaunch.
3. **Verify**:
   - App boots straight into the chat-home landing (or workspace
     draft surface) — the persisted ON state wins over the default.
   - Settings → Beta Features → the Switch starts in the ON state.
4. Toggle OFF in Settings, reload, verify Path C behavior, then
   toggle back ON. State should round-trip cleanly.

## Path E — Backend-leak verification (optional, more invasive)

Verifies that the four leaks plugged in Step 13 actually no-op when
the flag is off. Requires opening Tauri devtools + observing
network/process state.

1. From a fresh-install OFF state (Path A):
   - Open devtools, watch the console. Boot should NOT print any
     "Provider capabilities refresh" or "Skills sync" log lines.
   - `ps -ef | grep opencode` should show no `opencode serve`
     children — the OpenCode supervisor stays unspawned.
   - `ls -la ~/.local/share/codemux/sync-key.enc` may or may not
     exist depending on prior auth flow, but no new sync requests
     should fire — check the Network tab for `api.codemux.org/api/skills`
     URLs (should be zero).
2. Toggle ON via Path B, verify the OpenCode child eventually
   spawns when the picker is opened.

## Sign-off

Before declaring the toggle ready:

- [ ] Path A passes — fresh install lands on EmptyState, all Beta
      surfaces hidden.
- [ ] Path B passes — toggle ON brings up every Step 6–12 surface
      and the data flows correctly.
- [ ] Path C passes — toggle OFF replaces chat panes with placeholders;
      re-enabling restores them losslessly.
- [ ] Path D passes — existing-user persisted ON state survives
      app restart.
- [ ] Path E passes — no background work fires when the flag is
      off.
- [ ] No console errors at any step.
- [ ] Restored your original `observability.json` before quitting.

## Known caveats (out of scope for Step 13)

- Sidebar header label still reads "New Agent" / "New chat in home
  directory" tooltip text even when the flag is off (research §
  "Cosmetic loose ends"). Cosmetic only — the flag-gated behavior
  itself is correct.
- Toggling off mid-session leaves spawned `opencode serve` /
  `claude-agent-sidecar` children alive until app close. Acceptable
  per research §5; the registry stops receiving commands and on
  next boot the registry is not re-initialised.
