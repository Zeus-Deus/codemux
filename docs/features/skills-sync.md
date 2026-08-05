# Skills Sync

- Purpose: Cross-device synchronization of user-authored skills, stored server-side, cross-product compatible with Vexis at the auth layer.
- Audience: Anyone touching skills, the codemux-api `/api/skills` routes, or the Settings → Sync UI.
- Authority: Canonical feature reality doc for Step 10.
- Update when: Behavior, storage model, sync target, or wire format changes.
- Read next: `docs/archive/step-10-skills-sync.md` (per-stage history), `~/.claude/skills/codemux-api-infrastructure` (server-side reference).

## What This Feature Is

User-authored skills (markdown files under `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.agents/skills/`, `~/.opencode/skills/`, `~/.config/opencode/skills/` — the full list, with per-root provider tags, is under "Provider scope rules" below) sync across every device the user signs into Codemux on. Skills are stored **server-side**: the skill name and content travel as plaintext and the server persists them in plaintext columns, protected at rest by the database — the same model Codemux's `user_settings` sync already uses. There is **no client-held encryption key**, so single-sign-on users (GitHub OAuth) sync without ever setting a sync password.

The Better Auth account is still cross-product compatible with Vexis (email+password login derives the same `AuthSecret` via the shared `codemux-api-*` HKDF protocol); only the **skills payload** is non-E2E.

## Current Model

### Storage

- **Server-side, plaintext.** The server stores the skill `name` + `content` as `TEXT` columns, server-readable. Protection is encryption-at-rest at the database/disk layer plus access control — not zero-knowledge. (Skills are markdown instructions; this matches the posture of essentially every settings/notes sync product. Users should not paste secrets into skill files expecting a vault.)
- **No client key.** Skills sync needs nothing derived from the password. A signed-in user — email/password OR GitHub OAuth — is sync-ready immediately.

### Auth derivation (unchanged, shared with Vexis)

- `derive_auth_secret(password, email)` (Argon2id `m=64MiB, t=3, p=4` → HKDF-SHA256 with the `codemux-api-auth-secret-v1` label) still produces the `AuthSecret` that Codemux sends to Better Auth in place of the raw password.
- The `encryption_key` half (`derive_login_credentials`, `codemux-api-encryption-key-v1`) is **no longer used by Codemux** — skills are server-side. The function + its cross-product hex pin against Vexis are retained in `auth/derivation.rs` as a protocol canary (Vexis still uses the key half for `voice_*`). Drift still fails CI.

### Setup flows

- **Email/password:** signed in ⇒ sync ready. No second password, no key.
- **GitHub OAuth:** signed in ⇒ sync ready. **No password prompt** — this is the whole point of the server-side model. (Previously OAuth users had to invent a sync password to enable the client-side key; that flow is gone.)
- **No repair flow.** There's no device-local key to lose, so there's nothing to repair.

### Sync engine

- **Trigger sources:**
  - Auth state transitions to `syncAvailable=true` (any signin, gated on the Agent Chat interface flag — on by default since PR #232).
  - File watcher (`crate::skills::watcher` emits `skills-changed`) → frontend debounces 1.5s on top of the watcher's own 300ms → `skillsSyncNow`.
  - Periodic 5-minute timer in `useSkillsSync`, gated on `document.visibilityState === "visible"`.
  - Manual "Sync now" via the engine's command surface.
- **Cycle (`SyncEngine::sync_now`):**
  1. One-time migration: if the mapping's `plaintext_migrated` flag is `false` (it was written by a pre-server-side build), zero every entry's `last_synced_at_millis` so the push pass re-uploads each mapped skill as plaintext via **PUT to its existing `remote_id`** — rewriting the old ciphertext row in place (no duplicate rows, no data loss). Flag set `true` afterward; runs exactly once.
  2. `GET /api/skills` → list every skill (plaintext name + content).
  3. For each remote row: **skip if its name is empty** (a legacy ciphertext-only row the server couldn't migrate yet); otherwise compare server `updated_at` to the mapping's recorded value and write to `<home>/.codemux/skills/<name>/SKILL.md` if the server is newer (or first-pull).
  4. Drop mapping entries whose `remote_id` is no longer on the server (server is authoritative for deletion).
  5. Walk every syncable directory for `<root>/<name>/SKILL.md`. For each: skip if `mtime == mapping.last_synced_at_millis` (echo-loop guard); otherwise `POST /api/skills` (new) or `PUT /api/skills/:id` (update, falls back to `POST` on 404), sending plaintext `{name, content, provider, scope}`.
  6. Persist the mapping atomically via tmp+rename.
- **Conflict resolution:** last-write-wins by `updated_at`. Concurrent cross-device edits land at "whichever device finished pushing last" — accepted risk.
- **Mapping table:** versioned JSON at `~/.codemux/sync/skills-mapping.json`, keyed on `remote_id`. Carries the `plaintext_migrated` flag (serde-default `false` for old files, `true` for fresh installs). Atomic writes; corrupt files renamed aside and treated as empty.

### Provider scope rules

- **Synced (`scope=user`):** `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.agents/skills/` (Codex's newer root), `~/.opencode/skills/`, `~/.config/opencode/skills/`. The origin provider tag is preserved through the wire format and restored on the receiving device's mapping.
- **Lockstep invariant:** this list is `path_detection::USER_SCOPE_PROVIDERS`, and it must cover every user root that `skills::paths::enumerate_scan_paths` returns. The push pipeline walks the scanner's enumeration and classifies each hit through `detect_skill_path`; a root the scanner knows but the table doesn't classifies as `None` and is dropped from sync with no diagnostic. `user_scope_table_covers_every_scan_root` / `project_scope_table_covers_every_scan_root` fail the build if the two drift.
- **NOT synced (`is_syncable=false`):** project-scope skills. `PROJECT_SCOPE_SUBDIRS` recognizes five roots — `<project>/.codemux/skills/`, `.claude/skills/`, `.codex/skills/`, `.agents/skills/`, `.opencode/skills/` — and `provider_from_project_subdir` maps **both** `.codex/skills` and `.agents/skills` to provider `codex`. They are classified (so they show correct provider grouping and are covered by the lockstep guard) but never pushed. Reserved for Step 10.5.
- **Alias dedup upstream of sync:** the scanner dedupes roots by canonical path (`skills::paths::canonical_key`), so when two roots resolve to the same directory — a project root aliasing `$HOME`, or a symlinked plugin dir — only the first-enumerated one survives and sync sees exactly that one. User scope claims before project scope, and the canonical location claims before its alias (`~/.codex/skills` wins over `~/.agents/skills`). See `docs/features/agent-chat.md` § "Cross-provider skill system" for the full enumeration order.
- **NOT synced (`None`):** plugin skills, marketplace skills, anything outside the recognized layouts.
- **Sync target invariant:** the receiving device always writes to `~/.codemux/skills/<name>/SKILL.md` regardless of origin provider.

### Settings UI surface

- **Settings → Account → Sync section.** Two states based on `syncAvailable`:
  - `syncAvailable=true` → `SyncStatusDisplay` (icon, state label, "Last synced N minutes ago", Sync now / Retry) + Export / Import buttons.
  - `syncAvailable=false` → a one-line "Sign in to sync your skills" hint (only reachable while the session is still settling or signed out).
- No password forms, no reset-password dialog — both removed with the E2E model.
- Local export to plaintext JSON + import-from-JSON remain for backup/restore.

### Server-side surface

Lives in `~/codemux-api/api/src/index.ts` on the production VPS. Five `/api/skills` routes:

- `GET /api/skills` — list the user's skills (plaintext name + content), newest-updated first.
- `POST /api/skills` — create one.
- `PUT /api/skills/:id` — replace one in place.
- `DELETE /api/skills/:id` — delete one.
- `DELETE /api/skills` — wipe all.

Wire format: `{remoteId, name, content, provider, scope, updatedAt}`. The `user_skills` table carries plaintext `name`/`content` columns alongside the now-nullable legacy ciphertext columns (`encrypted_name`/`nonce_name`/`encrypted_content`/`nonce_content`). The move to server-side was an **additive** migration: `ADD COLUMN IF NOT EXISTS name/content` + `ALTER COLUMN ... DROP NOT NULL` on the legacy columns. Existing ciphertext rows are left in place; the client rewrites them to plaintext in place (PUT) on first sync after upgrade. Rows that can't be migrated come back with an empty name and are skipped.

## What Works Today

- Server-side skills sync across devices; OAuth users sync with **no password prompt**.
- Cross-product Better Auth login still compatible with Vexis (auth_secret + encryption_key hex pins green in CI).
- Per-skill conflict resolution by `updated_at` (last-write-wins).
- File-watcher-triggered push (1.5s debounce), 5-minute periodic sync when visible.
- One-time in-place migration of pre-server-side ciphertext rows (no duplicates, no data loss; relies on local skill files as the source of truth).
- Local export to plaintext JSON + import-from-JSON.
- 316 server-side tests + the Rust lib suite + Vitest cover the surface; `examples/stage5_smoke.rs` (full engine cycle) + `examples/skills_smoke.rs` (single-record round-trip) are runnable against the live API after deploy.

## Current Constraints

- **No project-scoped sync.** Reserved for Step 10.5 — the wire format already carries `scope` plaintext so the migration is additive.
- **Auto-delete on local-file-removal not implemented.** Deleting a local `SKILL.md` leaves the server row until the user wipes it (`DELETE /api/skills`).
- **No conflict UI.** Concurrent edits silently last-write-wins.
- **Not zero-knowledge.** Skills are server-readable. The privacy promise is "encrypted at rest," not "we can't read your skills." Docs/UI say so; users shouldn't store secrets in skills.
- **Legacy ciphertext rows on a device with no local copy** can't be migrated (the server can't read them and there's no local file to re-push). They linger harmlessly until wiped. This only affects pre-server-side data; the actual skill content lives in local files.

## Important Touch Points

### Rust (client)

- `src-tauri/src/auth/derivation.rs` — `derive_auth_secret` (login), plus `derive_login_credentials`/`EncryptionKey` retained as the Vexis cross-product canary.
- `src-tauri/src/skills_sync/mod.rs` — `SyncEngine`, push/pull/conflict resolution, the one-time plaintext migration.
- `src-tauri/src/skills_sync/api_client.rs` — `/api/skills` HTTP wrappers + the plaintext `SkillWire`/`SkillUpload`.
- `src-tauri/src/skills_sync/path_detection.rs` — provider/scope classification + canonical destination path.
- `src-tauri/src/skills_sync/mapping.rs` — JSON mapping table (carries `plaintext_migrated`).
- `src-tauri/src/skills_sync/export.rs` — local export/import.
- `src-tauri/src/commands/auth.rs` — `get_sync_status` (= signed in), signin/check_auth/sign_out emit `syncAvailable`.
- `src-tauri/src/commands/skills_sync.rs` — `skills_sync_now`, `skills_sync_status`, `export_skills_to_file`, `import_skills_from_file`.

### Frontend

- `src/stores/auth-store.ts` — `syncAvailable`, `authMethod`, `setSyncStatus`, `refreshSyncStatus`.
- `src/components/settings/sync-section.tsx` — two-state SyncSection (ready dashboard / sign-in hint) + Export/Import.
- `src/components/settings/sync-status-display.tsx` — live status dashboard.
- `src/hooks/use-skills-sync.ts` — auto-trigger orchestration.
- `src/tauri/commands.ts` — Tauri command bindings + `SyncStatus`.

### Server (production VPS)

- `~/codemux-api/api/src/index.ts` — `user_skills` schema (plaintext + nullable-legacy columns) + 5 `/api/skills` routes.
- `~/codemux-api/api/src/tests/skills-sync.test.ts` — server-side tests for the skills routes (plaintext shape + legacy-row coverage).
- `~/codemux-api/api/src/tests/preload.ts` — mirrors the production DDL for the test DB.

## Notes

- **Removed with the E2E model:** the OAuth `setup_sync_password` / `provide_password_for_sync` commands, the `EncryptionManager` + `~/.local/share/codemux/sync-key.enc` persistence, the `encryption/mod.rs` AEAD module, the reset-sync-password dialog + `wipe_remote_skills_for_reset`, and the custom `set-password` route's role in skills setup (the route itself may remain for other uses).
- **The `codemux-api-*` HKDF labels remain load-bearing for auth** across Codemux + Vexis. Any rotation must bump the `vN` suffix in both clients + the server reset-password page simultaneously.
- **Deploying the server change:** the schema migration is additive and idempotent (runs on container startup). Deploy with `cd ~/codemux-api && docker compose up -d --build`.
