# Codemux Docs Index

- Purpose: Canonical internal docs hub for new sessions and future handoffs.
- Audience: Humans and coding agents continuing work in this repo.
- Authority: Entry point for the internal documentation system.
- Update when: The doc structure, read order, or file ownership changes.
- Read next: `docs/core/PROJECT.md`, `docs/core/STATUS.md`

## Read Order For New Sessions

1. `docs/core/PROJECT.md`
2. `docs/core/STATUS.md`
3. `docs/core/PLAN.md`
4. `docs/core/TESTING.md`
5. relevant feature docs in `docs/features/`
6. `docs/reference/ARCHITECTURE.md` if you need the repo/layer map
7. `docs/reference/CONTROL.md` if touching CLI, socket, browser automation, memory, or indexing
8. `docs/reference/DESIGN-SYSTEM.md` if touching UI, components, colors, or theming
9. `AGENTS.md` for Codemux-specific agent operating rules

If the docs themselves feel stale or scattered, also read `docs/reference/DOCS_REINDEX.md`.

## Canonical Layers

- `docs/core/*`: durable project truth
- `docs/features/*`: current subsystem capability and constraints
- `docs/reference/*`: stable protocol and command references
- `docs/plans/*`: **active** implementation notes and real next steps. A plan whose
  work has fully landed does not stay here — it moves to `docs/archive/`, and the
  current behavior it describes lives in `docs/features/*`.
- `docs/research/*`: research, spikes, and pre-implementation notes — including
  ones whose recommendations were later reversed. Read as historical reasoning,
  never as current truth.
- `docs/archive/*`: closed plans, superseded design notes, operator smoke
  checklists for shipped work, and historical release notes.

## Current Entry Points

- Auth system: `docs/features/auth.md`
- Settings panel: `docs/features/settings.md`
- Settings sync: `docs/features/settings-sync.md`
- Tool permissions (Settings → Permissions — read/remove the agent's `allow`/`deny`/`ask` rules across the user/project/local `settings.json` scopes; Agent Chat Beta only): `docs/features/permissions.md`
- Agent Chat (Beta-gated chat pane, providers, sidecar, attachments): `docs/features/agent-chat.md`
- Chat code blocks (Shiki syntax highlighting themed from the live terminal ANSI palette — `shiki-chat-theme.ts` + the module-level `use-chat-code-plugin.ts` store, `chat.code_wrap` Appearance toggle; PR #216, unreleased after `v0.15.5`): `docs/features/agent-chat.md` (§ Code blocks in chat), `docs/reference/DESIGN-SYSTEM.md`
- Stale provider requests (an approval or `AskUserQuestion` answered after the provider session restarted — `ProviderError::RequestNotPending`, the persisted `RequestResponseFailed` event, the OpenCode survives-restart opt-out, and the resolved-state reducer guard; PR #220, unreleased after `v0.15.5`): `docs/features/agent-chat.md` (§ Pending requests are not conversation history)
- Claude Opus 5 chat-model support (research complete, implementation pending — capability-registry touchpoints, alias/default decision, test impact): `docs/research/opus-5-agent-chat-support.md`
- Subagent view (cross-provider Subagents orchestration card + read-only drill-in — Claude / Codex / OpenCode; PR #125, shipped `v0.11.0`; the docked per-thread `SubagentActivityBar` that replaced the pane-header/titlebar "N subagents running" pills landed in PR #143, shipped `v0.13.0`): `docs/features/agent-chat.md` (§ Subagent view (cross-provider), § Docked live activity bar); locked plan + implementation record at `docs/archive/subagent-view.md`
- Agent run checkpoints (issue #80 — opt-in background rollback snapshot at run start + restore): `docs/features/agent-chat.md` (§ Run checkpoints); design note at `docs/archive/agent-run-checkpoint.md`
- Thread Scope (first-send scope controls below the composer — `ThreadScopeRow`, **`DraftChatSurface` only**: location/checkout/branch with deferred + auto-named worktree creation on first send; a workspace-bound `AgentChatPane` has one project root and one checkout shared by all its tabs, so it renders the read-only Context Row instead; Home draft hides the preset bar; PR #142 shipped `v0.13.0`. Shipped `v0.15.5`: fuzzy name-only search with `/`-path-mode (PR #205), per-thread worktree + project pickers removed from the pane surface plus the Rust adopt-existing-worktree guard (PR #211), and Active/Settled sectioning with snooze folding, a collapsed settled tail, scoped avatar loading, and the `focusCmdkOnOpen` fix across all cmdk chat pickers (PR #212)): `docs/features/agent-chat.md` (§ Thread Scope (first-send scope controls))
- Workflow orchestration (Claude-only — `Workflow` tool_use tap → in-thread `WorkflowRunCard` approval/progress/summary + conditional Orchestration right-panel tab with per-phase agent drill-in + `/workflow` slash command; PR #137, shipped `v0.12.0`): `docs/features/workflow-orchestration.md`
- Multi-provider chat (Step 12): `docs/features/multi-provider-chat.md`; plan + final-state summary at `docs/archive/step-12-opencode-implementation-plan.md`; research at `docs/research/step-12-opencode-research.md`; operator UI smoke at `docs/archive/step-12-ui-smoke-checklist.md`
- Skills sync (server-side, Step 10; six user scan roots incl. `~/.agents/skills/` and `~/.config/opencode/skills/`, kept in lockstep with `skills::paths::enumerate_scan_paths` by build-failing guard tests — PR #217, unreleased after `v0.15.5`): `docs/features/skills-sync.md`; plan + per-stage history at `docs/archive/step-10-skills-sync.md`; research at `docs/research/step-10-skills-sync-research.md`; operator UI smoke at `docs/archive/step-10-ui-smoke-checklist.md`
- Attachments + context system (Step 8): `docs/research/step-8-attachments.md` (research + locked plan)
- MCP host runtime (Step 9 — Codemux as MCP host/client for chat): current behavior in `docs/features/agent-chat.md` (§ MCP host runtime) + `docs/features/mcp-server.md`; research at `docs/research/step-9-mcp-servers.md`; Codex MCP gateway feasibility spike (future Step 11) at `docs/research/step-9-codex-mcp-spike.md`
- Beta Features toggle (Step 13): `docs/archive/step-13-beta-toggle-research.md`; operator UI smoke at `docs/archive/step-13-ui-smoke-checklist.md`
- Setup/teardown scripts: `docs/features/setup-teardown.md`
- Worktree bootstrapping: `docs/features/worktree-setup.md`
- Browser work: `docs/features/browser.md`, `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md` (browser stream stability fix archived at `docs/archive/browser-stream-fix.md`)
- Background browser in Agent Chat GUI mode (agent-opened browser runs detached instead of splitting the chat — inline chip + context-bar indicator + floating `BrowserPeekOverlay` with promote-to-pane; PR #138, shipped `v0.12.0`; run-finished `LIVE` release (PR #139) + opt-in desktop-size peek viewport `agent_chat.background_browser_desktop_viewport` (PR #140), both shipped `v0.13.0`): `docs/features/browser.md` (§ Background browser in GUI mode, § Run-finished release, § Desktop-size peek viewport)
- OpenFlow work: `docs/features/openflow.md`, `docs/plans/openflow.md`
- MCP server (Codemux as host + as server): `docs/features/mcp-server.md`; vexis-agent integration plan (Phase 1 / 1.5 / 1.6 — all merged) at `docs/research/vexis-agent-integration.md`, with supporting research at `docs/research/codemux-control-surfaces-current.md` and `docs/research/codemux-phase-1-5-research.md`; **MCP-on-remote** (headless `codemux-remote serve` + 12-tool stdio MCP bridge — landed in `v0.6.2`, `worktree_create` added in `v0.7.5`) plan at `docs/plans/mcp-on-remote.md`; the `v0.7.5` agent-created-workspace pull/adoption fix is tracked at `docs/archive/remote-workspace-pull-fix.md`
- File editor: `docs/features/file-editor.md`
- Diff viewer: `docs/features/diff-viewer.md`
- File tree: `docs/features/file-tree.md`
- Merge resolver (**currently unreachable from the UI** — backend, commands, store, and settings row intact, but both entry-point buttons were removed by `92965c9`): `docs/features/merge-resolver.md`, `docs/plans/git-bot.md`
- Changes panel: `docs/features/changes-panel.md`
- Review tab / PR integration (resting layout: header + checks + read-only threads; the review composer, merge controls, and deployments were intentionally removed, backends retained): `docs/features/review-integration.md`
- GitHub issues: `docs/features/github-issues.md`
- Terminal system (incl. the pane-header live cwd hint — OSC 7 primary + Linux `/proc` poll fallback, shown only off the workspace root; PR #209, shipped `v0.15.5`): `docs/features/terminal.md`
- Resource monitor (title-bar CPU/memory): `docs/features/resource-monitor.md`
- Terminal presets: `docs/features/presets.md`
- Session persistence: `docs/features/session-persistence.md`
- Persistent agents (PTY daemon — now the default spawn path, no setting): `docs/features/persistent-agents.md`
- Remote hosts (DevicePicker + `codemux-remote` binary + SSH transport + zero-touch workspace push + headless `serve` MCP daemon + background version-upgrade poller + **`v0.7.9` SSH tunnel-health UI pill + defer-upgrade-restart-while-agents-run** + **post-`v0.9.5` "Reinstall agent on host" button + cloud-push migration overlay**): `docs/features/remote-hosts.md`; deferred desktop pieces (Steps 1/5/6/9) tracked in `docs/plans/mcp-on-remote.md`
- OpenCode conversation sync across cloud-push (issue #16 — `opencode export`/`import` so a pushed/pulled OpenCode pane continues the same session; sibling of the Claude JSONL sync): `docs/features/opencode-conversation-sync.md`
- Operate a remote workspace in place — **"Open on host"** (attach-in-place: terminal/agent runs on the host with no local copy; detached pty-daemon survives app close and reattaches on reopen; overview action + `attach_only`/`remote_cwd` snapshot fields): `docs/features/remote-in-place.md`
- Automations (scheduled host-side agent runs): `docs/features/automations.md`; roadmap at `docs/plans/automations.md`; Phase 2 (sync + remote-host) detailed plan at `docs/archive/automations-sync.md`; Superset research at `docs/research/superset-automations.md`
- Agent hooks: `docs/features/hooks.md`
- Execution backends / sandboxing: `docs/features/execution.md`
- Observability (flags, metrics, safety config + native log file / `codemux logs` / `codemux doctor` + opt-in cloud-push diagnostic tracing via `CODEMUX_TRACE_CLOUD_PUSH`): `docs/features/observability.md`
- Linux file-dialog backend preflight (issue #95 — `select_backend` Portal/Zenity/None decision, Codemux-driven zenity fallback with sanitized env + timeout for the portal-hangs case, cause-specific remediation toast): `docs/features/workspace-creation.md` (§ constraints), `docs/features/observability.md` (§ native log file), `docs/reference/CONTROL.md` (§ local diagnostics)
- Port detection (incl. Docker-published container ports for open worktrees): `docs/features/ports.md`
- Search: `docs/features/search.md`
- Code indexing: `docs/features/code-indexing.md`
- Project memory: `docs/features/project-memory.md`
- Command palette: `docs/features/command-palette.md`
- Workspace creation: `docs/features/workspace-creation.md`; model-selection-at-launch plan at `docs/archive/model-selection-before-launch.md`
- Non-git projects (plain-folder mode — `is_git` snapshot flag, opt-in "Initialize Git" bare-`git init` affordance in the context bar / Context Row cluster / Changes panel, worktree controls hidden in Thread Scope, no add-path git gate; PRs #147/#148, shipped `v0.13.1`): `docs/features/workspace-creation.md` (§ Non-Git Projects)
- Workspaces overview (full-screen device-grouped list with filters, sibling-device adoption, confirm-before-push + undo, divergence chip, elapsed-time pill): `docs/features/workspaces-overview.md`
- Workspace archive (one-click non-destructive archive replacing the sidebar X/Hide flow, Settings → Archive restore panel, Rust-enforced protected-root delete refusal, honest force semantics for dirty-worktree deletes): `docs/features/workspace-archive.md`
- Project identity (first-class `project_uid` + `main`/`worktree` kind + boot sweep, Superset-adapted): `docs/plans/project-identity.md`
- Workspaces sync (cross-device workspace registry — `/api/workspaces` + 30s pull/push loop + git-HEAD divergence tracking + **asymmetric auto-publish from `codemux-remote` hosts** via the 60s `hosts_inventory` SSH poller): `docs/features/workspaces-sync.md`; **repo-unit sync** (treat repo root + worktrees as one shared-history unit instead of cloning the default-branch checkout into a divergent copy; protected non-deletable `repo root` entry + `standalone copy` warning chip — shipped `v0.7.8`, snapshot-local only) — current behavior in `docs/features/workspaces-sync.md` + `docs/features/workspaces-overview.md`, plan + remaining follow-ups (SSH round-trip validation) at `docs/plans/repo-unit-sync.md`; **`v0.7.9` multi-device robustness pass** (project-first "Pull project" + protected root + `default_branch`, serialized adopts, `dedupe_sibling_rows`, one-repo-root-per-project, uid-keyed collision-safe host paths, non-destructive "Reconcile copy") in `docs/features/workspaces-sync.md` § "Robustness hardening"
- Project avatars (sidebar project image + color customization, favicon derivation + cache-bust; reachable from the `Project "<name>"` submenu on all three inbox row shapes — active card, settled row, snoozed row — re-homed there by PR #213 after PR #198 unmounted the project tree; the appearance store merges a racing load field-by-field so a write always wins): `docs/features/project-avatars.md`
- IDE integration: `docs/features/ide-integration.md`
- Left sidebar (two-state: expanded **flat workspace inbox** ↔ 52px rail. The inbox — shipped `v0.15.0`, from the `Sidebar Inbox.dc.html` design handoff — replaced the nested project tree (and the PR #182 living-sidebar strip/LIVE-grouping) with search + a project-filter dropdown (with active-workspace counts) + one card per active workspace + a persisted "Settled" section with settle/auto-settle/un-settle lifecycle, 288px default width, and Settings → Appearance → Sidebar "Show git stats" + "Auto-settle idle work" toggles; the rail was rebuilt as a per-workspace avatar strip with individual status dots and select-without-expand (the old project-avatar rail + hover flyout was deleted), and the configurable working indicator remains. Shipped `v0.15.5`: backend-owned activity stamps, a "Wrapping up" tier, snooze, a bulk-action safety net (PR #206), an oldest-blocked-first pinned needs-you strip with newest-first ordering (PR #207), and a shared workspace hover details card on every surface (PR #208). Unreleased after `v0.15.5`: PR badges restored on settled rows plus server-side completion settlement — merged/closed settles immediately with no idle grace — and palette indexing of settled workspaces by project path (PR #219), row-scoped keyboard activation, and a two-column card meta line so PR chips right-align (PR #221)): `docs/features/sidebar.md`
- Workspace context bar (bottom status strip under the work surface: branch · kind · ↑↓ · diff · PR/issue chips · background-browser pill · device for the active workspace — the detail home for the clean sidebar appearance; since PR #144 it hides itself while an Agent Chat pane is active in GUI chrome, and the same detail renders in the pane's Context Row instead): `docs/features/workspace-context-bar.md`
- Context Row (the Agent Chat pane's below-composer status row — read-only project/branch labels + the shared `WorkspaceStatusCluster` (behind chip, PR chip, `BackgroundBrowserIndicator`, workspace-details popover); renders whenever the pane's project root resolves — at every message count, so the strip never changes shape across the first send; PR #144, shipped `v0.13.0`; the `trailing` slot it originally also filled was deleted with PR #211): `docs/features/agent-chat.md` (§ Context Row (running-thread status)); the bar's side of the split at `docs/features/workspace-context-bar.md`
- GUI chrome (Agent Chat Beta on: the four chrome rows collapse to a single `h-10` title bar with in-titlebar tabs + `+` agent launcher + inline chat favorite + rehomed right-panel/Run controls; legacy chrome byte-identical when the flag is off; PR #135, shipped `v0.12.0` — the in-titlebar "N subagents running" pill was later removed by PR #143 in favor of the docked `SubagentActivityBar`): `docs/features/gui-chrome.md`
- Notifications: `docs/features/notifications.md`
- Auto-update: `docs/features/auto-update.md`
- Windows cross-platform work: `docs/plans/windows-support.md`
- Dev mock runtime (dual-guarded `src/dev/` Tauri shim that boots the real UI in a plain browser under `npm run dev` with seed data, for visual/screenshot work): `docs/features/dev-mock-runtime.md`
- Web-remote transport bandwidth (lossless negotiated WS compression `?compress=deflate` + `[0x02]`/`[0x03]` envelopes with per-connection context takeover, binary PTY framing over the `[0x01]` channel frame, HTTP gzip on JSON APIs + bundle, `CODEMUX_WEB_REMOTE_STATS=1` baseline counters; PR #224 / issue #215, unreleased after `v0.15.5`): `docs/features/web-remote-access.md` (§ WS protocol contract)
- Web remote access (embedded default-off HTTP+WS server in the desktop app — **shipped in `v0.14.0`**; a browser on another device loads the same UI bundle and drives the same running instance via a `__TAURI_INTERNALS__` WebSocket shim — pairing-token **or same-account** admission, `codemux remote pair` CLI, `all`/`tailscale`/`loopback` bind scopes, mirror-mode multi-client stream fan-out, web fallbacks incl. remote project creation; post-`v0.14.2`, `codemux serve` boots the full backend headlessly with the same command surface): `docs/features/web-remote-access.md`; locked v1 design contract + remaining work at `docs/plans/web-remote-access.md`
- Web remote — from-anywhere account/relay/hosted tier (the desktop as an **iroh** QUIC endpoint gated by relay mode, self-registering its `NodeId` with the `api.codemux.org` device registry; a hosted static client at `app.codemux.org` — GitHub OAuth / email sign-in — dials a chosen device over E2E-encrypted QUIC; **code landed on `main`, hosted service not yet deployed**): account/relay design (Stages A–C) at `docs/plans/web-remote-account-mode.md`; hosted-client deploy runbook at `docs/plans/app-codemux-org-hosting.md`; current-truth for the shipped LAN/pairing/account-mode base in `docs/features/web-remote-access.md`
- Repo boundaries: `docs/reference/ARCHITECTURE.md`
- Design system (color tokens, theming layers, `.theme-warm`/density scales, the no-hardcoded-colors rule — shadcn stone base `b1HYEHloH`): `docs/reference/DESIGN-SYSTEM.md`
- Keyboard shortcuts: `docs/reference/SHORTCUTS.md`
- Feature inventory: `docs/reference/FEATURES.md`
- Control and automation work: `docs/reference/CONTROL.md`
- Docs cleanup and recovery work: `docs/reference/DOCS_REINDEX.md`
- Historical release notes (`v0.6.1`–`v0.13.2`, moved out of `docs/core/STATUS.md`): `docs/archive/release-notes-v0.6-v0.13.md`
- Agent behavior rules: `AGENTS.md`
- Website docs: https://docs.codemux.org — source lives in ~/projects/codemux-sitev2/ (Next.js + Fumadocs). New doc pages go in the content/docs/ directory of that repo.

## Update Rules

- Update `docs/core/PROJECT.md` when the product direction or architecture boundaries change.
- Update `docs/core/STATUS.md` when implementation reality changes.
- Update `docs/core/PLAN.md` when build order or major milestones change.
- Update `docs/core/TESTING.md` when the verification strategy changes.
- Update feature docs when subsystem behavior or constraints change.
- Update plan docs when active next steps or working notes change.
- Move stale notes to `docs/archive/` instead of leaving them mixed into canonical docs.
- Start new feature docs from `docs/templates/FEATURE_TEMPLATE.md`.
- Start new plan docs from `docs/templates/PLAN_TEMPLATE.md`.

## Single Source Of Truth

The maintained docs system now lives entirely in:

- `WORKFLOW.md`
- `docs/core/*`
- `docs/features/*`
- `docs/plans/*`
- `docs/research/*`
- `docs/reference/*`
- `docs/archive/*`
- `docs/templates/*` as helper starting points for new docs
