# Codemux Status

- Purpose: Canonical reality snapshot for the repo.
- Audience: Anyone deciding what is actually true today.
- Authority: Current implementation truth.
- Update when: Behavior, constraints, or known gaps change.
- Read next: `docs/core/PLAN.md`, `docs/core/TESTING.md`

## Current Headline

Codemux is past Linux MVP and shipping cross-platform binaries. The workspace shell, terminal management, git integration, presets, settings sync, and most ADE features are real and daily-drivable on both Linux and Windows. Latest released version is `v0.7.8`.

Landed on `main` after the `v0.7.8` tag (unreleased) is a **multi-device robustness + remote-persistence pass** layered on top of repo-unit sync. (1) **SSH tunnel health is now surfaced in the UI**: the `TunnelStatus` the supervisor already computed (`connected`/`pending`/`reconnecting`/`circuit_open`) is bridged to the frontend via a new `tunnel-status-changed` event + `spawn_tunnel_status_forwarder` (self-terminating per supervisor), a zustand `tunnel-status-store` fed by an app-root `useTunnelStatusEvents` hook, and a sidebar pill — amber **"Reconnecting…"** on a sleep/wake or WiFi flap, red **"Connection lost — re-push"** once the circuit breaker trips — so a dropped tunnel no longer looks like a frozen workspace. (2) **Host persistence**: auto-upgrade no longer kills host-side agents — `hosts_upgrade` probes the daemon's `live_terminals` (via `codemux-remote serve status`) and **defers** the systemd-unit restart when sessions are live (`UpgradeOutcome::Skipped`); separately, the local pty-daemon now **idle-reaps** itself after 1h with zero sessions (hard re-check under lock so it can never reap a live session). (3) **Workspaces-sync robustness**: project-first remote pull with a real protected root (new local-only `default_branch` column + `resolve_default_branch`/`ensure_origin_head` + `workspaces_adopt_project` "Pull project" action), serialized adopts via a per-row creation lock, client-side `dedupe_sibling_rows` collapse of cross-device duplicate cards, daemon-side `collapse_main_for_uid`/`normalize_main_workspaces` (one repo root per project), uid-keyed collision-safe host paths (`<basename>-<short-uid>`), and a non-destructive `workspaces_reconcile_copy` action for legacy divergent copies. (4) **OpenFlow comm-log fix**: the daemon-backed agent spawn path (default since persistent agents) now tees cleaned PTY output to the communication log via the shared `comm_log_entry_for_chunk` helper, so daemon-spawned OpenFlow agents stop producing an empty log that blinded stuck-detection. See `docs/features/remote-hosts.md`, `docs/features/persistent-agents.md`, `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`, `docs/features/openflow.md`, `docs/plans/repo-unit-sync.md`.

Shipped in `v0.7.8` is **repo-unit sync** — cross-device sync no longer clones a project's default-branch checkout into a divergent full copy under `~/.codemux/worktrees/<project>/main` (its own object store, drifting from the real repo, and — because it lived in the worktrees tree — misclassified as a deletable disposable worktree). New `git_canonical_root` + `RepoRoot` + `is_divergent_copy` + `is_protected_repo_root` helpers in `src-tauri/src/git.rs` classify a checkout divergence-safely via `git rev-parse`; `WorkspaceSnapshot` gained `protected` + `divergent_copy` flags (`#[serde(default)]`, stamped at create time plus a background boot backfill `backfill_workspace_protection` wired in `lib.rs`, mirrored onto the TS type). The overview renders a `repo root` badge on a protected root (whose row offers only "Close workspace…" / detach — never "Delete worktree…", files untouched) and an amber `standalone copy` warning chip on a legacy divergent copy guiding the user to delete + re-pull cleanly. Adoption routes a `main`-kind row through `create_synced_root_shell` landing at `~/.codemux/projects/<repo>` (a genuine protected root) instead of the divergent worktree landing; `workspace_push_to_host` pushes a repo root to `~/.codemux/projects/<repo>` on the host (`conventional_remote_root_path`) and `workspace_pull_back_impl` is root-aware with a fallback to the legacy `worktrees/` path so roots pushed before the change still pull. The three flags are snapshot-local only (not added to `workspaces_sync`/cloud — sibling rows have no delete action). The cross-device SSH round-trip and one-click reconcile of existing copies remain follow-ups. See `docs/plans/repo-unit-sync.md`, `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`.

The same `v0.7.8` window added **Docker-published container ports** to the sidebar: `scan_ports` folds in `detect_docker_ports`, which shells out to `docker ps` (cached `DOCKER_CLI_STATE` so a missing binary doesn't trigger a doomed spawn every cycle) and surfaces each published TCP host port under a dedicated **Docker** group labeled by container name — recovering ports the Linux `/proc` scan can't attribute because the publishing `docker-proxy` runs as root. Detection is scoped to open codemux worktrees by matching the compose `working_dir` label against `all_workspace_paths`; the kill action is hidden for these rows. See `docs/features/ports.md`. Plus two new-workspace-dialog fixes: issue-link dedup now surfaces a toast + an inline "already has a workspace / Open it" notice instead of silently activating an existing workspace and dropping the typed message, and the "Link pull request" button now uses the same polished `PrPickerPanel` (search, `PrStatusIcon` state-colored icons, skeleton loading, "Link ↵" hover affordance) as the issue picker. See `docs/features/workspace-creation.md`.

Shipped in `v0.7.7` is a **cross-device pull/adopt/close hardening pass** — two audits (one deep code audit, one live end-to-end SSH round-trip against a loopback host) surfaced a cluster of failure-mode/lifecycle bugs in the remote-host push/pull/adopt + inventory-poll pipeline, all off the happy path. The fixes: (1) `workspaces_adopt_synced` now checks `pull_outcome.ok` — a failed rsync/SSH pull previously returned a success toast over an empty shell that the idempotency guard then refused to retry; on failure it now removes the shell, reverts the row to a re-pullable sibling (`unlink_workspace_sync_from_local`), and returns a real `Err`. (2) Both close paths (`close_workspace` **and** `close_workspace_with_worktree`) now tear down the SSH tunnel supervisor + cached client (`forget_workspace_client` + `shutdown_supervisor`); previously closing a pushed workspace leaked the `TunnelSupervisor` task, the bound local socket, and the remote pty-daemon for the app's lifetime. (3) All three adopt paths (single-dir, worktree-repo-rsync, clone) now stamp `project_uid` (`set_workspace_project_identity`) so adopted workspaces converge with their siblings in the overview instead of pushing a `None` that wiped the daemon-derived uid. (4) The inventory poller undeletes a remote-discovered tombstone on reappear (and dedupes duplicate ids within one envelope) instead of INSERTing a fresh row that churned the cloud row. (5) `pull_workspace_back` classifies a missing remote path as `RemoteNotFound` via a stdout sentinel (`CMX_REMOTE_DIR_OK/MISSING`) instead of a fragile `exit status: 7` string-match. (6) `provision_workspace_mcp_config` resolves the remote `$HOME` and emits an absolute `.mcp.json` command path (the systemd `%h` specifier agent CLIs can't expand silently broke remote MCP auto-discovery). Plus shell-injection hardening on `ssh_upload_executable`/`ssh_write_file` (via tilde-aware `shell_escape`), a tunnel-socket hash widened 12→16 hex chars (48→64-bit, kills a birthday-collision risk), a stale-link pull guard (`detect_same_branch_project_conflict` now intersects against live workspace ids so a closed-but-stale sync row no longer phantom-blocks Pull), and a sidebar fix so the workspace-row X button no longer overlays + swallows clicks on the linked-issue badge. A new env-gated (`CODEMUX_E2E_SSH_HOST`) integration harness, `src-tauri/tests/codemux_ssh_roundtrip.rs`, encodes the `RemoteNotFound`, `.mcp.json`-absolute, and tilde-expansion regressions. See `docs/features/workspaces-sync.md`, `docs/features/remote-hosts.md`.

The `v0.7.5` release rounds out the remote-workspace story and hardens the terminal. **Agent-created host workspaces now pull with their files and git intact**: a workspace an agent built directly on a `codemux-remote` host (e.g. via `git worktree add` + `workspace_create`) previously pulled to the desktop as an empty worktree because the pull rsynced from the conventional `~/.codemux/worktrees/<project>/<branch>` path rather than the workspace's real on-host `path`. The fix threads the daemon's actual path through a new local-only `origin_path` column on `workspaces_sync`, recreates linked worktrees of local-only repos via a whole-repo rsync + `git worktree prune`/`add` (so a worktree's cross-machine-broken `.git` gitfile is rebuilt locally), and stops the cloud round-trip from clobbering locally-derived `project_uid`/`workspace_kind` with `COALESCE(server, local)`. The same work added a **`worktree_create` tool to the headless daemon catalog (now 12 tools, up from 11)** so an agent on a host can make a canonical-layout worktree directly instead of improvising one. See `docs/plans/remote-workspace-pull-fix.md`. The terminal gained **PTY output flow-control / backpressure** plumbing (a daemon-side `SetFlowPaused` request + HIGH/LOW write-queue watermarks) — landed but currently inert on the live render path (see "Workspace & terminals" below) — plus split panes now inherit the workspace cwd, and the **standalone "Diff Viewer" new-tab (+) and command-palette entry points were removed** (the Changes-panel file-click flow is the only way to open a diff tab; the diff tab kind and all diff infrastructure stay).

`v0.7.4` shipped **first-class project identity + `main`/`worktree` workspace kind** (landed on `main` before the tag). "Project" was previously implicit (path-derived `basename(project_root)`, with `main`-vs-`worktree` inferred from `worktree_path == null`), so a repo cloned at different paths on different devices/hosts had no stable identity and agent-/host-created workspaces appeared anonymous. A new `src-tauri/src/project_identity.rs` computes identity **deterministically** — `project_uid = UUIDv5(canonical git remote ?? absolute project_root)` — so every checkout of the same remote converges on the same uid with zero coordination (no replicated projects table). `derive_kind` classifies `main` (`.git` is a directory) vs `worktree` (`.git` is a file). The headless daemon (`remote/workspace.rs`) stamps `project_uid`/`project_name`/`kind`/`repo_remote` at create plus an idempotent boot sweep; the `hosts_inventory` poller threads them into `workspaces_sync` (and finally populates `project_remote` for remote-discovered rows, closing a v1 gap); the desktop stamps `WorkspaceSnapshot.project_uid`/`workspace_kind` in `set_workspace_project_root`; and the cloud `codemux_workspaces` schema now round-trips all three fields (server-authoritative on pull). The overview groups by `project_uid` and renders a `main`/`worktree` badge, and the pull-conflict guard matches on exact `project_uid`. See `docs/plans/project-identity.md`, `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`.

The headline addition in `v0.7.x` is **asymmetric workspace auto-publish from `codemux-remote` hosts**: a workspace an agent creates directly on a host — via the MCP `workspace_create` tool or a manual `codemux-remote workspace register` — now surfaces in every dev device's Workspaces overview without an explicit push from the laptop. The model is deliberately asymmetric: dev devices keep their existing manual push/pull (preserves the "close laptop, continue in cloud" flow and user agency over what leaves the device); `codemux-remote` hosts auto-publish because they're always-on and exist to be used as hosts. Pipeline: a new `codemux-remote workspace list` CLI subcommand reads the daemon's SQLite directly (works even when the daemon isn't running); a new `hosts_inventory.rs` background poller runs every 60 s on each dev device, SSHes the CLI, and reconciles the result into `workspaces_sync` as sibling-only rows keyed by `(host_server_id, origin_uid)`; the existing 30 s push tick then uploads dirty rows to the cloud, so other devices see new host-side workspaces within ~90 s. Schema is additive (`ALTER TABLE workspaces_sync ADD COLUMN origin_uid TEXT`). A pull-conflict guard (`AdoptionPreview.same_branch_project_exists_at` + `SameBranchProjectBlock` dialog variant) refuses to clobber a local workspace already on the same branch of the same project.

`v0.7.x` also lands three UX fixes: configured-host buckets stay visible in the Workspaces overview even when empty (a device the user has set up shows up immediately, not only after the first workspace lands on it); close-workspace paths reconcile + push the sync row immediately so the overview never briefly mis-tags a just-closed workspace as "lives on another device"; and `ssh::probe` falls back to `~/.local/bin/codemux-remote` when the PATH lookup fails (non-interactive SSH on Arch/Ubuntu/Fedora doesn't source `~/.profile`).

The earlier `v0.6.2` headline — **MCP-on-remote** — remains daily-drivable: `codemux-remote` graduated from a slim PTY-daemon binary into a full **headless Codemux daemon** that owns its own workspace registry, axum HTTP control endpoint with bearer-token auth, and a stdio MCP server (an 11-tool catalog at `v0.6.2`; `worktree_create` added in `v0.7.5` brings it to 12). An agent CLI on a remote host (VPS, home server, anywhere `codemux-remote` is installed) can drive Codemux through MCP — `workspace_create`, `terminal_write`, `app_status` — without any UI. The desktop's push flow auto-provisions the systemd user unit, writes a per-workspace `.mcp.json`, auto-registers `codemux-remote mcp` into every supported agent config (`~/.claude.json`, `~/.codex/config.toml`, `~/.cursor/mcp.json`, …) on `serve` startup, and registers the pushed workspace into the daemon's registry — so the user clicks "Push workspace to host" once and the agent on the host sees Codemux as an MCP server with no further config. The `hosts_upgrade` background poller silently re-bootstraps every registered host when the bundled `codemux-remote` binary version differs from the host's installed version. Binary upload uses a `ssh-cat` pipeline (instead of `scp`) to work around an OpenSSH 9+ tilde-expansion bug.

The earlier `v0.6.1` additions — **account-wide Workspaces overview + cross-device workspaces sync** (full-screen device-grouped list with filters, push/pull/adopt actions, host-backed adoption, clone-from-git adoption fallback, confirm-before-push + 10-second undo safety guardrails, cross-device git-HEAD divergence chips, elapsed-time pill while a push or pull is in flight, first-run welcome banner, "how it works" popover) — and the `v0.6.0` **seven new agent presets** (Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode) remain daily-drivable. The `v0.5.x` headline — **Automations**, account-synced scheduled agent runs with eight `automation_*` MCP tools (bringing the desktop inventory to **52 tools**) — remains daily-drivable. The agent-chat surface (Step 6–12: chat pane, multi-provider picker, skills sync, attachments, mode pills, slash commands, plan proposals, MCP host runtime, …) remains merged to `main` and Beta-gated; the **persistent PTY daemon** is the default spawn path (every shell survives app close, the env-var escape hatch is the only off-switch), the **SSH workspace-push** action that was deferred in step 2d has landed (push a worktree to a user-owned host with Claude conversation sync), and the right sidebar + Settings panel have been redesigned to a **refined-minimal aesthetic** to match the rest of the app. The MCP server inventory grew from 31 to 44 tools across the Phase 1 / 1.5 / 1.6 vexis-agent integration steps before Automations took it to 52.

OpenFlow and the browser pane are still being hardened. OpenFlow is intentionally disabled on Windows until the bash-wrapper rewrite lands.

The repo structure is clean and domain-split:

- `src/` is the React + Tailwind + shadcn UI and Tauri IPC layer
- `src-tauri/` is the Rust app/runtime layer
- `sidecar/claude-agent/` is the Bun-compiled TypeScript subprocess that hosts the Claude Agent SDK

## Solid — Daily-Drivable Features

### Workspace & terminals
- Workspace shell, sidebar, workspace sections with color coding and drag-drop
- Multi-session terminals with xterm.js, WebGL renderer + Unicode 11 widths, kitty protocol, low-latency pane input
- Hidden-pane terminal pause to eliminate cross-workspace typing lag
- Tab bar with terminal, browser, editor, and diff tab types
- Pane splits, resize, drag-swap, close — split panes inherit the workspace cwd (`v0.7.5`)
- **PTY output flow-control / backpressure** plumbing (`v0.7.5`): a daemon-side `SetFlowPaused` wire request + per-session `flow_paused` gate, and renderer HIGH (16 MiB) / LOW (4 MiB) write-queue watermarks that can `pause_pty_output`/`resume_pty_output`. The daemon side and fail-safes are real, but only the disabled `terminal-cache.ts` lineage calls pause/resume — the live `TerminalPane` path does not, so backpressure is **inert in production today** (the live freeze fix is the throttled write pump, not PTY pausing). See `docs/features/terminal.md`.
- **Terminal workspace-switch performance**: the live terminal path is the per-mount lifecycle in `src/components/terminal/TerminalPane.tsx` (xterm constructed on mount, disposed on unmount — only the active workspace/surface is ever rendered). Switching no longer freezes because all xterm writes (disk-scrollback restore + the `attach_pty_output` reattach replay + live output) drain through a throttled, byte-budgeted write pump (`src/components/terminal/terminal-write-pump.ts`) that yields between batches, and the wasted serialize of alt-screen TUI buffers (Claude Code, lazygit, vim, btop) is skipped on unmount. The earlier persistent-xterm cache (`terminal-cache.ts`, shipped `14735bf`) was rolled back in `2baa42f`; it is retained but **disabled / not wired** (see its banner) pending a possible future flag-gated revival, so `useTerminalCacheGc` / `useTerminalThemeSync` are no-ops today.
- **Session persistence**: terminal scrollback save/restore across restarts (Windows-only backend backstop in `scrollback::flush_cache_to_disk`), adapter-based resume for CLI tools (Claude Code `--resume`/`--continue` via hook-captured session IDs)
- **Persistent PTY daemon** (`pty_daemon::server` + `client` + `supervisor` + `manifest`): every shell spawn now routes through a detached `codemux pty-daemon` subprocess so agents survive app close. On relaunch the supervisor adopts the running daemon and reattaches live sessions. **Default-on**, no setting; `CODEMUX_DISABLE_PTY_DAEMON=1` is the only escape hatch. Graceful fallback to the in-process portable-pty path on every error site, plus a 3-failures-in-60s crash circuit breaker that disables the daemon path for the rest of the process lifetime. Unix only; Windows still uses the in-process path until the named-pipe IPC is wired. **Post-`v0.7.8`**: the daemon **idle-reaps** itself (manifest + socket removed, process exits) after 1h with zero live sessions (hard re-check under the lock so it can never reap a live session), and the daemon-backed reader now tees cleaned PTY output to the OpenFlow comm log via the shared `comm_log_entry_for_chunk` helper (fixing empty comm logs for daemon-spawned OpenFlow agents).

### Git & GitHub
- Git worktree-based workspaces (create from new/existing branch, import orphans, derivative-branch picker with recency)
- Changes panel in right sidebar (stage/unstage/commit/push, inline per-file diffs, AI commit messages, Open-PR button on toolbar, AI merge resolver entry)
- Full-pane diff viewer tab (unified/split layouts, section filters incl. `against_base`, hunk/file navigation, focus mode)
- Review tab (renamed from PR tab, React Query refactor): PR creation, header, reviews, checks, deployments, merge controls
- Sidebar PR status icon per workspace with stale-clearing on branch switch and DRAFT collapse
- **Default-branch detection** drives the sidebar branch pill: seed from `origin/HEAD`, follow live remote-branch changes, and the derivative-branch picker drops the phantom `origin/<name>` rows so users never pick a remote-only ref by accident
- "Checkout default branch" workspace action
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge) with non-blocking activate + visibility-based gate
- Sidebar ahead/behind arrows refresh against fresh remote refs
- Auto-transition to main workspace after merge+delete
- Merge-into-base runs on the blocking pool so it cannot freeze the GUI; uses `update-ref` for worktree compatibility
- Hide stale merged-PR pill on long-lived branches
- Review tab unfreezes on repos with thousands of PRs (paginated fetch)
- All git/gh shell-outs moved off the GTK main thread to keep IPC responsive

### GitHub issues
- Link issues to workspaces, issue picker in creation dialog, sidebar display with detail popover, auto-branch naming from issue, prompt auto-injection of issue context
- CLI: `codemux issue list/view/link`, control socket commands

### Browser
- Screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP)
- Browser pane in pane layouts; address bar, refresh, home, external-link
- Per-workspace stream sessions keyed by `workspace_id` (PID-tracked daemons, single canonical key, atomic teardown, symmetric `TcpListener` bind probe, reactive `stream_url` reconnect on the frontend)
- Dynamic stream ports (9223–9299) for concurrent worktrees
- Stealth Chromium flags + realistic user-agent string
- Browser data management in Settings (clear cookies, clear all data, view data size)
- Inspector panel for debugging web content
- **Viewport presets**: `codemux browser viewport <mobile|tablet|desktop|WxH|reset>` resizes the actual viewport via CDP so CSS media queries fire and screenshots capture at the simulated dimensions. MCP exposes `browser_viewport` + `browser_viewport_presets`.
- Browser stream stability fix shipped (commit `7e36420`): unified port keying, hardened daemon lifecycle, dropped dead workspace_id alias lookups. Eliminates the silent stream failure that appeared after multiple concurrent worktrees used the browser.

### Workspace creation
- Multi-step creation dialog with task description, branch selection, agent preset
- AI-generated branch names from task description
- GitHub issue / PR linking with branch auto-fill
- **Paste clipboard images directly into the prompt input** (in addition to the existing file picker)
- File attachments appended to agent prompt
- Project onboarding flow with package manager detection and setup script configuration
- Orphan worktree detection and import
- Derivative-branch picker (icons, recency, worktree tab)
- Lazy workspace creation (Beta-gated): sidebar-plus and boot-into-Home open a client-side chat draft instead of eagerly materialising a workspace; the draft is promoted on first message send

### Workspaces overview & cross-device sync
- **Account-wide Workspaces overview** (`v0.6.1`): full-screen overlay opened from the sidebar (`Workspaces` button under `Automations`) listing every workspace this account tracks — local + every host pushed-to + every sibling device on the same account. Filters (search, project, device, status, sort), per-row actions (open, copy branch, rename, push to any host, pull back, delete), agent-state status dots shared with the sidebar, hover-reveal action menu. Configured-host buckets stay visible even when empty so a device the user has just added shows up immediately, not only after the first workspace lands on it (`v0.7.x`).
- **Cross-device workspaces sync**: `workspaces_sync.rs` mirror of `hosts_sync` / `automations_sync` — every create / rename / push / pull / delete propagates through `/api/workspaces` on the shared API server (Postgres `codemux_workspaces`) on a 30s loop, scoped per-user via Better Auth bearer tokens. Sibling-device rows render as dashed cards in the right bucket. Close-workspace paths reconcile + push the sync row immediately so the overview never briefly mis-tags a just-closed workspace as "lives on another device" (`v0.7.x`).
- **Asymmetric auto-publish from `codemux-remote` hosts** (`v0.7.x`): a new `hosts_inventory.rs` poller SSHes each configured host every ~60 s, runs `codemux-remote workspace list` (a new CLI subcommand that reads the daemon's SQLite directly — no running daemon required), and reconciles the result into `workspaces_sync` as sibling-only rows keyed by `(host_server_id, origin_uid)`. The existing 30 s push tick then uploads them to the cloud, so a workspace an agent creates on a host (via the MCP `workspace_create` tool or a manual `codemux-remote workspace register`) surfaces in every dev device's overview within ~90 s — no explicit push from the laptop required. Schema is additive (`ALTER TABLE workspaces_sync ADD COLUMN origin_uid TEXT`). The asymmetry is deliberate: dev devices keep manual push/pull (user agency), hosts auto-publish (always-on, exist to be used).
- **First-class project identity + `main`/`worktree` kind** (shipped in `v0.7.4`): a deterministic `project_uid = UUIDv5(canonical git remote ?? project_root)` (in `src-tauri/src/project_identity.rs`) gives every checkout of the same repo a stable identity across devices/hosts with zero coordination. Stamped at the source (daemon `WorkspaceStore::create` + boot sweep; desktop `set_workspace_project_root`), threaded through the host-inventory poller and the cloud `codemux_workspaces` round-trip (server-authoritative on pull), and consumed by the overview (groups by `project_uid`, renders a `main`/`worktree` badge) and the pull-conflict guard (exact `project_uid` match). See `docs/plans/project-identity.md`.
- **Sibling-device adoption** (Phase 3): "Pull to this device" on any sibling row — host-backed via the existing `workspace_pull_back_impl` rsync flow when both devices share a configured host, clone-from-git fallback for rows that have a `project_remote` but no shared host (creates a fresh server_id; both devices end up with independent copies sharing the git remote).
- **Pull-conflict guard** (`v0.7.x`): `AdoptionPreview.same_branch_project_exists_at` populates when **another local workspace** on this device matches `(basename(project_path), git_branch)` of the previewed remote row; the dialog renders `SameBranchProjectBlock` with an "Open the existing workspace" CTA and hides Pull, so a pull can't silently clobber work the user is already doing locally.
- **Safety guardrails** (Phase 4): every push gates through `ConfirmPushDialog` with a per-host "don't ask again" affordance; every successful push, pull, and adoption surfaces a 10-second `Undo` toast that fires the reverse action.
- **Cross-device divergence detection** (Phase 4c): `git_head_sha` syncs through the API; when the same workspace exists on multiple devices via clone-adoption and their HEADs diverge, both rows show an amber `diverged` chip with a tooltip suggesting push or pull to share.
- **Repo-unit sync — repo-root protection + labels** (`v0.7.8`): cross-device sync treats a git repo (root + worktrees) as one shared-history unit instead of cloning a default-branch checkout into a divergent full copy. A protected default-branch root (`WorkspaceSnapshot.protected`, stamped divergence-safely by `crate::git::is_protected_repo_root`) shows a `repo root` badge and can only be closed/detached, never deleted-as-worktree; sibling `RemoteRow` `main`-kind rows render `repo root` too. A legacy divergent full copy (`divergent_copy` — a `.git` directory in the worktrees tree) shows an amber `standalone copy` warning chip. Adoption lands a `main` row at `~/.codemux/projects/<repo>` via `create_synced_root_shell`; push/pull put a repo root under `~/.codemux/projects/<repo>` on the host (`conventional_remote_root_path`) with a legacy `worktrees/` fallback. Snapshot-local only (not synced). See `docs/plans/repo-unit-sync.md`.
- **Multi-device robustness pass** (post-`v0.7.8`, unreleased): **project-first pull** with a real protected root (local-only `default_branch` column threaded daemon→poller→sync→TS; `resolve_default_branch`/`ensure_origin_head`; `workspaces_adopt_project` materialises the root then recreates each worktree under it — the overview's **"Pull project"** button); **serialized adopts** via a per-`server_id` async creation lock (`acquire_adopt_lock`) so a double-clicked Pull or a poller race can't make duplicate shells; **client-side `dedupe_sibling_rows`** collapse of cross-device duplicate sibling cards (keeps the canonical `server_id`'d row, tombstones the rest); **daemon-side one-repo-root-per-project** (`collapse_main_for_uid` + boot `normalize_main_workspaces` sweep; `WorkspaceStore::create` collapses after inserting a `main` row); **uid-keyed collision-safe host paths** (`<basename>-<short-uid>` so `acme/api` vs `widgets/api` stop colliding, exact legacy basename layout when no uid is known); and a **non-destructive `workspaces_reconcile_copy`** action (detach-card-only, files left on disk, refuses on uncommitted/unpushed work) plus an overview **"Reconcile copy…"** menu item. See `docs/features/workspaces-sync.md` § "Robustness hardening", `docs/plans/repo-unit-sync.md`.
- **SSH tunnel health in the UI** (post-`v0.7.8`, unreleased): a dropped tunnel (sleep/wake, WiFi flap) no longer looks like a frozen workspace — `spawn_tunnel_status_forwarder` bridges the supervisor's `TunnelStatus` watch channel to a `tunnel-status-changed` event, a zustand `tunnel-status-store` (app-root `useTunnelStatusEvents` hook) feeds a sidebar pill: amber "Reconnecting…" while retrying, red "Connection lost — re-push" once the circuit breaker trips. See `docs/features/remote-hosts.md` § "Tunnel health in the UI".
- **Elapsed-time indicator** (Phase 4d): when a push or pull takes longer than 2s, a compact `12s` pill appears next to the spinner in both the sidebar row and the overview row so the user knows the operation hasn't stalled. Powered by `workspacePushPullStartedAt` in `app-store`.
- **First-run welcome banner** (Phase 5): three state-aware variants (brand-new, device-configured, has-siblings) — dismissable, persisted in localStorage, never re-shows. Bundled with the "how it works" popover off the overview header.
- **PR-state sidebar icon swap**: when a worktree workspace has a PR, the sidebar row's leading icon becomes the PR-state-colored icon (open=green, merged=purple, closed=red, draft=gray) and the trailing duplicate PR pill is gone. Click opens the PR on GitHub.

### Search & navigation
- Keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+Shift+P via fd)
- Command palette (Ctrl+K, fuzzy search across all actions)
- Local lexical indexing (`codemux index build/search`)

### Notifications
- D-Bus desktop notifications via `notify_rust::Notification` (Normal urgency so daemons auto-dismiss)
- Sidebar notification section with unread badge counts
- Workspace alerts with severity levels
- Desktop notification toast + chime when an off-screen agent finishes
- Notification sound playback wired on all three platforms (Linux: `paplay` + freedesktop `complete.oga`; macOS: `afplay` + Glass.aiff; Windows: PowerShell SystemSounds)
- **Per-worktree mute** (`set_workspace_muted` + sidebar context-menu toggle): silences agent-completion notifications for a specific workspace without touching global sound state; muted state surfaces as a sidebar row icon

### Auth & sync
- GitHub OAuth, email/password with email verification, encrypted token storage (AES-256-GCM, machine-bound key)
- **Auth module split**: `auth/{mod,api,derivation}.rs` with the `AuthSecret` typed boundary on the API helpers (compile-time guard against raw-password leaks)
- **Zero-knowledge auth derivation** (Step 10): `derive_login_credentials(password, email)` produces both the server-visible `AuthSecret` (sent to Better Auth in place of the raw password) and a client-only `EncryptionKey` (32 raw bytes, never leaves the device). Argon2id (m=64MiB, t=3, p=4) with email-bound salt, fanned out via HKDF-SHA256 to two domain-separated secrets. Cross-product byte-identical with Vexis via the shared `codemux-api-*` HKDF labels — pinned in CI.
- **End-to-end-encrypted skills sync** (Step 10, Stages 1-6): cross-device sync of user-authored skills under `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.opencode/skills/`. XChaCha20-Poly1305 per blob, machine-bound key persistence at `~/.local/share/codemux/sync-key.enc` (AES-GCM under `/etc/machine-id`). Push triggered by file watcher (1.5s frontend debounce on top of the watcher's 300ms), 5-min periodic when window is visible, or manual "Sync now" button. Last-write-wins by `updated_at`. Settings → Account → Sync surfaces live status + relative-time + Export/Import/Forgot-password controls. Multi-step reset dialog enforces export-or-explicit-skip before the destructive wipe. Production-deployed and end-to-end smoked. See `docs/features/skills-sync.md`.
- Per-user synced settings with server sync, offline cache, and dirty flag

### Presets & launchers
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode, Pi, Shell, Chat Agent) — each agent preset launches in its CLI's skip-permissions/YOLO mode
- Pin/unpin to control bar visibility
- Auto-run on workspace creation or new tab
- Partial-materialise recovery for preset DnD; new-tab preset launch
- Preset failures surface as 8-second sonner toasts (was silently `.catch(console.error)`)
- Agent context injection for Claude / Codex / Pi / Gemini presets (uses `$VAR` on POSIX, `$env:VAR` on Windows)

### File tooling
- File tree panel (right sidebar, lazy-loaded, `.gitignore`-aware, opens in built-in editor or external editor)
- Built-in file editor with CodeMirror 6, syntax highlighting for 20+ languages, markdown preview (image loading in markdown and as standalone files)
- IDE integration: detect up to 19 editors and open the workspace from the title-bar launcher or workspace context menu. Launcher and submenu partition entries into labelled sections (VS Code family / Modern editors / JetBrains / Other) when more than one family is detected. On Windows, `find_editors()` uses the `which::which()` Rust crate plus `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code / Cursor / VSCodium / Zed / Windsurf / Trae / Lapce; JetBrains stays PATH-only via Toolbox shims.
- Port detection (auto-scan, sidebar display, open in browser). Windows path filters system processes (`svchost.exe`, `System`, `lsass.exe`, etc.) so the sidebar doesn't surface 16+ kernel-owned ports. **Docker-published container ports** for open codemux worktrees surface under a dedicated **Docker** group (via `docker ps`, scoped by the compose `working_dir` label) — recovering ports the Linux `/proc` scan can't see because `docker-proxy` runs as root; kill is hidden for these rows. See `docs/features/ports.md`.

### Theming
- Neutral dark shell theming with Omarchy accent sync
- Sans-serif chrome (DM Sans), monospace terminals
- Terminal colors fully theme-reactive via CSS variables + MutationObserver
- Fallback Tokyonight-inspired theme when Omarchy unavailable
- **Refined-minimal sidebar redesign**: slim Changes panel + ADE-native right sidebar, consolidated title-bar menu into the sidebar footer, aligned preset-bar icons + tab-bar drop indicator, sidebar workspace rows redesigned with new hover/active states
- **Refined-minimal Settings panel**: every section now uses shared primitives, back-button hit area widened, panel layout matches the sidebar aesthetic

### Agent Chat (Beta — opt-in via Settings → Beta Features)
- Full multi-provider chat pane with streaming, approvals, mode pills (Ask / Allow always / Plan / Debug), and permission-mode restart
- **Three providers** behind one unified picker: **Claude** (Claude Agent SDK via Bun-compiled `claude-agent` sidecar), **Codex** (`codex app-server` JSON-RPC), **OpenCode** (federated — Rust-direct HTTP against a managed `opencode serve` child, 100+ upstream providers funneled through one rail entry). See `docs/features/multi-provider-chat.md`.
- Unified provider+model picker (2-column popover: provider rail + searchable model list); favorites with `localStorage` persistence
- Codex finally GUI-selectable (was hidden behind a stale `ENABLE_PROVIDER_PICKER` flag pre-Step 12)
- Session history selector + draft surface chrome polish + plan proposals + AskUserQuestion panel + thinking indicator
- **Attachments via `+` and `@`**: files, folders, GitHub issues + PRs, images via paste / drop / + picker. Inline chips, send-time injection, expand, caps, gif guard, chip tooltips
- Slash command popup + Shift+Tab mode cycling
- Cross-provider **skill system** (watcher, conflicts, disable, refined compat)
- Per-tool body rendering in approval blocks (read/write/edit/grep/etc.)
- **MCP host runtime**: Codemux discovers user-installed MCP servers across Codemux / Claude / Cursor paths, spawns each child once, dedupes identical configs, exposes tools to the Claude SDK via an in-process facade with dynamic `setMcpServers` refresh. Settings panel and `+` popup surface enable/disable + status badges + tool list modal + 50-tool cap warning. Codex MCP support planned for Step 11 via HTTP gateway.
- Permissions settings page with per-tool body rendering
- Wired Debug mode pill with marker cleanup flow
- Plain-quit on Beta toggle off (no auto-restart)
- See `docs/features/agent-chat.md` for the canonical feature breakdown

### Infrastructure
- Global overlay manager (single overlay at a time)
- MCP server exposing **52 tools** via JSON-RPC 2.0 (browser tier 1/2/3 + info + viewport, workspace incl. open/close, pane incl. close, git, notification, terminal read/write, app_status, port_list, worktree_create, preset_apply/list, issue_list/get/link_workspace, automation list/get/create/update/delete/pause/resume/runs — Phase 1/1.5/1.6 vexis-agent integration tools and the eight `automation_*` tools all merged)
- CLI and socket control (Unix socket on Linux/macOS, named pipe on Windows). Control-endpoint errors now surface instead of being swallowed.
- Per-workspace display isolation (X11/Wayland sandboxing for agent-spawned GUI apps — opt-in for human persona, default-on for agent persona)
- Local project memory (`codemux memory show/set/add`, `codemux handoff`)
- Auto-update via Tauri updater (Linux AppImage + Windows NSIS, signed with the same Ed25519 key, shared `latest.json`)
- Onboarding skip affordance + re-trap fix
- Dev builds isolated from installed release (separate data dirs)
- **Remote hosts + workspace push**: Settings → Hosts with real `hosts_test_connection` probe + `hosts_bootstrap_install` install flow, full `codemux-remote` server binary (`[[bin]] codemux_remote.rs`), and SSH transport (`ssh::probe`/`bootstrap`/`tunnel`/`tunnel_supervisor`/`push`/`registry`) so a workspace can be pushed to a user-owned SSH host. Push synchronizes the worktree, spawns the remote daemon, attaches the local UI through an SSH-forwarded socket, and **syncs the Claude conversation** across local/remote ends. `WorkspaceSnapshot.host_id` + shared `<DevicePicker>` pill wires the host selection into the new-workspace dialog. **Background `hosts_upgrade` poller** (`hosts_upgrade.rs`) walks every registered host ~5 s after app start and re-bootstraps any whose `codemux-remote` version differs from the bundled binary — users never see the upgrade. Post-`v0.7.8` it probes the daemon's `live_terminals` first and **defers** the unit restart while host agents are running, so an auto-upgrade never silently kills host-side work. **Background `hosts_inventory` poller** (`hosts_inventory.rs`) walks every configured host every ~60 s, runs `codemux-remote workspace list` over SSH, and reconciles the host's workspace registry into `workspaces_sync` so host-created workspaces auto-publish to the user's cloud registry without an explicit push (see "Workspaces overview & cross-device sync"). SCP was replaced by an `ssh-cat` pipeline to work around OpenSSH 9+ tilde-expansion. `ssh::probe` falls back to `~/.local/bin/codemux-remote` when the PATH lookup fails so non-interactive SSH on Arch/Ubuntu/Fedora (which doesn't source `~/.profile`) still finds the binary.
- **MCP-on-remote (headless Codemux daemon)**: `codemux-remote serve` runs an axum HTTP server on loopback at a port recorded in `<state-dir>/manifest.json` (mode `0600`, includes a 32-byte bearer secret + `host_id` + `owner_id` reserved for a future relay). `codemux-remote mcp` is a stdio JSON-RPC MCP bridge that reads the manifest and forwards `tools/call` to the daemon over HTTP. The headless tool catalog (separate from the desktop's 52) is **12 tools**: `workspace_{create,list,info,update,close}`, `worktree_create` (added `v0.7.5`), `terminal_{spawn,write,read,list,close}`, `app_status`. Self-contained module at `src-tauri/src/remote/` (`manifest.rs`, `auth.rs`, `identity.rs`, `workspace.rs`, `pty.rs`, `server.rs`, `mcp.rs`, `mcp_register.rs`, `tools/mod.rs`, `git.rs`). `Identity` enum reserves a `Cloud { user_id, org_id, role }` variant for a future paid-tier relay without changing handler signatures. Push auto-provisioning: `ssh::bootstrap::provision_serve` installs the systemd user unit + `loginctl enable-linger`; `ssh::push::push_workspace` drops a workspace-scoped `.mcp.json`; `ssh::bootstrap::register_workspace_on_remote` calls `codemux-remote workspace register` so the pushed workspace shows in `workspace_list` from any agent on the host. On every `serve` startup, `mcp_register.rs` idempotently inserts a `codemux` MCP entry into `~/.claude.json` / `~/.codex/config.toml` / `~/.cursor/mcp.json` so user-level (not just per-workspace) agent sessions also discover Codemux as an MCP server. Desktop-side Step 1 (extract `codemux_core`), Step 5 (pull-workspace UI), Step 6 (`--host` CLI flag), and Step 9 (migrate desktop transport to HTTP+manifest) are explicitly deferred — the headless daemon ships with its own self-contained registry and tools instead of forcing the Tauri-coupled extraction first. See `docs/plans/mcp-on-remote.md`.

### Automations
- **Scheduled agent runs**: a named prompt + agent + RFC 5545 recurrence that fires on a user-chosen host. Each fire creates an isolated git worktree, runs the agent headlessly (`claude --print` / `codex exec`), and records a real `succeeded` / `failed` / `skipped_offline` / `skipped_busy` terminal status. Same-automation overlap is serialised; a per-minute `fire_key` keeps a double tick idempotent.
- **Automations view**: a first-class destination opened from the left sidebar (under "New agent", above the project list) — list + detail pane for create / edit / pause / resume / delete, a frequency/time/weekday schedule builder with a raw RFC 5545 escape hatch, per-automation run history, and a per-row health dot driven by the last run.
- **Account sync**: `automations_sync` replicates the registry through the live `/api/automations` endpoints with the same dirty-flag / tombstone model as `hosts_sync`, so every signed-in device sees the same list; `automation_runs` stay per-device.
- **Host routing**: the desktop scheduler runs only `host_id IS NULL` automations; `codemux-remote scheduler` — a systemd user service provisioned at host bootstrap — runs host-targeted ones on an always-on machine. A stuck-run reconciler fails crashed runs at scheduler startup so a dead run can't pin its automation in `skipped_busy`.
- **GitHub backbone**: a remote host obtains the project repo by cloning / fetching its git remote with the host's own credentials (no token injected); a per-repo `git ls-remote` preflight flags an unreachable repo at setup, not at the first fire.
- Surface: seven `automations_*` Tauri commands + `automations_check_repo_access`, and eight `automation_*` MCP / control-socket tools. See `docs/features/automations.md`.

### Performance
- High-frequency app-state emits coalesced into 16 ms windows
- `transition-all` scoped to actually-transitioning properties
- Markdown view + workspace-tied components no longer re-render on every backend tick
- Workspace-switch mount-time IPC roundtrips cut; IPC thread unblocked
- Editor file read + language module import parallelised
- Worktree-include listener no longer re-attaches every backend tick
- `ensure-draft-when-empty` effect uses a primitive fingerprint
- Chat transcript rows + file-tree nodes memoised to skip per-token re-renders

## Partial / Being Hardened

- **OpenFlow**: orchestration works but large-run reliability and intervention flow still maturing. Backend-driven orchestration loop (5s active / 15s blocked). Disabled on Windows until the bash-wrapper rewrite lands.
- **Browser pane**: screenshot-driven, functional but lower fidelity than a native embedded webview
- **AI merge resolver**: backend and frontend working, recent fixes (close stdin + kill child on timeout, skip-permissions flags, blocking-pool offload), needs more depth of live validation
- **Browser automation depth**: DOM commands, coordinate commands, OS-level input, wait conditions, JS evaluation, CSS style inspection — all working; toolbar back/forward/reload still need focused validation

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Agent Chat is **off by default**; opt in via Settings → Beta Features
- Memory drawer UI is still backend + CLI only (no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor
- Context menus on pane headers are not yet implemented (workspace rows, section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Browser automation uses `agent-browser` v0.24.0 (pure Rust binary, direct CDP). The legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation are gone.
- Feature docs exist for all major subsystems (see `docs/INDEX.md`)

## Windows Support

Windows support shipped in `v0.1.20` and `v0.1.21` and has been hardened progressively through every subsequent release. Latest published Windows binaries (NSIS `.exe` installer + auto-update via the shared `latest.json`) ship on `v0.7.8`; in-app auto-update on Windows was fixed in `v0.5.1`. The MCP-on-remote `codemux-remote serve` daemon is Unix-only by design — the Windows build of that binary is a no-op stub that prints the rationale and exits.

What's in place:

- `cfg`-gates cover every Linux-specific code path — the app compiles on `x86_64-pc-windows-msvc` without unsafe `unix` stubs
- Control socket → named pipe (`\\.\pipe\codemux-{username}`) via `tokio::net::windows::named_pipe`. Client now retries on `ERROR_PIPE_BUSY`.
- Port detection via `netstat -ano` parser (cross-platform pure function, unit-tested on Linux CI) with a Windows system-process name filter
- Agent-browser port reclamation via `netstat -ano` + `taskkill` with exact-port matching; auto-detect installed Chromium (Edge / Chrome / Brave / Chromium)
- `portable-pty` fork pinned to `Zeus-Deus/portable-pty@codemux-0.8.1-no-window` (`STARTF_USESHOWWINDOW + SW_HIDE` so PTY spawns don't flash a `cmd.exe` console window)
- **PowerShell is the default Windows shell** (`pwsh` → `powershell` → `COMSPEC` → literal `"cmd.exe"`). Agent context injection uses PowerShell `$env:VAR` syntax; preset commands terminate with `\r`. Gemini path writes its system-prompt temp file via PowerShell `Set-Content -NoNewline`.
- Editor detection uses `which::which()` + `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code, Cursor, VSCodium, Zed; JetBrains stays PATH-only
- `<WindowChrome />` extracted so login, empty-state, settings, and new-project screens have minimize/maximize/close buttons (Codemux runs with `decorations: false`)
- Scrollback flush waits 10s on Windows (3s elsewhere); Windows-only `scrollback::flush_cache_to_disk` backend backstop catches anything the frontend can't persist before timeout
- OpenFlow disabled at the UI + backend level on Windows (bash wrappers not yet ported)
- `release.yml` builds on `[ubuntu-22.04, windows-latest]` with `fail-fast: false`; tauri-action merges both platforms into a single `latest.json`
- NSIS installer produced on Windows CI (`--bundles nsis` to skip MSI which needs WiX)
- Claude Code hooks register and execute on Windows
- Tier-3 OS-level input injection via Win32 `SendInput`
- Path normalization + four latent windows portability issues fixed

Still gated before a polished Windows v1:

- Windows Authenticode code signing (SmartScreen friction expected on unsigned first-install; deferred behind a cert budget decision)
- OpenFlow bash wrapper rewrite (blocks OpenFlow on Windows)
- Full PTY lifecycle / worktree / agent-spawn integration tests on a live Windows runner

See `docs/plans/windows-support.md` for the complete checklist.

## React Frontend Status

The frontend is React + Tailwind v4 + shadcn + Vite. The Rust backend is unchanged. The old Svelte frontend has been removed.

### Working

- App shell: shadcn Sidebar with collapsible workspace sections, tab bar, right panel
- Workspace list from real Tauri backend data (zustand + app-state-changed events, coalesced into 16ms windows)
- Terminal panes with xterm.js WebGL + DOM fallback + PTY via Tauri Channel, persistent across workspace switch
- Pane splits (horizontal/vertical) with CSS Grid, resize handles, drag-to-swap
- Right panel with Changes panel, File tree, and Review (PR) panel tabs
- OpenFlow UI: orchestration view, agent config, communication panel, agent graph
- Agent Chat UI: chat pane, composer (with `+` popup, `@` mention popup, slash command popup, image paste/drop), transcript, mode pill, model picker, session selector, attachment chips, plan proposal block, AskUserQuestion panel, thinking indicator, permission request block, tool-call card with per-tool body rendering, debug-mode banner + exit dialog
- Settings panel (15+ sections including Beta Features, Sync, Skills, MCP, Permissions)
- Command palette (Ctrl+K) with fuzzy search
- Search: file name search (Ctrl+Shift+P) and content search (Ctrl+Shift+F)
- Browser pane with screenshot-driven rendering and toolbar (reactive `stream_url` reconnect)
- Workspace drag-and-drop reordering in sidebar
- Terminal presets bar with quick-launch
- Auth system with GitHub OAuth, email/password, encrypted token storage
- Synced settings (per-user server-synced with offline cache)
- Skills sync UI (Settings → Account → Sync)
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Tauri bridge: 120+ typed command wrappers, 12+ event helpers, all types ported

### Remaining Gaps

- Context menus on pane headers (workspace rows, workspace section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Memory drawer UI (backend memory system exists, CLI works, no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/*` for subsystem detail
- `docs/plans/windows-support.md` for the cross-platform checklist
- `docs/plans/openflow.md` for the active OpenFlow hardening work
