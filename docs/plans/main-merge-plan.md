# Plan — Merge `origin/main` into `feature/agent-chat`

- Purpose: Investigate the divergence and lay out the safest path to merge `origin/main` into `feature/agent-chat`.
- Audience: The operator and the agent who will execute the merge.
- Status: **Investigation only.** Nothing has been merged or modified. Decision (GO / MODIFY / DEFER) is the operator's.
- Read next: `docs/plans/step-10-skills-sync.md` (auth derivation context), `docs/features/auth.md` (target structure on main).

## 1. Pre-merge backup tag

Working tree is clean (only the untracked research file `docs/plans/step-13-beta-toggle-research.md`).

Tag the current `feature/agent-chat` HEAD before any merge attempt:

```bash
git tag pre-main-merge-$(date +%Y%m%d-%H%M%S)
git tag --list 'pre-main-merge*'
```

The recommended tag name to use when executing: **`pre-main-merge-YYYYMMDD-HHMMSS`** (timestamp set at execution time). Print it back into the post-execution log so the rollback path can be replayed verbatim.

Current HEAD that the tag will point at: `6b4e472 fix(ci): gate opencode spawn-against-non-opencode-binary test to unix`.

If anything looks off mid-merge: `git merge --abort` and verify HEAD still resolves to that commit.

## 2. Diff audit

- **Merge base**: `a344a63` ("chore: bump version to 0.1.26")
- **Prior merge into the branch**: `6850f29` (2026-04-19) merged main at 0.1.26 — most pre-divergence content already absorbed there.
- **Commits on `origin/main` since merge-base**: **52** (0.1.27 → 0.1.33 + Windows + auth refactor + sidebar/changes-panel polish + revert saga).
- **Commits on `feature/agent-chat` since merge-base**: **96** (Step 10 skills sync, Step 11 polish, Step 12 OpenCode + multi-provider chat).
- **Files only on main**: 94. **Files only on agent-chat**: 332. **Files on both**: 26.

### Top main-only highlights (additive — low merge risk)

- Onboarding skip affordance, terminal Unicode/WebGL/persistence, sidebar PR icon, "Checkout default branch", PR-tab→Review React-Query refactor, terminal pause-when-hidden, desktop notification toast.
- Windows portability work: portable-pty repin, Win32 SendInput tier-3, ERROR_PIPE_BUSY retry, `resolve_binary` Windows branch, Edge auto-detect, hooks register on Windows, agent-browser argv fix, single-instance/control-pipe fixes.
- Auth refactor split (commit `0c38e20`): `auth.rs` → `auth/{mod,api,derivation}.rs` + Argon2id timing relax (`3f65bf2`).

### Top agent-chat-only highlights (already on this branch)

- Steps 6–12 of the agent-chat feature branch: sidecar, Codex/Claude/OpenCode adapters, draft surface, modes, attachments, skills sync, multi-provider picker, dynamic capability harvest.

## 3. Conflict surface (authoritative — `git merge-tree` virtual merge)

`git merge-tree --write-tree origin/main feature/agent-chat` (read-only, no working-tree changes) returns exit-code 1 with **13 real content conflicts**:

| File | Diff size | Notes |
|---|---|---|
| `src-tauri/src/lib.rs` | +198 / -196 | Both branches reshuffled the `invoke_handler!` list; resolve as **union of both command lists** |
| `src-tauri/src/execution/mod.rs` | +694 / -19 | Main reverted env-isolation scaffolding; agent-chat is the superset (see §4.2) |
| `src-tauri/src/github.rs` | +623 / -86 | Main's PR-status icon / Review-tab work; agent-chat largely untouched in this file |
| `src-tauri/src/commands/auth.rs` | +374 / -35 | Both branches restructured; agent-chat adds Step-10 sync commands (see §4.1) |
| `src-tauri/src/commands/workspace.rs` | +148 / -81 | Main added "Open ↵ main" + checkout-default; agent-chat added pane-scoped chat fields |
| `src-tauri/Cargo.lock` | +148 / -16 | Always-regenerable; preferred resolution = `--theirs Cargo.lock` then `cargo check` to rewrite |
| `src-tauri/Cargo.toml` | +43 / -19 | See §4.3 — most conflict-prone of the small files |
| `package-lock.json` | +98 / -36 | Same regenerate strategy via `npm install` |
| `src/App.tsx` | +29 / -4 | Probably overlay registration only |
| `src/components/layout/sidebar-workspace.test.tsx` | +232 / -26 | Main rewrote suite; agent-chat added rows (compose) |
| `docs/core/STATUS.md` | +8 / -5 | Both branches updated current state — manual narrative resolve |
| `src/components/overlays/new-project-screen.tsx` | +9 / -1 | Trivial |
| `src/hooks/use-project-actions.ts` | +16 / -6 | Trivial |

Auto-merged but **needs eyeballing** (git did the wrong thing on the auth rename — see §4.1):

| File | Reason to re-check |
|---|---|
| `src-tauri/src/auth/mod.rs` (= ex-`auth.rs`) | Git treated this as a rename and wholesale dropped agent-chat's 1283-line file into `auth/mod.rs` — the inline Step 10 derivation block must be **manually relocated** to `auth/derivation.rs`. |
| `src-tauri/src/commands/files.rs` | +242 / -67 — main extracted `reveal_in_file_manager`; agent-chat added attachment helpers. Verify both survived. |
| `src-tauri/src/commands/github.rs` | +36 / -7 — main added open-PR helpers. |
| `src-tauri/src/commands/mod.rs` | +134 / -6 — agent-chat is a strict superset (9 new modules); compose. |
| `src-tauri/src/hooks.rs` | +22 / -126 — main slimmed (Windows fix); ensure agent-chat hooks code we depend on is preserved. |
| `src-tauri/tauri.conf.json` | +5 / -4 — version bump + window flags; verify. |
| `src/components/layout/sidebar-workspace-row.tsx` | +96 / -175 — main rewrote with PR icon + checkout-default; ensure agent-chat add-ons preserved. |
| `src/components/overlays/new-workspace-dialog.tsx` | +23 / -40 — verify chat-agent preset filtering still applies. |
| `src/components/workspace/changes-panel.tsx` | +67 / -141 — main migrated to React Query; agent-chat added base-branch picker. |
| `src/tauri/commands.ts` | +647 / -11 — agent-chat is a near-strict superset; verify. |
| `src/tauri/types.ts` | +250 / -1 — agent-chat is a near-strict superset; verify. |
| `package.json` | +6 / -3 — small. |
| `docs/INDEX.md`, `docs/reference/ARCHITECTURE.md` | Both small additive; eyeball doc-list union. |

## 4. Per-file resolution strategy

### 4.1 Auth restructure — the headline

The state we want is: **main's directory layout** (`auth/{mod,api,derivation}.rs`) **with agent-chat's enriched derivation** (AuthSecret + EncryptionKey + `derive_login_credentials`).

What's where today:

- `origin/main:src-tauri/src/auth/mod.rs` — 696 lines, contains the auth state, OAuth flow, token storage. Re-exports `derive_auth_secret`, `AuthSecret` from `derivation`. Calls `api::login_email_api`, `api::signup_email_api`.
- `origin/main:src-tauri/src/auth/api.rs` — 370 lines, HTTP helpers that take `AuthSecret` (typed), with log-leak regression test using `gag` dev-dep.
- `origin/main:src-tauri/src/auth/derivation.rs` — 537 lines, **AuthSecret only** (Codemux has no E2E on main). Cross-product pin tests reference Vexis's golden values.
- `feature/agent-chat:src-tauri/src/auth.rs` — 1283 lines, single file with Step-10 derivation block inlined at lines 657–906. Block adds `EncryptionKey` (`pub struct EncryptionKey([u8; 32])` with `Drop` zero-write), `derive_login_credentials` returning `(AuthSecret, EncryptionKey)`, and `encryption_key_matches_vexis_for_known_input` test pin. TODO at line 660 documents the deferred relocation.

Verified: both branches use the same protocol constants — `MASTER_SALT_DOMAIN = b"codemux-api-master-v1\0"`, `AUTH_SECRET_INFO = b"codemux-api-auth-secret-v1"`, Argon2id m=64MiB / t=3 / p=4. The derivation byte-output is identical for the same input. Cross-product login is preserved by either path.

What `git merge-tree` produces today (broken, must be fixed):
- `auth/mod.rs` ← agent-chat's whole `auth.rs` (the big inlined-Step-10 file gets renamed to `auth/mod.rs` because git detected a 1↔1 rename).
- `auth/derivation.rs` ← main's AuthSecret-only file (untouched).
- `auth/api.rs` ← main's typed helpers (untouched).
- Result: `EncryptionKey` lives in `auth/mod.rs`, `derive_login_credentials` lives in `auth/mod.rs`, the TODO is still there, and the cross-product encryption-key pin test never gets re-homed to `derivation.rs`. Compilation may even succeed but the architecture is wrong.

**Manual relocation steps (post-`git merge`):**

1. Open `src-tauri/src/auth/mod.rs` (the merged file). Locate the Step 10 block (lines marked `Zero-knowledge auth credential derivation (Step 10 — Skills Sync)`).
2. Cut the block: every `use` import for `argon2`, `hkdf`, `base64::Engine`; every constant (`ARGON2_M_COST_KIB`, `ARGON2_T_COST`, `ARGON2_P_COST`, `AUTH_SECRET_LEN`, `ENCRYPTION_KEY_LEN`, `MASTER_SALT_DOMAIN`, `AUTH_SECRET_INFO`, `ENCRYPTION_KEY_INFO`); the `AuthSecret` and `EncryptionKey` structs (with `Drop`/`Debug`/`Clone` impls); the `derive_login_credentials`, `derive_auth_secret`, `derive_master_material`, `normalize_email` functions; and **all** Step-10 derivation tests including `auth_secret_matches_vexis_*` (single + multiple-input) and `encryption_key_matches_vexis_for_known_input`.
3. Open `src-tauri/src/auth/derivation.rs` (currently main's AuthSecret-only). **Delete** main's `AuthSecret`, `derive_auth_secret`, `derive_master_material`, `normalize_email`, and tests — the agent-chat versions are protocol-identical and a strict superset.
4. Paste the cut block into `derivation.rs`. Keep the file header rationale comment (the block-of-text at the top of agent-chat's section is more accurate than main's).
5. In `auth/mod.rs`: replace the now-deleted block with a `pub use derivation::{AuthSecret, EncryptionKey, derive_auth_secret, derive_login_credentials};` re-export (extending the existing re-export line).
6. Drop the TODO comment that pointed at the relocation.
7. Verify `auth/api.rs` still compiles — it imports `crate::auth::AuthSecret` which now resolves through the re-export.
8. Run `cargo test --manifest-path src-tauri/Cargo.toml --lib auth::derivation` and confirm the cross-product hex pins match Vexis (`auth_secret_matches_vexis_for_known_input`, `auth_secret_matches_vexis_across_multiple_inputs`, `encryption_key_matches_vexis_for_known_input`).

### 4.2 `execution/mod.rs` — the env-strip revert collision

Main's history on this file:
1. `9401c93` "split GUI policy by persona — humans keep DISPLAY" (added `Persona` enum, sanitize-only-for-agents).
2. `885f64c` "make GUI env isolation opt-in, restore Ctrl+V image paste" (kept scaffolding, switched defaults).
3. `6e5c99a` "revert: undo env-isolation saga — restore plain env inheritance" — **removed the scaffolding entirely**. Main's `execution/mod.rs` is now 205 lines and contains zero `sanitize_gui_env_*` or `gui_env_keys` symbols.

Agent-chat still uses `sanitize_gui_env_std`/`sanitize_gui_env_tokio` in **18 source files** (`agent_browser.rs`, `agent_provider/opencode/{discovery,server}.rs`, `ai.rs`, `auth.rs`, `commands/{files,git,mod,openflow,workspace}.rs`, `git.rs`, `github.rs`, `mcp/runtime.rs`, `mcp_server.rs`, `ports.rs`, `scripts.rs`, `session_adapters.rs`, `execution/mod.rs`). Branch CLAUDE.md still mandates calling them.

**Resolution: keep agent-chat's superset** (880 lines, with the helpers). Justification:
- The functional default on main today is "inherit env"; agent-chat's helpers no-op when the policy doesn't strip, which is the same behavior unless a user explicitly opts in.
- Removing the helpers means rewriting 18 files and dropping CLAUDE.md guidance — high blast radius for zero functional gain.
- Main's persona-aware refinements (commits 9401c93, 885f64c) were partly walked back by 6e5c99a. The portions still useful (Persona enum on `TerminalPreset`, `worktree_session_default_for_persona`) overlap with agent-chat's chat_agent preset machinery and need a manual reconciliation pass: skim the union, drop dead code, keep the persona enum if it's wired through frontend `TerminalPreset` types.

When resolving the merge: take **`--ours`** (agent-chat) for `execution/mod.rs`, then forward-port any genuinely new public API from main if it isn't already represented. Verify with `cargo check` then run the `tests/persona_execution.rs` integration suite if main's tests file lands cleanly.

### 4.3 `Cargo.toml` — six independent decisions

| Line | main | agent-chat | Decision |
|---|---|---|---|
| `version` | `0.1.33` | `0.1.26` | **Take main** (don't downgrade tagged release line) |
| `default-run = "codemux"` | absent | present | **Take agent-chat** (needed once we add fake_* test binaries) |
| `portable-pty` | `"0.8.1"` (upstream) | git-fork `codemux-0.8.1-no-window` | **Take main** — the fork was repined upstream by `06c0301` after fixes landed; using upstream is the canonical version |
| `reqwest` features | `["blocking", "json"]` | `["blocking", "json", "stream"]` | **Take agent-chat** — `stream` is required by the OpenCode SSE adapter |
| `[target.'cfg(windows)'].dependencies.windows-sys` | present (Win32 SendInput, EnumWindows) | absent | **Take main** — needed by tier-3 input injection landed in `d3cd101` |
| `[dev-dependencies] tauri = { features = ["test"] }` | absent | present | **Take agent-chat** — Step 9 integration tests use `tauri::test::mock_app` |
| `[target.'cfg(unix)'.dev-dependencies] gag = "1"` | present | absent | **Take main** — needed by `auth/api.rs` log-leak regression guard test |
| Agent-chat-only deps (`chacha20poly1305`, `async-trait`, `futures-core`, `serde_yaml`, `glob`, `ignore`, `nucleo-matcher`) | absent | present | **Take agent-chat** — all referenced by Step 10–12 features |
| Agent-chat-only `[[bin]]` blocks (`fake_rpc_child`, `fake_codex_app_server`, `fake_claude_sidecar`) | absent | present | **Take agent-chat** — referenced by tests |

Net manual edit on `Cargo.toml`: take agent-chat as the base, change `version = "0.1.26"` → `"0.1.33"`, swap `portable-pty` to the upstream `"0.8.1"` form, append the windows-sys block, append the `gag` unix-only dev-dep block.

### 4.4 `Cargo.lock`, `package-lock.json`

Don't try to merge by hand. Take **agent-chat's** version of both lockfiles for the merge commit (it has the agent-chat dependency tree), then:

```bash
cd src-tauri && cargo check     # regenerates Cargo.lock for the new Cargo.toml
cd ..       && npm install       # regenerates package-lock.json for the new package.json
git add src-tauri/Cargo.lock package-lock.json
```

`cargo check` will fail loudly if `portable-pty` upstream + the windows-sys deps don't resolve — that's the signal to fix the Cargo.toml.

### 4.5 `lib.rs` — `invoke_handler!` union

198 + 196 line churn — both branches reordered the registration list. Strategy: take **agent-chat as the base** and patch in main's new commands. agent-chat's new modules (`agent_chat`, `mcp`, `opencode`, `permissions`, `project_files`, `skills`, `skills_sync`, `virtual_display`) are not present on main; main's new `commands` (`open_pr_for_workspace`, `checkout_default_branch`, `refresh_branch_info_all`, etc.) need to be appended to the existing list.

Sanity check after merge: `cargo check` will refuse if any registered command name doesn't resolve. There's no orderdependence in `tauri::generate_handler!` — alphabetize where it helps readability.

### 4.6 `commands/auth.rs`

Three Step-10 commands on agent-chat must survive: `get_sync_status`, `setup_sync_password`, `provide_password_for_sync`. Main reorganized signin/signup to call `auth::api::login_email_api`/`signup_email_api` with `AuthSecret`. Resolution:

1. Take main's `signin_email`, `signup_email`, `forgot_password`, `start_oauth_flow`, `check_auth`, `sign_out`, `get_auth_token` definitions (they use the new typed API).
2. Re-add agent-chat's three Step-10 commands. Update them to call `crate::auth::derive_login_credentials(password, email)` (now in `auth/derivation.rs`) instead of the old inline derivation, and to use the new typed `AuthSecret` for any `/api/auth/desktop/*` round-trips.
3. Re-register all three in `lib.rs`.

### 4.7 Frontend conflicts (`App.tsx`, `sidebar-workspace.test.tsx`, etc.)

Generally additive. Where a real conflict appears, prefer agent-chat's structure (it's the working chat UI surface) and patch main's adjustments in.

For `sidebar-workspace.test.tsx` — the +232/-26 hunk — main rewrote test setup to cover PR icon + checkout-default. Take main's harness updates wholesale, then re-add any agent-chat-specific assertions.

### 4.8 Docs

- `docs/INDEX.md`: agent-chat is a superset (added Step 10/11/12 entries). Main added nothing material — treat agent-chat as authoritative.
- `docs/core/STATUS.md`: real conflict. Manually rewrite the "Current State" narrative to reflect both: the version bumps + Windows port + checkout-default + Review tab from main, and Step 10–12 from agent-chat. Keep the section as a single coherent picture.
- `docs/reference/ARCHITECTURE.md`: small additive; eyeball the layer table.
- `AGENTS.md` / `CLAUDE.md`: agent-chat already has the spawn-sanitizer rules; main's CLAUDE.md is shorter (51 vs 59 lines). Take agent-chat as base, forward-port any new main-only rules (none found in this audit, but verify).

## 5. Strategy options

### Option A — Standard `git merge origin/main` (recommended)

```bash
git tag pre-main-merge-$(date +%Y%m%d-%H%M%S)
git merge origin/main
# resolve conflicts file-by-file per §4
git add ...
git merge --continue   # creates the merge commit
```

**Pros**:
- Single merge commit preserves both histories visibly. Future operators can read `git log --first-parent` on the branch and see "merged main 0.1.26→0.1.33" as one event.
- Conflict resolution happens in one focused session; we already have the per-file plan.
- `git merge-tree` already showed us the exact conflict set — no surprises.

**Cons**:
- ~13 real conflicts to resolve in one go. Operator must stay focused.
- Lockfile regeneration requires a working Rust + npm toolchain.

### Option B — `git rebase origin/main`

```bash
git rebase origin/main
# resolve conflicts per-commit (96 commits)
```

**Pros**: Linear history.

**Cons**:
- 96 commits to rebase, each potentially hitting the auth.rs↔auth/{api,derivation,mod}.rs path collision. The same conflict re-emerges on every rebase pick that touches auth, which is dozens of commits (Step 10 onwards).
- Rebase hides the "main was merged" event from the topology — bad audit trail for a coordinated cross-product change like the auth-derivation port.
- Force-pushes are required on the feature branch afterwards.

**Recommendation: don't.**

### Option C — Cherry-pick main commits onto agent-chat

```bash
for c in $(git log --reverse --format=%H origin/main ^feature/agent-chat); do
  git cherry-pick "$c"
done
```

**Pros**: Fine-grained — could selectively skip commits.

**Cons**:
- 52 commits to cherry-pick, many of which (auth refactor, env-isolation revert chain, persona changes) aren't independently apply-able. You'd hit the same conflicts as Option A but smeared across 52 commit boundaries instead of one.
- Creates 52 new commit hashes on the feature branch — unrelated to upstream main, visually noisy.

**Recommendation: don't, unless we end up wanting to skip specific main commits (e.g., the env-isolation revert if we choose to keep agent-chat's scaffolding).**

### Recommended path: **Option A** (single `git merge` commit with manual resolution per §4).

## 6. Verification checklist

Run in this order. Stop on first failure and investigate.

1. **Compile clean**: `cargo check --manifest-path src-tauri/Cargo.toml`. Must succeed before considering anything else done. Agent-chat-side imports (`sanitize_gui_env_std`, `EncryptionKey`, etc.) often surface here.
2. **Cross-product hex pins** — *the most important test*: `cargo test --manifest-path src-tauri/Cargo.toml --lib auth::derivation` (or, if tests are still in `auth/mod.rs::tests`, run the whole `auth` module). Required passes:
   - `auth_secret_matches_vexis_for_known_input` (single golden value)
   - `auth_secret_matches_vexis_across_multiple_inputs` (5 cases)
   - `encryption_key_matches_vexis_for_known_input` (E2E key half — only on agent-chat side, must not be lost during relocation)

   Failure here means we silently broke cross-product login or skills sync — DO NOT push.
3. **Full Rust suite**: `cargo test --manifest-path src-tauri/Cargo.toml --lib`. Baseline before merge: **1363 tests on agent-chat**. After merge expect ≥ 1363 plus main's net additions (likely ~13–25 new tests from `persona_execution.rs` if we keep main's scaffolding, otherwise just the few extra tests added in `auth/api.rs` and `auth/derivation.rs`). A drop below 1363 is a regression.
4. **Integration tests**: `cargo test --manifest-path src-tauri/Cargo.toml --tests`.
5. **Frontend type-check + tests**: `npm run check && npm run test`. Baseline before merge: **1769 vitest tests**. Expect ≥ 1769 after merge.
6. **Top-level verify**: `npm run verify`.
7. **Smoke the chat surface (manual)**: `npm run tauri dev` → spawn chat pane → send a turn → verify approvals, mode pills, skills, attachments, image paste (Ctrl+V) still all work. Especially the Ctrl+V image paste since main's revert chain was specifically motivated by that breaking.
8. **Smoke main-only features (manual)**: open-PR button on the changes-panel toolbar, "Checkout default branch" action on a workspace row, sidebar PR icon visible after a `gh` login, terminal Unicode rendering.
9. **Sanity-check the auth flow end-to-end**: sign out, sign in via email/password, observe AuthSecret reaches the API, set up sync password (Step 10), restart, verify ProvidePasswordForm shows for OAuth users with sync set up.

## 7. Rollback plan

The merge commit is reversible up until it's pushed. Two rollback paths:

### 7.1 Mid-merge abort (no merge commit yet)

```bash
git merge --abort
git status   # working tree should be clean
git log -1   # HEAD should still be 6b4e472
```

### 7.2 After merge committed but before push

```bash
git reset --hard pre-main-merge-YYYYMMDD-HHMMSS    # the tag from §1
git status                                          # clean
git log -1                                          # back at 6b4e472
```

**Destructive operation guard**: `git reset --hard` discards uncommitted work. Don't run it without explicit operator confirmation (auto-mode default per the system prompt — the merge work is the only uncommitted state at that moment, and you intend to discard it). Also remember: don't push the merge commit until manual smoke (§6.7–9) passes — once it's on `origin/feature/agent-chat`, rollback requires a force-push.

### 7.3 After merge pushed

If conflicts surface only after CI runs:

```bash
git revert -m 1 <merge-commit-sha>     # creates a "Revert merge X" commit
git push                                 # safe non-force push
```

Don't force-push the rolled-back state on a shared branch.

## 8. Effort estimate

For an experienced operator with this plan in hand:

| Phase | Estimate |
|---|---|
| Tag + invoke `git merge` + read conflict list | 5 min |
| `Cargo.toml` resolution per §4.3 | 15 min |
| `Cargo.lock` + `package-lock.json` regen | 15 min |
| `lib.rs` invoke_handler union per §4.5 | 30 min |
| `commands/auth.rs` Step-10 rewire per §4.6 | 30–60 min |
| **`auth/{mod,derivation}.rs` relocation per §4.1** | **60–90 min** ← the trickiest |
| `execution/mod.rs` superset reconciliation per §4.2 | 30–45 min |
| `github.rs` (623-line diff) | 60–90 min |
| `commands/workspace.rs` | 30 min |
| Frontend conflicts (App.tsx, sidebar-workspace.test.tsx, etc.) | 30–60 min |
| Doc reconciliation (`STATUS.md` etc.) | 15–30 min |
| First full `cargo check` + fix-up loop | 15–30 min |
| Verification per §6 | 30–45 min |
| **Total** | **5.5–9 hours** focused work |

If something goes sideways (e.g. `chacha20poly1305` doesn't resolve against the new dep tree, or `tauri = { features = ["test"] }` breaks cargo unification under `gag = "1"`) add another 1–2 hours.

## 9. Open questions for the operator

1. **Env-isolation revert reconciliation** (§4.2): Take agent-chat's superset and let main's revert effectively be a no-op? Or remove the helpers and align the agent-chat call sites with main's "default-inherit-env"? Recommendation: keep the superset.
2. **Version bump**: Take main's `0.1.33` immediately, or hold the agent-chat at 0.1.26 until the feature ships? Recommendation: take 0.1.33 — the agent-chat feature isn't ready to ship and the version is already in the tagged release line.
3. **Test pin verification**: Before pushing the merge, do you want a manual diff of agent-chat's hex pins vs Vexis's current `pinned_golden_values_codemux_api_v1` test in `~/projects/vexis/src-tauri/src/encryption/manager.rs` to be sure Vexis hasn't drifted? Recommendation: yes — five minutes of paranoia for a cross-product contract.

## 10. Decision

GO / MODIFY / DEFER — operator's call. If GO, the next session should:
- create the pre-merge tag,
- run `git merge origin/main`,
- work through §4 in the order listed (`Cargo.toml` → lockfiles → auth split → `lib.rs` → `commands/auth.rs` → frontend → docs),
- and gate on §6 before commit.
