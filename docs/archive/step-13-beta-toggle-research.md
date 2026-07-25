# Step 13 — Agent Chat Beta Toggle: Pre-Merge Scoping

> **ARCHIVED.** This plan's work has landed; it is kept as the implementation
> record and reasoning trail, not as current truth. For how this behaves today
> read the relevant `docs/features/*` doc (see `docs/INDEX.md`).

- Purpose: Scope a single Settings toggle "Agent Chat (Beta)" that
  controls every Step 6–12 GUI surface so the merge to `main` can ship
  with the new behaviour OFF by default and the legacy behaviour
  preserved for users who don't opt in.
- Audience: The implementer of Step 13 and reviewers deciding whether
  to merge `feature/agent-chat` to `main` once the toggle lands.
- Authority: Pre-merge research only — **no code in this stage**.
  Step 13 has since LANDED; the file inventories below are a
  **point-in-time snapshot taken before the merge** and are no longer
  maintained (e.g. `pickers/WorktreePicker.tsx` was deleted by PR #142,
  `SubagentActivityBar.tsx` was added by PR #143). Treat them as
  history. For the current flag surface and file map, read
  `docs/features/agent-chat.md` and `docs/features/gui-chrome.md`.
- Update when: Do not update. Superseded by the feature docs above.
- Read next: `docs/features/agent-chat.md`,
  `docs/features/multi-provider-chat.md`,
  `docs/features/skills-sync.md`, `docs/features/mcp-server.md`.

## Headline

The toggle does not need to be invented from scratch. The codebase
already carries two persisted feature flags
(`enable_agent_chat`, `enable_lazy_workspace_creation`) that gate
~95 % of the Step 6–12 surface, and most of the Off paths are already
wired and unit-tested. The work is mostly **closing four leaks**
(default values, eager OpenCode boot, skills-sync auto-pull, settings
nav visibility), unifying the two flags into one user-facing toggle,
and adding a Settings UI for it. The home-screen revert is already
clean — `EmptyState` on the branch is byte-identical to `main`'s
fallback path when both flags are off.

The honest estimate is **~1.5–2 sessions** of implementation work, at
roughly Step 11's complexity tier (smaller than Step 10 or Step 12).

## 1 — Toggle Entry Points (exhaustive)

Every place where the Step 6–12 surface diverges from `main`. File:line
plus the divergence kind. "Already gated" means the existing
`enableAgentChat` / `enableLazyWorkspaceCreation` flag check is in
place and tested; "leak" means the surface lights up regardless of
the flag state.

### App shell / routing

- `src/components/layout/app-shell.tsx:71` — empty-state guard now
  reads `if (!hasWorkspaces && !(lazyEnabled && hasActiveDraft))`.
  **Already gated.** When `enableLazyWorkspaceCreation` is off this
  collapses to `if (!hasWorkspaces)` — byte-identical to `main`.
- `src/components/layout/workspace-main.tsx:111-129` — early-return
  branch that renders `DraftChatSurface` when a client-side draft is
  active. **Already gated** on `enableLazyWorkspaceCreation`.
- `src/App.tsx:60-61` — calls `useProviderCapabilitiesInit()` and
  `useEnsureDraftWhenEmpty()` unconditionally on every boot.
  **Mixed**: `useEnsureDraftWhenEmpty` already self-gates on both
  flags (`use-ensure-draft-when-empty.ts:48`), but
  `useProviderCapabilitiesInit` is a **leak** — see §"Leaks" below.

### Sidebar plus-buttons (workspace creation)

- `src/components/layout/sidebar-header.tsx:13-71` — the "New Agent"
  row. Plain click → home draft when both flags on; Shift+click or
  agent-chat off → legacy `setShowDialog`. **Already gated**, with
  an explicit `enableAgentChat ? "New chat in home directory ·
  Shift+click for workspace dialog" : ...` tooltip. Note: the row
  *label* still reads "New Agent" even when the flag is off — see
  §"Cosmetic loose ends" below.
- `src/components/layout/sidebar-project-group.tsx:90-154` — per-
  project "+" button. Plain click → project draft when both flags on;
  flag off → legacy dialog. **Already gated**, with the symmetric
  `enableAgentChat ? "New workspace · Shift+click for CLI" : "New
  workspace"` tooltip.
- `src/components/layout/sidebar-workspace-row.tsx:255-263` — workspace
  row click is `activateWorkspace(...)` only. **No divergence.**
  Always opens the legacy workspace, regardless of flag state. The
  `useChatDraftStore.getState().setActiveDraft(null)` on the same
  line is a no-op when no draft exists, so it remains safe.

### Pane tree

- `src/components/layout/PaneNode.tsx:284-314` — renders
  `AgentChatPane` when `node.kind === "agent_chat"`. **Leak.** No
  flag check; if a persisted layout from a flag-on session contains
  agent-chat panes, they render even after the flag is flipped off.
  Mitigation discussed in §5 (User Data Preservation) and §"Leaks".
- `src-tauri/src/state/state_impl.rs:295-940` — `AppStateStore`
  exposes `create_agent_chat_pane`, `agent_chat_thread_id`, etc.
  **Backend layer**, not user-visible. The Tauri command surface is
  already gated; the state APIs themselves don't need a gate.

### Tauri command surface

- `src-tauri/src/commands/agent_chat.rs:48-60` — `feature_flag_on`
  helper; every chat lifecycle command (`agent_chat_create_pane`,
  `_close_pane`, `_start_session`, `_send_turn`, `_interrupt_turn`,
  `_respond_to_request`, `_set_model`, `_set_permission_mode`,
  `_stop_session`) returns `feature_disabled: enable_agent_chat is
  off` when the flag is off. **Already gated.**
- `src-tauri/src/commands/agent_chat.rs:505-548` —
  `list_chat_provider_capabilities` is **explicitly NOT gated** by
  design comment ("the frontend's capabilities store refreshes at
  app boot regardless"). **Leak.** This is what spawns
  `opencode serve` lazily and harvests Codex via the
  `CodexCapabilityCache`. With the flag off this is wasted work and
  surfaces unwanted background-process activity.
- `src-tauri/src/commands/opencode.rs` — `opencode_check_availability`,
  `opencode_ping`, `opencode_list_models`. Comments admit "integration
  is gated by the `enable_agent_chat` flag at the picker", but the
  commands themselves are not gated. **Leak** for the same reason.
- `src-tauri/src/commands/agent_chat.rs:212` —
  `dev_agent_chat_spawn_test_pane` is dev-only and **already gated**.

### Provider runtime (backend boot)

- `src-tauri/src/lib.rs:520-603` — Claude / Codex / OpenCode adapters
  are spawned into `ProviderRegistry` only when
  `observability.agent_chat_enabled()` is true. **Already gated.**
  When the flag is off the registry stays empty, the sidecar binary
  is never spawned, and the OpenCode supervisor does not pre-warm.
  This is the cleanest part of the existing implementation.

### Composer / chat panes (Step 6–12 visual surface)

All of these live under `src/components/chat/` and are reached only
via the gated entry points above (chat-pane render, draft surface,
preset launch). They do not need their own flag checks because they
cannot mount when the flag is off. Touch points for reference:

- `AgentChatPane.tsx`, `AgentChatPaneHeader.tsx` — pane chrome.
- `Composer.tsx`, `ComposerFooter.tsx`, `DraftChatSurface.tsx` —
  composer / draft surface.
- `pickers/MultiProviderModelPicker.tsx` (Step 12),
  `pickers/ModelPicker.tsx`, `pickers/WorktreePicker.tsx` —
  multi-provider rail + legacy single-provider picker.
- `MessageList.tsx`, `SessionSelector.tsx`, `AttachmentChip.tsx`,
  `DebugCleanupBanner.tsx`, `DebugExitDialog.tsx`,
  `provider-logo.tsx` — message rendering / approvals / dev affordances.
- `pickers/MultiProviderModelPicker.tsx:49`,
  `AgentChatPane.tsx:103` — `ENABLE_PROVIDER_PICKER = true` (now
  hard-coded `true` after Step 12 Stage 4 flipped it). **Vestigial
  flag** — not a real toggle anymore; should be deleted in Step 13's
  cleanup pass since the master toggle subsumes it.

### Settings sections (Step 6–12 added these)

`src/components/settings/settings-view.tsx:112-143` defines the
`Section` union and `NAV_GROUPS`. The Step 6–12 additions vs `main`:

- `"permissions"` (line 136) — Permissions section, Step 9 era.
  Renders `PermissionsSection` (line 1119).
- `"skills"` (line 137) — Step 10's Skills section. Renders
  `SkillsSection` (line 1122).
- `"mcp"` (line 138) — Step 9's MCP Servers section. Renders
  `McpSection` (line 1125).
- `"agent"` (line 135) — pre-existed on `main` with only the
  "auto-configure MCP" toggle, **not new from Steps 6–12**. Stays
  visible regardless of the master toggle.
- Account section, lines 749-753 — embeds `<SyncSection />` (Step 10
  skills-sync UI). New surface, needs gating.

**All Step 6–12 settings entries are unconditionally rendered today.**
None of them check `enableAgentChat`. This is the single largest
visible-leak cluster.

### Preset bar

- `src/components/layout/preset-bar.tsx:232-258` — handles
  `preset.kind === "chat_agent"` clicks (the built-in "Chat Agent"
  preset added during Step 6–12). Currently shown unconditionally in
  the preset bar. **Leak**: when the toggle is off, the chat-agent
  preset row should be hidden so a user who never opts in never sees
  the affordance. Note: `src/components/overlays/new-workspace-dialog.tsx:237`
  already filters the CLI launcher's preset list to `kind === "cli"`,
  so the dialog itself is fine — only the preset bar row leaks.
- `src-tauri/src/presets.rs:198` registers the `chat_agent` built-in
  preset. The preset still exists in the store regardless of the
  flag; we simply hide it at the UI layer.

### Hooks / background work

- `src/hooks/use-ensure-draft-when-empty.ts:48` — early-returns when
  either flag is off. **Already gated.**
- `src/hooks/use-skills-sync.ts:67-138` — auto-fires `skillsSyncNow`
  on auth, file-watcher events, and a 5-min interval **regardless of
  any flag**. **Leak.** Skills sync was added in Step 10 and is
  scoped under the master toggle per the user's brief. The
  `~/.local/share/codemux/sync-key.enc` keyfile and any synced
  blobs should keep persisting (data preservation §5), but the
  background pull/push should pause when the flag is off so a
  toggled-off user doesn't burn API calls or surprise the user with
  "skills appearing".
- `src/components/debug/SpawnChatPaneButton.tsx:26-30` — already
  self-gates on `import.meta.env.DEV && enableAgentChat`. **No
  leak.**
- `src/components/layout/title-bar.tsx:322` — mounts
  `<SpawnChatPaneButton />`, which self-gates. **No leak.**

### Frontend project actions

- `src/hooks/use-project-actions.ts:21-23` — `shouldSkipOnboarding()`
  reads `flags.enableAgentChat && flags.enableLazyWorkspaceCreation`.
  **Already gated.** When either flag is off, the legacy
  `ProjectOnboarding` wizard runs as it does on `main`.
- `src/components/overlays/new-project-screen.tsx:118-122` —
  `skipOnboarding = flags.enableAgentChat &&
  flags.enableLazyWorkspaceCreation`. **Already gated.**

## 2 — Current Dev-Flag Map

Today the codebase carries the following flags. Step 13's job is to
collapse them into one user-facing master.

| Flag | Where defined | Persistence | Default | Controls |
|---|---|---|---|---|
| `FeatureFlags.enable_agent_chat` (Rust) | `src-tauri/src/observability.rs:34-58` | `~/.local/share/codemux/observability.json` (per-machine) | **`true` in `default_snapshot`** at `observability.rs:259` (BUT `#[serde(default)]` falls back to `false` on missing field). **Inconsistent — see §"Risks".** | All gated chat lifecycle commands; backend provider registry init; `feature_flag_on` helper. |
| `FeatureFlags.enable_lazy_workspace_creation` (Rust) | `observability.rs:48-58` | Same observability.json | Same — `true` in `default_snapshot` (`observability.rs:260`), `false` on `serde(default)`. | The home-draft empty-state replacement, sidebar "+" button draft creation, project-screen onboarding skip. |
| `useFeatureFlags.enableAgentChat` (TS) | `src/stores/feature-flags.ts:9, 25` | Mirror of backend, refreshed at App mount. | Initial state `false`; `refresh()` overwrites with backend value. Error fallback `false`. | Frontend gates listed in §1 above. |
| `useFeatureFlags.enableLazyWorkspaceCreation` (TS) | `feature-flags.ts:13, 26` | Same mirror. | Same. | Same. |
| `ENABLE_PROVIDER_PICKER` (TS) | `src/components/chat/AgentChatPane.tsx:103` | Compile-time constant. | `true` (Step 12 Stage 4 flipped it from false). | Switches `ComposerFooter` between `MultiProviderModelPicker` and the legacy single-provider `ModelPicker`. |
| `unstable_openflow`, `unstable_browser_automation`, `unstable_indexing` (Rust) | `observability.rs:35-37` | Same observability.json. | All `true` in `default_snapshot`. | **Pre-existed on `main`** and unrelated to Step 6–12. Out of scope for Step 13. |

Recommendations for Step 13:

1. **Unify** `enable_agent_chat` and `enable_lazy_workspace_creation`
   into a single user-facing toggle. The branch ships them as
   independent flags only because Stage C (lazy creation) shipped
   after Stage B (chat panes); the two-flag matrix made dogfooding
   incremental, but every production path that reads them does an
   AND, never an XOR. Concretely: keep the two backend fields for
   wire-compat with existing observability.json files, but make the
   Settings UI write both to the same value, and rename the
   user-facing label to "Agent Chat (Beta)".
2. **Delete `ENABLE_PROVIDER_PICKER`** (already always `true`).
3. **Fix the default mismatch** between `default_snapshot()`
   (currently `true`) and `#[serde(default)]` (currently `false`).
   The intended new-user default is OFF — flip the
   `default_snapshot()` literals to `false`. See §"Risks".

## 3 — Home Screen / App Shell Entanglement

Lower risk than the brief implied. The git diff is small and
ablation-tested.

### Today's behaviour (toggle ON)

- App boot resolves auth → renders `<App>` → calls
  `useEnsureDraftWhenEmpty()` (App.tsx:61).
- If no workspaces and homeDir resolved →
  `getOrCreateHomeDraft()` creates an in-memory draft and sets it
  active.
- `<AppShell />` reads `lazyEnabled && hasActiveDraft`. Because both
  are true, it bypasses `<EmptyState />` and renders the normal app
  shell (sidebar + title bar + `<WorkspaceMain />`).
- `<WorkspaceMain />` reads `lazyEnabled && activeDraftId`, hits
  the early-return at line 116, and renders
  `<PresetBar workspaceId={null} draftId={...} />` plus
  `<DraftChatSurface />`.

### Pre-Step-7 behaviour (the target for toggle OFF)

`git show main:src/components/layout/app-shell.tsx` (lines 60-66
on main) confirms the pre-Step-7 fallback:

```tsx
if (!hasWorkspaces) {
  return <EmptyState />;
}
```

`<EmptyState />` (`src/components/layout/empty-state.tsx`) is the
"Open Project / New Project" splash. **It still exists on the branch
unmodified** — `git diff main..feature/agent-chat -- src/components/layout/empty-state.tsx`
is empty.

### Why the entanglement is shallow

The branch's app-shell change is exactly two lines: the import of
`useChatDraftStore` and `useFeatureFlags`, plus the guard
augmentation at line 71 that adds `&& !(lazyEnabled && hasActiveDraft)`.
**With both flags off, that augmentation evaluates to `false &&
hasActiveDraft = false`, the guard collapses to `if (!hasWorkspaces)`,
and the user sees `<EmptyState />` byte-for-byte.**

The same is true of `WorkspaceMain.tsx:116` — the lazy branch is
gated on `lazyEnabled`, so off → falls through to the workspace
render path that exists on `main`.

The same is true of `useEnsureDraftWhenEmpty` — line 48 short-circuits
both off paths, so no auto-spawn fires.

### What concretely needs to happen for the OFF revert

Nothing in `app-shell.tsx`, `workspace-main.tsx`, or `empty-state.tsx`
needs to change. Setting both backend flags to `false` and ensuring
the frontend reads them is **already** the correct revert.

The only home-screen-adjacent leak is **stale persisted state**: a
user who used flag-on in dogfooding and persisted a layout containing
agent-chat panes. When the flag flips off, those panes still
render (PaneNode.tsx:284-314, see §1 "Pane tree leak"). Mitigation in
§5.

## 4 — Click-Handler Revert Plan

Already done. Verified by reading the existing code and unit tests.

The "+" buttons in `sidebar-header.tsx:22` and
`sidebar-project-group.tsx:132` already match the user's spec:

```tsx
if (e.shiftKey || !enableAgentChat) {
  setShowDialog(true);
  return;
}
```

i.e. **plain-click with the flag off → legacy New Workspace Dialog**.
This is the pre-Step-7 behaviour. Test coverage:

- `src/components/layout/sidebar-header.test.tsx:81-95` — "flag OFF +
  plain click → opens NewWorkspaceDialog", "flag OFF + Shift+click →
  opens NewWorkspaceDialog".
- `src/components/layout/sidebar-workspace.test.tsx:159-175` — same
  asserts at the project-group level.

Workspace row click (`sidebar-workspace-row.tsx:255-263`) is unchanged
from `main` — it's just `activateWorkspace(...)` and an idempotent
draft clear. No flag check needed.

Once the master toggle ships, the only refactor is to **swap the
flag-name** in the click handler: today it reads `enableAgentChat`;
after unification, it reads the master toggle (whichever name we
pick — see §6).

## 5 — User Data Preservation Behaviour

Recommended posture: **hide, never delete.** All Step 6–12 user data
keeps persisting in the background; the toggle only controls the
visible surface and active background work. This matches Codemux's
conventions for `unstable_*` flags.

| Data | Storage | Recommended OFF behaviour |
|---|---|---|
| Chat sessions / messages | `agent_chat_sessions` + `agent_chat_messages` SQLite tables (`src-tauri/src/database.rs:140-162`) | Keep on disk untouched. The data is unreachable from the UI when chat panes are gone, but flipping the toggle back on instantly reveals every prior session via the existing `SessionSelector` history. |
| Draft state | `useChatDraftStore` zustand (in-memory) | Cleared on app close anyway — no action needed. When the flag flips off mid-session, also call `useChatDraftStore.getState().setActiveDraft(null)` so the lazy-branch in WorkspaceMain disengages immediately. |
| Persisted layout containing `agent_chat` panes | Same persisted-layout SQLite store other panes use | **Hide them.** Two viable strategies: (a) sweep on app boot when the flag is off — walk every workspace's pane tree and replace `agent_chat` panes with a placeholder or close them; (b) render them as a "this pane requires Agent Chat (Beta) — enable in Settings" banner. Strategy (b) is cheaper to implement and more reversible — if the user toggles back on, the pane is intact. **Recommend (b).** |
| Synced skills (Step 10) | `~/.codemux/skills/`, `~/.claude/skills/`, etc. + cloud at `api.codemux.org/api/skills` | Files keep on disk. The watcher at `src/hooks/use-skills-sync.ts:67` should pause when the flag is off — no auto-pulls, no auto-pushes, no 5-min poll. Manual "Sync now" button is gone with the Settings → Account → Sync subsection (hidden when the flag is off). The next time the user flips on, the watcher catches up and syncs forward. **Important caveat**: a user who edits a skill while flag-off and never flips on again has unsynced edits — accept this; the user explicitly opted out. |
| MCP server configs | `~/.codemux/mcp-servers.json`, plus discovery at `~/.claude/`, `~/.cursor/` (`docs/features/mcp-server.md`) | Configs stay on disk. **MCP runtime should not spawn child servers when the flag is off** (it currently spawns lazily via `agent_chat_start_session` — already gated, no leak here). Settings → MCP Servers section is hidden. |
| Permission rules | Stored alongside MCP runtime; check `permissions-section.tsx` for storage path | Stays on disk; settings page hidden. Rules are scoped to the chat surface, so they're inert when no chat session exists. |
| Skills-sync encryption key | `~/.local/share/codemux/sync-key.enc` (machine-bound, AES-GCM under `/etc/machine-id`, see `docs/features/skills-sync.md`) | Stays on disk. Forgot-password / reset surface lives in `Settings → Account → Sync`, which is hidden when the flag is off — the key is dormant. |
| Provider capability caches | `OpenCodeServerManager` singleton, `CodexCapabilityCache` (in-memory only) | Skip the eager `useProviderCapabilitiesInit()` boot call when the flag is off (this is a leak, see §1). The OpenCode `tokio::process::Child` is `kill_on_drop` so a never-spawned manager has no resource cost. |
| Favorites | `localStorage["codemux:picker-favorites:v1"]` | Stays in localStorage — picker-only data, no surface to render when chat is hidden, harmless. |

The toggle should never be destructive. If the user later wants to
nuke chat data they can use the existing skills-sync reset dialog
(`reset-sync-password-dialog.tsx`) for skills, and we don't yet have
a "delete chat history" affordance — that's out of scope for Step 13.

### A note on toggle-off mid-session

If the user flips the toggle off while a chat session is live, the
sidecar / `codex app-server` / `opencode serve` children remain spawned
in the `ProviderRegistry` until app close (the registry init is
boot-time only). This is acceptable: the pane tree no longer mounts
the chat pane (placeholder banner per §"persisted layout"), the
backend registry simply stops receiving commands, and on next app
boot the registry is not re-initialised (lib.rs:537 gate). Don't
bother forcing a teardown — it'd require ripping the registry out
of `tauri::State` mid-session, which has no clean idiom.

## 6 — Settings UI Placement Recommendation

Recommended: a **new "Beta Features" nav group** at the top of the
Settings sidebar, above "PERSONAL". Single entry today: "Agent Chat
(Beta)". Picks up future betas (e.g. project-scoped skills sync from
Step 10.5) without re-architecting.

Visual treatment:

- Group label `BETA FEATURES` with the same `[11px] font-semibold
  uppercase tracking-[0.12em]` styling as the other group labels.
- The nav row uses the same `Bot` icon already imported by the Agent
  section (line 35 of settings-view.tsx) — visual cue that this is
  the agent-chat surface.
- The section body has a `<SectionHeader>` matching the other
  sections, plus a single primary `Switch` with a `<Badge>Beta</Badge>`
  next to the label, plus a paragraph explaining what the toggle
  enables (chat panes, multi-provider model picker, skills sync,
  MCP servers, permissions UI, home-chat landing).
- Below the switch, a fine-print line listing the Step 6–12 surfaces
  the toggle controls — same copy that appears in the Settings nav
  rows it hides — so users understand the blast radius.
- A secondary line: "Your data is preserved when this is off." with a
  link to Settings → Account if they want to manage skills-sync data
  directly.

Why not "Experimental"? Because the agent-chat surface is the
intended new default once Beta graduates — calling it experimental
sets the wrong expectation. "Beta" matches the user's framing and
the tone the product is going for.

Why not under Account? Account is per-user / cloud-scoped; the toggle
is per-machine (lives in `observability.json`). Mixing scopes in one
section confuses users.

Why a new group, not slotted into "EDITOR & WORKFLOW"? Because
when the toggle is off, **every** entry in the new group's eventual
lineup is hidden too, so the group is visually empty. Better to
have a self-contained nav group that's always present (with one
visible row) than to interleave hidden rows into an existing group.

## 7 — Effort Estimate

### Stages

**Stage 1 — Backend default flip + flag unification.** ~1/4 session.
1. Flip `default_snapshot()` literals at `observability.rs:259-260`
   to `false`. Add a unit test that loads a missing observability.json
   and asserts both flags default to `false`.
2. Audit every place that reads `enable_lazy_workspace_creation` and
   confirm it's always paired with `enable_agent_chat` (true today).
   Add a one-line comment that the two flags move together; defer
   physical merging to a follow-up if the comment + invariant test
   suffices.
3. Plug the `list_chat_provider_capabilities` leak: gate it on
   `agent_chat_enabled()` returning the existing
   `FEATURE_DISABLED_ERROR`. Frontend handles errors gracefully
   already (`provider-capabilities-store.ts:39-46`).
4. Plug the OpenCode availability-probe leak (`opencode_check_availability`,
   `opencode_ping`, `opencode_list_models` in `commands/opencode.rs`)
   the same way.

**Stage 2 — Frontend boot-path gating.** ~1/4 session.
1. Make `useProviderCapabilitiesInit` no-op when
   `useFeatureFlags(s => s.enableAgentChat)` is false. Read inside
   the hook, not at the call site, so flag flips at runtime engage
   live without an app restart.
2. Make `useSkillsSync` no-op when the flag is false (single
   `if (!enableAgentChat) return;` at the top of the hook).
3. Hide chat-agent presets in `preset-bar.tsx` when the flag is off
   (mirror the existing `kind === "cli"` filter that
   `new-workspace-dialog.tsx:237` uses).
4. Pane-tree placeholder for `agent_chat` panes when the flag is off
   — replace the `<AgentChatPane />` render in `PaneNode.tsx:310`
   with a small banner ("This pane requires Agent Chat (Beta).
   Enable in Settings.") that includes a button opening Settings.

**Stage 3 — Settings hide / show.** ~1/3 session.
1. In `settings-view.tsx`, conditionally drop `permissions`, `skills`,
   `mcp` from `NAV_GROUPS[1].items` when the flag is off.
2. Conditionally hide the `<SyncSection />` block in the Account
   section (lines 749-753).
3. Add the new "BETA FEATURES" nav group with the toggle row, per
   §6.
4. Wire the toggle to the existing `update_feature_flags` Tauri
   command (already exposed) — set both `enable_agent_chat` and
   `enable_lazy_workspace_creation` from the same UI control.

**Stage 4 — Vestigial cleanup + smoke tests.** ~1/4 session.
1. Delete `ENABLE_PROVIDER_PICKER` (always `true`).
2. Update tooltip copy on the sidebar plus-buttons so the OFF state
   doesn't leak the term "Agent" ("New Agent" → "New Workspace" in
   the sidebar header when the flag is off — see §"Cosmetic loose
   ends" below).
3. Add a `update_feature_flags` integration test that flips the
   toggle and asserts `feature_flag_on` returns the disabled error.
4. Manual UI smoke checklist: a new doc at
   `docs/archive/step-13-ui-smoke-checklist.md` patterned after
   `step-12-ui-smoke-checklist.md` covering: fresh-install boot
   shows EmptyState; flip toggle on → agent surfaces appear; flip
   off → surfaces disappear without crashing; pane-tree placeholder
   renders correctly when persisted layout had a chat pane.

### Total

**~1 session, optimistic; ~2 sessions if the pane-tree placeholder
turns into a fight.** Smaller than Step 10 (skills sync end-to-end —
~5–6 sessions) and Step 12 (multi-provider chat — ~7 sessions).
Comparable to Step 11 (planned-but-not-yet-shipped Codex MCP via HTTP
gateway, also ~1.5 sessions).

### Risks (highest first)

1. **`default_snapshot` vs `serde(default)` mismatch.** The current
   `default_snapshot()` returns `enable_agent_chat: true` and
   `enable_lazy_workspace_creation: true`, but the `#[serde(default)]`
   on the field means a missing key in observability.json defaults
   to `false`. Today's behaviour: a user with an existing
   observability.json that doesn't carry these keys gets `false`;
   a user with no observability.json at all gets `true` from
   `default_snapshot`. Step 13 must align both paths. The fix is
   trivial (flip the literals to `false`) but the testing matrix
   (fresh install, partial json, full json, env var override?) is
   what makes this worth flagging as the top risk.
2. **Pane-tree placeholder UX.** A user who toggles off after
   building up a workspace with multiple chat panes will see a wall
   of "Enable Agent Chat (Beta)" banners. Cosmetically ugly but
   correct. Alternative: collapse all `agent_chat` panes into a
   single "you have N hidden chat panes" toast in the workspace
   header. More work; defer to follow-up unless smoke testing
   reveals it as offensive.
3. **`update_feature_flags` Tauri command write semantics.** The
   command writes via `set_feature_flags`, which holds a mutex and
   serialises the snapshot to disk. If a user flips the toggle and
   the disk write fails, the in-memory state is out of sync with
   disk. Existing code logs and continues; if Step 13 makes the
   toggle the headline UI, surface the error to the user.
4. **Skills-sync pause semantics.** When the flag flips off mid-
   session, an in-flight `skillsSyncNow` should be allowed to
   complete (the engine serialises calls anyway). Pausing means
   "stop scheduling new calls" — already implicit if `useSkillsSync`
   no-ops on flag-off. No bespoke teardown needed, but worth
   smoke-testing.
5. **OpenCode server zombie.** If `opencode serve` was already
   spawned (because the flag was on at boot) and the user flips the
   flag off, the child stays alive until app close. Documented
   acceptable in §5 above; revisit only if smoke test reveals
   memory pressure.

### Prerequisites

None. The toggle itself is self-contained. The unfinished cleanup
items (stray prod user, unrun smoke checklists) called out in the
brief are explicitly out of scope.

## 8 — Recommendation

**GO with full toggle.**

The architecture is friendlier than the brief assumed. Two of the
three "highest-risk" surfaces (home screen, click handlers) are
already gated and unit-tested for the flag-off path. The Step 6–12
backend provider registry is already gated. The remaining work is
mostly:

- Closing four leaks (`list_chat_provider_capabilities`,
  `opencode_*`, `useProviderCapabilitiesInit`, `useSkillsSync`).
- Hiding settings nav rows.
- Adding the toggle UI itself.
- Fixing the `default_snapshot` / `serde(default)` default mismatch.

**Don't modify scope to "settings-only" toggle.** The user wants the
home-chat landing reverted when the flag is off — that's already
true on the branch, so we get it for free. Cutting it would be
strictly worse for no win.

**Don't defer.** The branch has been carrying `enable_agent_chat:
true` defaults in `default_snapshot` since the first dogfooding
commit, which means a release-branch merge today would silently
flip every user into Beta. The toggle is a hard prerequisite for
merging to `main`.

## Cosmetic Loose Ends (catch in Stage 4)

These are not toggle-correctness bugs but are visible to users when
the flag is off and look weird:

- Sidebar header label "New Agent" should read "New Workspace" when
  the flag is off (currently always "New Agent" regardless;
  `sidebar-header.tsx:61`).
- Sidebar header tooltip "New chat in home directory · Shift+click
  for workspace dialog" should fall back to "New Workspace" when the
  flag is off (currently always shows the agent-chat phrasing;
  `sidebar-header.tsx:65`).
- The unused legacy `Home draft → eager create` path described as a
  comment at `sidebar-header.tsx:37-41` ("the legacy 'openHomeChat'
  helper has been removed") should also be cleaned up — the comment
  is documenting code that no longer exists.

## Constraints honoured

- Stayed on `feature/agent-chat`, no new branch, no code changes —
  the deliverable is this doc.
- The unfinished Step 10 cleanup and unrun smoke checklists are
  noted as out of scope and not part of Step 13.
