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
8. `AGENTS.md` for Codemux-specific agent operating rules

If the docs themselves feel stale or scattered, also read `docs/reference/DOCS_REINDEX.md`.

## Canonical Layers

- `docs/core/*`: durable project truth
- `docs/features/*`: current subsystem capability and constraints
- `docs/reference/*`: stable protocol and command references
- `docs/plans/*`: active implementation notes and next steps
- `docs/research/*`: supporting research feeding a specific plan in `docs/plans/`
- `docs/archive/*`: superseded notes kept for context

## Current Entry Points

- Auth system: `docs/features/auth.md`
- Settings panel: `docs/features/settings.md`
- Settings sync: `docs/features/settings-sync.md`
- Agent Chat (Beta-gated chat pane, providers, sidecar, attachments): `docs/features/agent-chat.md`
- Multi-provider chat (Step 12): `docs/features/multi-provider-chat.md`; plan + final-state summary at `docs/plans/step-12-opencode-implementation-plan.md`; research at `docs/plans/step-12-opencode-research.md`; operator UI smoke at `docs/plans/step-12-ui-smoke-checklist.md`
- Skills sync (E2E, Step 10): `docs/features/skills-sync.md`; plan + per-stage history at `docs/plans/step-10-skills-sync.md`; research at `docs/plans/step-10-skills-sync-research.md`; operator UI smoke at `docs/plans/step-10-ui-smoke-checklist.md`
- Attachments + context system (Step 8): `docs/plans/step-8-attachments.md` (research + locked plan)
- Beta Features toggle (Step 13): `docs/plans/step-13-beta-toggle-research.md`; operator UI smoke at `docs/plans/step-13-ui-smoke-checklist.md`
- Setup/teardown scripts: `docs/features/setup-teardown.md`
- Worktree bootstrapping: `docs/features/worktree-setup.md`
- Browser work: `docs/features/browser.md`, `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md` (browser stream stability fix archived at `docs/archive/browser-stream-fix.md`)
- OpenFlow work: `docs/features/openflow.md`, `docs/plans/openflow.md`
- MCP server (Codemux as host + as server): `docs/features/mcp-server.md`; vexis-agent integration plan (Phase 1 / 1.5 / 1.6 — all merged) at `docs/plans/vexis-agent-integration.md`, with supporting research at `docs/research/codemux-control-surfaces-current.md` and `docs/research/codemux-phase-1-5-research.md`; **MCP-on-remote** (headless `codemux-remote serve` + 12-tool stdio MCP bridge — landed in `v0.6.2`, `worktree_create` added in `v0.7.5`) plan at `docs/plans/mcp-on-remote.md`; the `v0.7.5` agent-created-workspace pull/adoption fix is tracked at `docs/plans/remote-workspace-pull-fix.md`
- File editor: `docs/features/file-editor.md`
- Diff viewer: `docs/features/diff-viewer.md`
- File tree: `docs/features/file-tree.md`
- Merge resolver: `docs/features/merge-resolver.md`, `docs/plans/git-bot.md`
- Changes panel: `docs/features/changes-panel.md`
- Review tab / PR integration: `docs/features/review-integration.md`
- GitHub issues: `docs/features/github-issues.md`
- Terminal system: `docs/features/terminal.md`
- Resource monitor (title-bar CPU/memory): `docs/features/resource-monitor.md`
- Terminal presets: `docs/features/presets.md`
- Session persistence: `docs/features/session-persistence.md`
- Persistent agents (PTY daemon — now the default spawn path, no setting): `docs/features/persistent-agents.md`
- Remote hosts (DevicePicker + `codemux-remote` binary + SSH transport + zero-touch workspace push + headless `serve` MCP daemon + background version-upgrade poller + **post-`v0.7.8` SSH tunnel-health UI pill + defer-upgrade-restart-while-agents-run**): `docs/features/remote-hosts.md`; deferred desktop pieces (Steps 1/5/6/9) tracked in `docs/plans/mcp-on-remote.md`
- Automations (scheduled host-side agent runs): `docs/features/automations.md`; roadmap at `docs/plans/automations.md`; Phase 2 (sync + remote-host) detailed plan at `docs/plans/automations-sync.md`; Superset research at `docs/research/superset-automations.md`
- Agent hooks: `docs/features/hooks.md`
- Execution backends / sandboxing: `docs/features/execution.md`
- Observability (flags, metrics, safety config): `docs/features/observability.md`
- Port detection (incl. Docker-published container ports for open worktrees): `docs/features/ports.md`
- Search: `docs/features/search.md`
- Code indexing: `docs/features/code-indexing.md`
- Project memory: `docs/features/project-memory.md`
- Command palette: `docs/features/command-palette.md`
- Workspace creation: `docs/features/workspace-creation.md`
- Workspaces overview (full-screen device-grouped list with filters, sibling-device adoption, confirm-before-push + undo, divergence chip, elapsed-time pill): `docs/features/workspaces-overview.md`
- Project identity (first-class `project_uid` + `main`/`worktree` kind + boot sweep, Superset-adapted): `docs/plans/project-identity.md`
- Workspaces sync (cross-device workspace registry — `/api/workspaces` + 30s pull/push loop + git-HEAD divergence tracking + **asymmetric auto-publish from `codemux-remote` hosts** via the 60s `hosts_inventory` SSH poller): `docs/features/workspaces-sync.md`; **repo-unit sync** (treat repo root + worktrees as one shared-history unit instead of cloning the default-branch checkout into a divergent copy; protected non-deletable `repo root` entry + `standalone copy` warning chip — shipped `v0.7.8`, snapshot-local only) — current behavior in `docs/features/workspaces-sync.md` + `docs/features/workspaces-overview.md`, plan + remaining follow-ups (SSH round-trip validation) at `docs/plans/repo-unit-sync.md`; **post-`v0.7.8` multi-device robustness pass** (project-first "Pull project" + protected root + `default_branch`, serialized adopts, `dedupe_sibling_rows`, one-repo-root-per-project, uid-keyed collision-safe host paths, non-destructive "Reconcile copy") in `docs/features/workspaces-sync.md` § "Robustness hardening"
- IDE integration: `docs/features/ide-integration.md`
- Notifications: `docs/features/notifications.md`
- Auto-update: `docs/features/auto-update.md`
- Windows cross-platform work: `docs/plans/windows-support.md`
- Repo boundaries: `docs/reference/ARCHITECTURE.md`
- Keyboard shortcuts: `docs/reference/SHORTCUTS.md`
- Feature inventory: `docs/reference/FEATURES.md`
- Control and automation work: `docs/reference/CONTROL.md`
- Docs cleanup and recovery work: `docs/reference/DOCS_REINDEX.md`
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
