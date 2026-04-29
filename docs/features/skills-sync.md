# Skills Sync

- Purpose: Cross-device synchronization of user-authored skills, end-to-end encrypted, cross-product compatible with Vexis.
- Audience: Anyone touching skills, the codemux-api `/api/skills` routes, the encryption module, or the Settings → Sync UI.
- Authority: Canonical feature reality doc for Step 10.
- Update when: Behavior, threat model, sync target, or wire format changes.
- Read next: `docs/plans/step-10-skills-sync.md` (per-stage history), `docs/plans/step-10-skills-sync-research.md` (design rationale), `~/.claude/skills/codemux-api-infrastructure` (server-side reference).

## What This Feature Is

User-authored skills (markdown files under `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.opencode/skills/`) sync across every device the user signs into Codemux on. The sync is end-to-end encrypted: ciphertext on the server, plaintext only on the user's own machines. Cross-product compatible with Vexis — the same Better Auth account works across both apps because both products derive credentials via the shared `codemux-api-*` HKDF protocol.

## Current Model

### Encryption

- **AEAD:** XChaCha20-Poly1305 (24-byte nonce, 16-byte auth tag, fresh OS-RNG nonce per encryption).
- **KDF:** Argon2id with `m=64MiB, t=3, p=4`, 32-byte output. Salt is `SHA256("codemux-api-master-v1\0" || normalize_email(email))`.
- **Split derivation (HKDF-SHA256):** the 32-byte master is fanned out to two domain-separated 32-byte secrets:
  - `auth_secret = HKDF-Expand(master, "codemux-api-auth-secret-v1", 32)` — base64-no-pad-encoded, sent to Better Auth in place of the password.
  - `encryption_key = HKDF-Expand(master, "codemux-api-encryption-key-v1", 32)` — never sent anywhere; encrypts skill names and contents before upload.
- **Cross-product pin:** pinned hex values for `(password="golden-test-password", email="golden-test@example.com")` lock byte-identity with Vexis (`auth_secret_matches_vexis_for_known_input`, `encryption_key_matches_vexis_for_known_input`). Drift fails CI.

### Key persistence

- **In-memory:** `EncryptionManager` holds the 32-byte key in `Mutex<Option<[u8; 32]>>`. `with_key(closure)` is the only escape route to the bytes — they don't cross the Tauri IPC boundary. `Drop` zeroizes via `write_volatile`.
- **On-disk:** `~/.local/share/codemux/sync-key.enc`. AES-256-GCM-wrapped with a key derived from `/etc/machine-id` + a random 16-byte salt + a random 12-byte nonce (file format: `salt(16) || nonce(12) || ciphertext+tag`). Cannot be copied between machines (different `machine-id` produces a different wrap key).
- **Lifecycle:** loaded on cold-start `check_auth` if the file exists; saved during `signin_email` (email/password users get sync automatically), `setup_sync_password` (OAuth users opt in once), and `provide_password_for_sync` (repair flow). Cleared + file deleted on `sign_out`, on `wipe_remote_skills_for_reset`, and on token-expiry detection during `check_auth`.

### Setup flows

- **Email/password:** the password used at signin IS the sync password. `signin_email` derives both halves; the AuthSecret half satisfies Better Auth's bcrypt check, the encryption_key half is saved locally. Sync is "ready" the moment signin succeeds. No second password.
- **GitHub OAuth:** OAuth users have no Better Auth credential account yet. They open Settings → Sync, fill in the inline `SetupSyncPasswordForm` (≥8 chars, must contain a letter + a digit, mandatory acknowledgment checkbox). The Tauri command `setup_sync_password` derives, calls `POST /api/auth/set-password` (custom server route — see "Server-side" below), then saves the encryption_key locally.
- **Repair (rare):** if the local `sync-key.enc` file is lost (manual delete, machine-id rotated, fresh install), `ProvidePasswordForm` lets the user re-enter their password. No server call — the bcrypt is unchanged; only device-local state is being rebuilt. Wrong password is detected lazily at the first sync attempt (matches Vexis's "lazy verification" pattern).

### Sync engine

- **Trigger sources:**
  - Auth state transitions to `syncAvailable=true` (signin or `setup_sync_password` completion).
  - File watcher (`crate::skills::watcher` emits `skills-changed`) → frontend debounces 1.5s on top of the watcher's own 300ms → `skillsSyncNow`.
  - Periodic 5-minute timer in `useSkillsSync`, gated on `document.visibilityState === "visible"`.
  - Manual "Sync now" button in Settings → Sync.
- **Cycle (`SyncEngine::sync_now`):**
  1. `GET /api/skills` → list every encrypted blob.
  2. For each remote blob: decrypt name + content; compare server `updated_at` to mapping's recorded `server_updated_at_millis`; write to `<home>/.codemux/skills/<name>/SKILL.md` if server is newer (or first-pull).
  3. Drop mapping entries whose `remote_id` is no longer on the server (server is authoritative for deletion).
  4. Walk every syncable directory (`enumerate_scan_paths().user_paths`) for `<root>/<name>/SKILL.md` files. For each: skip if `mtime == mapping.last_synced_at_millis` (echo-loop guard); otherwise re-encrypt with `with_key` and `POST /api/skills` (new) or `PUT /api/skills/:id` (update, falls back to `POST` on 404).
  5. Persist the mapping atomically via tmp+rename.
- **Conflict resolution:** last-write-wins by `updated_at`. Server wins on pull when `server_updated_at > mapping.server_updated_at_millis`; local wins on push when the file's `mtime != mapping.last_synced_at_millis`. Concurrent edits across devices land at "whichever device finished pushing last" — accepted risk per Stage 1 research §1.7.
- **Concurrency:** `SyncEngine` serializes `sync_now` callers via a top-level `Mutex`. The Tauri command wrapper emits `skills-sync-state-changed` events before + after each cycle so `useSkillsSyncStatus` can drive the UI without polling.
- **Mapping table:** versioned JSON at `~/.codemux/sync/skills-mapping.json`. Keyed on `remote_id` (server BIGSERIAL as string). Atomic writes; corrupt files renamed aside as `.corrupted-<unix-millis>` and treated as empty.

### Provider scope rules

- **Synced (`scope=user`):** `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.opencode/skills/`. Each provider's tag is preserved verbatim through the wire format and restored on the receiving device's mapping (so a Claude-origin skill remains tagged `claude` even after it lives at the canonical codemux path).
- **NOT synced (recognized but `is_syncable=false`):** project-scope skills under `<project>/.codemux/skills/`, `<project>/.claude/skills/`, etc. Reserved for Step 10.5 (project-scoped sync).
- **NOT synced (returns `None`):** plugin skills, marketplace skills, anything outside the recognized layouts.
- **Sync target invariant:** the receiving device always writes to `~/.codemux/skills/<name>/SKILL.md` regardless of the origin provider. The origin tag survives in the mapping but the file location does not. A user editing a synced skill on the receiving device will eventually re-tag it as `provider=codemux` if the mapping is ever rebuilt — accepted v1 trade.

### Settings UI surface

- **Settings → Account → Sync section** is the single discovery surface. Inline form, not a modal — opt-in.
- **Three render states based on `(syncAvailable, authMethod)`:**
  - `syncAvailable=true` → `SyncStatusDisplay` (icon, state label, "Last synced N minutes ago", Sync now / Retry button) + Export / Import / Forgot-password buttons.
  - `syncAvailable=false, authMethod=github` → `SetupSyncPasswordForm` (the OAuth onboarding form).
  - `syncAvailable=false, authMethod≠github` → `ProvidePasswordForm` (the repair form for email users whose local key file was lost).
- **Status display:** auto-refreshes the relative-time string every 30s via `useTickEvery`. Optimistic disable on Sync-now click; clears when the post-cycle event arrives.
- **Reset password dialog:** five-step modal — warn → export-running → exported → skip-confirm → wiping → done → error. The destructive `wipe_remote_skills_for_reset` is gated behind either a successful export OR an explicit acknowledgment checkbox in the skip path.

### Server-side surface

Lives in `~/codemux-api/api/src/index.ts` on the production VPS. Five `/api/skills` routes mirror `/api/voice/*`:

- `GET /api/skills` — list user's encrypted skills, newest-updated first.
- `POST /api/skills` — create one.
- `PUT /api/skills/:id` — replace one.
- `DELETE /api/skills/:id` — delete one.
- `DELETE /api/skills` — wipe all (used by reset-sync flow).

Plus a custom `POST /api/auth/set-password` route, added because Better Auth 1.5.6 (and confirmed 1.6.9) ships `setPassword` without a registered HTTP path — an upstream bug. The custom route hits `auth.$context.internalAdapter.linkAccount` directly with `bcrypt(newPassword)`, which is what an upstream-fixed setPassword would do.

Wire format for skills: `{remoteId, encryptedName, nonceName, encryptedContent, nonceContent, provider, scope, updatedAt}`. Ciphertext is base64 over the wire (matches Vexis `voice_*` shape). Schema at `api/src/index.ts` (table `user_skills` with `BIGSERIAL` ids, `TEXT` ciphertext columns, FK CASCADE to `"user"(id)`).

## What Works Today

- End-to-end encrypted skills sync across devices, cross-product compatible with Vexis (cross-product hex pins green in CI).
- Email/password users get sync automatically at signin; OAuth users opt in via Settings → Sync once per account.
- Per-skill conflict resolution by `updated_at` (last-write-wins).
- File-watcher-triggered push (1.5s frontend debounce on top of the watcher's 300ms), plus 5-minute periodic sync when the app window is visible.
- Local export to plaintext JSON + import-from-JSON for backup/restore. Atomic tmp+rename on every write.
- Multi-step reset dialog enforcing export-or-explicit-skip before the destructive wipe.
- Live SyncStatusDisplay in Settings → Account with auto-refreshing relative-time.
- 217 server-side tests + 1066 Rust lib tests + 1448 Vitest tests cover the surface; production smoke (`examples/stage5_smoke.rs`) verifies the engine end-to-end against `api.codemux.org`.

## Current Constraints

- **No project-scoped sync.** Reserved for Step 10.5 — the wire format already carries `scope` plaintext so the migration is additive.
- **Pending push count not surfaced in the UI.** The Stage 3 engine syncs scan-on-trigger without a queue; "N changes pending" would require a Stage 6+ engine redesign.
- **Auto-delete on local-file-removal not implemented.** A user who deletes `~/.codemux/skills/foo/SKILL.md` locally still has the encrypted blob on the server until they explicitly wipe it. Manual via "Reset sync password" dialog (which wipes everything) or a future per-skill "Remove from sync" action.
- **No conflict UI for human review.** Concurrent edits silently last-write-wins. Stage 6+ polish if the silent loss becomes a problem.
- **Inlined into `auth.rs` on `feature/agent-chat`.** When this branch eventually merges main (which carries an `auth/{api,derivation,mod}.rs` split), the derivation block needs to relocate to `auth/derivation.rs`. Marked with a `TODO` in code.
- **Lost password = lost data.** No recovery code, no escrow. Stage 4's local-export-before-reset is the only mitigation; this is the conscious trade documented in research §1.4 and §4.1.

## Important Touch Points

### Rust (client)

- `src-tauri/src/auth.rs` — `derive_login_credentials`, `EncryptionKey`, `save_encryption_key`/`load_encryption_key`/`delete_encryption_key`, machine-bound AES-GCM wrap.
- `src-tauri/src/encryption/mod.rs` — XChaCha20-Poly1305 wrapper, `EncryptionManager` singleton.
- `src-tauri/src/skills_sync/mod.rs` — `SyncEngine`, push/pull/conflict resolution, Tauri-managed singleton.
- `src-tauri/src/skills_sync/api_client.rs` — `/api/skills` HTTP wrappers.
- `src-tauri/src/skills_sync/path_detection.rs` — provider/scope classification + canonical destination path.
- `src-tauri/src/skills_sync/mapping.rs` — JSON mapping table with atomic write.
- `src-tauri/src/skills_sync/export.rs` — local export/import for the catastrophic-loss mitigation.
- `src-tauri/src/commands/auth.rs` — `setup_sync_password`, `provide_password_for_sync`, custom `set_password_api` HTTP helper.
- `src-tauri/src/commands/skills_sync.rs` — `skills_sync_now`, `skills_sync_status`, `export_skills_to_file`, `import_skills_from_file`, `wipe_remote_skills_for_reset`.
- `src-tauri/examples/skills_smoke.rs` — minimal one-record E2E smoke (Stage 1).
- `src-tauri/examples/stage5_smoke.rs` — full engine cycle smoke (Stages 1-4).

### Frontend

- `src/stores/auth-store.ts` — `syncAvailable`, `authMethod`, `setSyncStatus`, `refreshSyncStatus`.
- `src/components/settings/sync-section.tsx` — three-state SyncSection (`SyncStatusDisplay` / `SetupSyncPasswordForm` / `ProvidePasswordForm`) + Stage 4 Export/Import/Forgot-password controls.
- `src/components/settings/sync-status-display.tsx` — live status dashboard.
- `src/components/settings/reset-sync-password-dialog.tsx` — five-step reset flow.
- `src/hooks/use-skills-sync.ts` — auto-trigger orchestration (sync-state-changed event, file-watcher event, 5-minute periodic).
- `src/hooks/use-skills-sync-status.ts` — live status hook driven by `skills-sync-state-changed` events.
- `src/lib/relative-time.ts` — bucketed time formatter.
- `src/tauri/commands.ts` — Tauri command bindings.

### Server (production VPS)

- `~/codemux-api/api/src/index.ts` — `user_skills` schema + 5 `/api/skills` routes + custom `POST /api/auth/set-password` workaround.
- `~/codemux-api/api/src/tests/skills-sync.test.ts` — 34 server-side tests for the skills routes.
- `~/codemux-api/api/src/tests/set-password.test.ts` — 15 tests for the custom set-password route.

## Notes

- **Forward compat for Step 10.5 (project-scoped sync):** the schema's `scope` plaintext field is the hook. 10.5 adds `project_remote_url_hash TEXT NULL` as an additive migration — HMAC-of-normalized-git-remote so the server can route per-project pulls without seeing which repo. Push pipeline filters projects by hash; user-scope unchanged.
- **The `codemux-api-*` HKDF labels are now load-bearing across Codemux + Vexis.** Any rotation must bump the `vN` suffix in BOTH client implementations + the server's reset-password page derivation simultaneously. See `~/.claude/skills/codemux-api-infrastructure` "Shared auth protocol naming" for the canonical contract.
- **The custom `POST /api/auth/set-password` route is a workaround.** Track the upstream Better Auth bug; remove the custom route when an upstream version with a registered path lands. Client field shape (`{newPassword: string}`) already matches the documented Better Auth schema — no client change needed when upstream is fixed.
