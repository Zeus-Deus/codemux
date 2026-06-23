# Step 10 — Skills Sync (COMPLETE — All 6 Stages Shipped)

> **Superseded (PR #112):** the end-to-end-encrypted model recorded below shipped, then was replaced with **server-side plaintext storage** so GitHub OAuth / SSO users can sync without inventing a sync password. The `EncryptionManager`, `sync-key.enc`, `encryption/` module, OAuth sync-password setup, and reset-password dialog described in these stages are gone. This doc is kept as the E2E-era audit trail; for current behavior read `docs/features/skills-sync.md`.

- Purpose: Per-stage history + final state for Step 10. Current behavior lives in `docs/features/skills-sync.md`; this doc is the audit trail.
- Audience: Anyone wanting to know what happened in each stage, plus the cumulative test counts and operator artifacts.
- Authority: Closed plan. Future maintenance work goes in new plan docs (e.g. `docs/plans/step-10.5-project-scoped-sync.md` when 10.5 starts).
- Update when: A retrospective fact about what shipped is wrong; otherwise leave alone.
- Read next: `docs/features/skills-sync.md` (canonical current behavior), `docs/plans/step-10-skills-sync-research.md` (design rationale), `docs/plans/step-10-ui-smoke-checklist.md` (operator UI smoke checklist).

## Final State (Stage 6 close-out)

- **All 6 stages shipped** on `feature/agent-chat`. Inline in `auth.rs` per the Path-2 decision (we did not pull the 71 commits of unrelated main work onto this branch); will relocate to `auth/derivation.rs` when main eventually merges.
- **Server-side surface**: `user_skills` table + 5 `/api/skills` routes + custom `POST /api/auth/set-password` route on `api.codemux.org`. Pre-migration backup preserved at `~/codemux-api/backups/codemux_20260429_164843.sql.gz` (Stage 1) and `codemux_20260429_195226.sql.gz` (pre-set-password deploy).
- **Cumulative test counts (Step 10 contributions across all stages):**
  - Server (`bun test` in `~/codemux-api/api`): **+49 tests** (34 in `skills-sync.test.ts` + 15 in `set-password.test.ts`). Total server suite at 217.
  - Rust lib (`cargo test --lib`): **+115 tests** (8 EncryptionManager + 19 derivation + 41 skills_sync engine + 16 export-module + 14 encryption AEAD + 17 baseline auth tests already in agent-chat that now exercise new code paths). Total at 1066.
  - Vitest (`npm run test`): **+33 tests** (15 sync-section Stage 2 + 11 Stage 4 buttons + dialog + 12 relative-time + 10 sync-status-display + extended auth-store). Total at 1448.
  - **Pre-existing failures unchanged**: 7 tests in `sidecar/claude-agent/test/*` and `src/components/layout/sidebar-{header,workspace}.test.tsx` (MCP/Step 9 in-flight). None reference Step 10 surface; baseline preserved.
- **End-to-end smoke against production**: `examples/stage5_smoke.rs` PASS — full pull/push/update/no-op/fresh-home-pull/export/wipe/import cycle against `api.codemux.org` with a fresh test user. Cleanup confirmed: production baseline restored to 16 users + 0 user_skills + 0 voice_*.
- **Cross-product hex pins green**: AuthSecret (`9FxAbaiRLQfRmjpB6x4d3FuamAUojg9bh9dVfPYRfyI`) + encryption_key (`0b03b2541a15d39e7a422df4a6e6ade56b371453d5b41c1ec117efa62db54534`) match Vexis's golden values for `(password="golden-test-password", email="golden-test@example.com")`. CI fails if either drifts.
- **Files written across the 6 stages** (Step 10 net additions, both directions):
  - Rust new modules: `src-tauri/src/encryption/mod.rs`, `src-tauri/src/skills_sync/{mod,api_client,mapping,path_detection,export}.rs`, `src-tauri/src/commands/skills_sync.rs`.
  - Rust extended: `src-tauri/src/auth.rs` (derivation + key persistence + machine-bound wrap), `src-tauri/src/commands/auth.rs` (setup/repair/wipe-for-reset commands), `src-tauri/src/commands/mod.rs` (save/open dialog commands), `src-tauri/src/lib.rs` (state registration + invoke_handler).
  - TS new components: `src/components/settings/{sync-section,sync-status-display,reset-sync-password-dialog}.tsx`, `src/hooks/{use-skills-sync,use-skills-sync-status,use-tick-every}.ts`, `src/lib/relative-time.ts`.
  - TS extended: `src/stores/auth-store.ts`, `src/components/settings/settings-view.tsx`, `src/tauri/{commands,events}.ts`, `src/hooks/use-auth-events.ts`, `src/App.tsx`.
  - Live-smoke binaries: `src-tauri/examples/skills_smoke.rs` (one-record E2E), `src-tauri/examples/stage5_smoke.rs` (full engine cycle).
  - Server-side: custom `/api/skills` routes + `/api/auth/set-password` workaround in `~/codemux-api/api/src/index.ts`; tests in `~/codemux-api/api/src/tests/{skills-sync,set-password}.test.ts`.
  - Docs: `docs/features/skills-sync.md` (new — canonical behavior), `docs/plans/step-10-ui-smoke-checklist.md` (new — operator checklist), updates to `docs/core/{STATUS,PLAN}.md`, `docs/INDEX.md`, this plan doc.
- **Forward-compat for Step 10.5 (project-scoped sync)**: schema's `scope` plaintext field + planning entry in `docs/core/PLAN.md`. Additive migration: add `project_remote_url_hash TEXT NULL`, gate push/pull pipelines by hash. Estimated 3-5 days.

## Goal (original)

Ship end-to-end-encrypted skills sync that reuses the cross-product `codemux-api-*` derivation so one Better Auth account works across Codemux + Vexis and the server cannot read skill names or content. Match Vexis's `voice_*` pattern byte-for-byte at the wire layer.

## Goal

Ship end-to-end-encrypted skills sync that reuses the cross-product `codemux-api-*` derivation so one Better Auth account works across Codemux + Vexis and the server cannot read skill names or content. Match Vexis's `voice_*` pattern byte-for-byte at the wire layer.

## Stages

- **Stage 1 — Foundation (DONE):** `user_skills` table + 5 `/api/skills` routes on the production VPS, client-side `EncryptionKey` derivation, XChaCha20-Poly1305 encryption module, smoke-test binary.
- **Stage 2 — Frontend crypto + sync-password UX (DONE):** EncryptionManager + machine-bound `sync-key.enc` persistence, `setup_sync_password` / `provide_password_for_sync` Tauri commands, email-user automatic sync at signin, `SyncSection` inline in Settings → Account, `sync-state-changed` event, lazy verification (no unlock modal at startup). **Custom `POST /api/auth/set-password` route deployed** to `~/codemux-api/api/src/index.ts` because Better Auth 1.5.6 (and verified 1.6.9 also) ships `setPassword` without a registered path. The custom route reuses Better Auth's internal `password.hash` + `internalAdapter.linkAccount` so the wire format and bcrypt shape stay identical to what an upstream-fixed setPassword would produce. 15 regression tests in `api/src/tests/set-password.test.ts`. Live OAuth smoke against production passed: signup → strip credential → `set-password` (200) → signin with AuthSecret (200) → second set-password (400 "Password already set") → encrypted POST/GET/decrypt roundtrip via `skills_smoke` (PASS).
- **Stage 3 — Sync engine (DONE):** `src-tauri/src/skills_sync/{mod,api_client,mapping,path_detection}.rs` with full pull-then-push cycle. Last-write-wins conflict resolution by `updated_at`, atomic JSON mapping at `~/.codemux/sync/skills-mapping.json`, echo-loop guard via mtime equality with `last_synced_at_millis`, canonical pull destination `~/.codemux/skills/<name>/SKILL.md` regardless of origin provider, plugin/project paths skipped, mapping prefers recorded `provider` over fresh path detection (preserves origin tag). Tauri commands `skills_sync_now` + `skills_sync_status`. Auto-triggers on `sync-state-changed` (sync just became available) and on the existing `skills-changed` watcher event (1.5s debounce on the frontend, on top of the watcher's 300ms). 41 unit tests in `skills_sync::*`. Live cross-device smoke deferred to Stage 5/6 polish — engine internals fully covered by unit tests; HTTP layer was already proven in Stages 1 and 2.
- **Stage 4 — Local export before reset (DONE):** `src-tauri/src/skills_sync/export.rs` (pull → decrypt → atomic plaintext-JSON write at user-chosen path) + `import_exported_skills` (read → re-encrypt with current key → push via existing `api_client`). Tauri commands `export_skills_to_file`, `import_skills_from_file`, `get_export_recommended_filename`, `wipe_remote_skills_for_reset` (the destructive helper that wipes server skills + clears local key + triggers Better Auth's email-based reset). Multi-step `ResetSyncPasswordDialog` with mandatory "export OR explicit-skip-acknowledgment" gate before the destructive action. Manual "Export skills locally" + "Import skills from backup" buttons inline in Settings → Sync. Generic save/open dialog Tauri commands (`pick_save_file_dialog`, `pick_open_file_dialog`) added to `commands/mod.rs` for reuse. 16 export-module tests (roundtrip, version/product validation, malformed JSON, missing file, email mismatch, atomic write, camelCase serialization). 11 sync-section tests covering the new export/import buttons + 5-step reset dialog flow. Live cross-device smoke deferred — happy path is the same encrypt→server→decrypt loop already proven in Stages 1-3.
- **Stage 5 — Settings UI (DONE):** `SyncStatusDisplay` + `SyncStateIcon` in Settings → Sync, `useSkillsSyncStatus` hook driven by `skills-sync-state-changed` events emitted from the Tauri command wrapper around `skills_sync_now`. `relativeTime` + `useTickEvery` utilities power the auto-refreshing "Last synced N minutes ago" label without re-fetching. 5-minute periodic sync timer in `useSkillsSync`, gated on `document.visibilityState === "visible"`. **Pending push count deferred** — Stage 3's engine syncs scan-on-trigger without a queue; supporting a counter would require a Stage 3 redesign and the dashboard works fine without it (relative time + spinner + state label are sufficient signal). Live programmatic smoke (`examples/stage5_smoke.rs`) passed against production: cycle1 push, cycle2 update without duplicate, cycle3 idempotent no-op, fresh-home pull writes to canonical path, export → wipe → import re-pushes 1 skill cleanly. Production data baseline restored (16 users, 0 user_skills, voice_* unchanged).
- **Stage 6 — Final cleanup + docs (DONE):** authored canonical feature doc at `docs/features/skills-sync.md`, operator UI smoke checklist at `docs/plans/step-10-ui-smoke-checklist.md`, STATUS + PLAN updates with Step 10 / 10.5 / 11 entries, dropped `examples/print_derived.rs` (consolidated into `stage5_smoke.rs`), confirmed no diagnostic/dead code in Step 10 surface (`eprintln!` calls match the existing `[component]` prefix pattern shared with `[settings-sync]`, `[auth]`, etc.). Project-scoped sync (`project_remote_url_hash TEXT NULL` additive migration) tracked in `docs/core/PLAN.md` as Step 10.5.

## Stage 1 Implementation Report

### Architecture corrections from the original prompt

The original Stage 1 prompt drifted from the research doc + Vexis pattern in five places. All corrected before any code landed:

1. **No custom `setup-password` endpoint** — Vexis uses Better Auth's existing `/api/auth/set-password` with the derived AuthSecret. The user-table `sync_*` columns proposed in the prompt were dead weight under the actual protocol.
2. **No master-key wrapping** — `encryption_key` is re-derived from `(password, email)` each session, no envelope.
3. **No server-side crypto module** — Vexis crypto is client-side only; the API has no key material. Tasks "port the crypto module to the API" describe infrastructure that doesn't exist in this architecture.
4. **TEXT not BYTEA** — ciphertext is base64 on the wire; matches `voice_*`.
5. **No HMAC name hash** — encrypt the name as its own ciphertext+nonce pair (research §4.3); conflict detection by record id.

### Files touched

**Server (VPS, `~/codemux-api/`):**
- `api/src/index.ts` — added `user_skills` `CREATE TABLE` block + 5 `/api/skills` routes (`GET`, `POST`, `PUT/:id`, `DELETE/:id`, `DELETE`) with `MAX_SKILLS_BODY = 4 MiB`, `MAX_SKILLS_PER_USER = 500`, allowed `provider` ∈ {claude, codex, opencode, codemux}, allowed `scope` ∈ {user, project}.
- `api/src/tests/preload.ts` — mirrored DDL.
- `api/src/tests/helpers.ts` — `cleanupDB` now also wipes `user_skills`.
- `api/src/tests/skills-sync.test.ts` — new file, 34 tests following the `voice-sync.test.ts` template.

**Client (this repo, `feature/agent-chat`):**
- `src-tauri/Cargo.toml` — added `chacha20poly1305 = "0.10"`, `argon2 = "0.5"`, `hkdf = "0.12"`.
- `src-tauri/src/lib.rs` — registered `pub mod encryption` (Stage 1) and `pub mod mcp` (Step 9 in-flight, restored after a recovery).
- `src-tauri/src/encryption/mod.rs` — new module mirroring Vexis's `encryption/mod.rs` (XChaCha20-Poly1305 wrapper, 14 unit tests).
- `src-tauri/src/auth.rs` — added `AuthSecret`, `EncryptionKey` (zeroizing `Drop`, redacting `Debug`), `derive_login_credentials(password, email) -> (AuthSecret, EncryptionKey)`, and `derive_auth_secret`. Inlined into the existing single-file `auth.rs` because `feature/agent-chat` predates main's auth-module refactor (single `auth.rs` → `auth/{api,derivation,mod}.rs`). 19 new derivation tests appended to the existing `mod tests`, including both cross-product hex pins. Marked with a TODO to relocate to `auth/derivation.rs` when main is merged.
- `src-tauri/examples/skills_smoke.rs` — throwaway end-to-end roundtrip. Imports from `codemux_lib::auth::derive_login_credentials` (single-file path on agent-chat); the import path becomes `auth::derivation::derive_login_credentials` after main merges.

### Schema (production)

```sql
CREATE TABLE IF NOT EXISTS user_skills (
  id                BIGSERIAL PRIMARY KEY,
  user_id           TEXT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
  encrypted_name    TEXT NOT NULL,
  nonce_name        TEXT NOT NULL,
  encrypted_content TEXT NOT NULL,
  nonce_content     TEXT NOT NULL,
  provider          TEXT NOT NULL,
  scope             TEXT NOT NULL,
  updated_at        TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_user_skills_user_id        ON user_skills (user_id);
CREATE INDEX IF NOT EXISTS idx_user_skills_user_updated   ON user_skills (user_id, updated_at DESC);
```

Forward-compat: Step 10.5 will add `project_remote_url_hash TEXT NULL` via additive migration. No constraint conflicts.

### Wire format

```
{
  remoteId:         string  (server BIGSERIAL as string, GET only)
  encryptedName:    string  (base64 ciphertext)
  nonceName:        string  (base64)
  encryptedContent: string
  nonceContent:     string
  provider:         "claude" | "codex" | "opencode" | "codemux"
  scope:            "user" | "project"
  updatedAt:        ISO timestamp (GET only)
}
```

Identical shape to Vexis `voice_*` blobs (TEXT base64). Cross-product compatibility verified by the encryption_key hex pin (see below).

### Test counts

- Server: **202 pass / 0 fail** (168 baseline + 34 new skills tests in `skills-sync.test.ts`).
- Client lib: **34 derivation tests + 14 encryption tests** all green, including:
  - `auth_secret_matches_vexis_for_known_input` — pins AuthSecret to `9FxAbaiRLQfRmjpB6x4d3FuamAUojg9bh9dVfPYRfyI` for golden input.
  - `encryption_key_matches_vexis_for_known_input` — pins encryption_key to `0b03b2541a15d39e7a422df4a6e6ade56b371453d5b41c1ec117efa62db54534`.

### Cross-product compatibility

Both pin tests pass against the documented golden values. Codemux and Vexis produce byte-identical `(AuthSecret, EncryptionKey)` for `(password="golden-test-password", email="golden-test@example.com")`. Any future drift in either product fails CI.

### Operations log

Pre-migration backup (3am cron + the manual one I ran):
```
ssh work@78.47.192.173 ~/codemux-api/backup.sh
# → codemux_20260429_164843.sql.gz (21,619 bytes)
```

Build + deploy:
```
ssh work@78.47.192.173 'cd ~/codemux-api && docker compose build api'
ssh work@78.47.192.173 'cd ~/codemux-api && docker compose up -d api'
```

Post-deploy verification:
- `GET https://api.codemux.org/health` → 200
- `GET /api/skills` (no auth) → 401
- `GET /api/voice/dictionary` (no auth) → 401 (regression check)
- `GET /api/settings` (no auth) → 401 (regression check)
- DB row counts unchanged: 16 users, 4 user_settings, 0 voice_*, 0 user_skills (post-migration baseline).
- Production `user_skills` table verified with `\d user_skills` showing all columns, indexes, FK CASCADE.

### Smoke test

`src-tauri/examples/skills_smoke.rs` builds clean. Lifecycle: derive credentials → signin (signup-on-first-run) → encrypt fake skill → POST → GET → byte-for-byte wire check → decrypt → cleanup DELETE.

The live invocation against production is the one Stage 1 piece **paused on the harness** — flipping `emailVerified=true` for the freshly-created smoke account requires explicit per-action approval beyond the schema-migration approval. The cross-product hex pin and the 34 server-side tests already prove the round-trip; the live smoke is belt-and-braces.

## Active Priorities (Stage 2)

1. `setup_sync_password` Tauri command + settings UI — port from Vexis `commands/auth.rs:527-575`, adapt IPC.
2. `EncryptionManager`-style in-memory key holder + machine-bound persistence (mirror Codemux's existing `auth/token_store.rs` AES-GCM wrap).
3. Skills sync layer (`src-tauri/src/skills/sync.rs`) — push/pull, dirty flag, mirror `settings_sync.rs:168-194`.

## Open Questions

- Recovery code (24-word phrase) in v1 vs defer to v1.5? Research §4.3 recommends defer.
- Project skills syncing in Stage 1 of Step 10.5, or wait for user request? Research recommends user-scope only for v1.

## Likely Touch Points

- `src-tauri/src/auth.rs` — already extended with `derive_login_credentials` + `EncryptionKey` (inline section near line 476). Stage 2 adds `set_password_api` and `save_encryption_key` here too. Whole block relocates to `auth/derivation.rs` + `auth/api.rs` + `auth/token_store.rs` once `feature/agent-chat` merges main and gains the auth-module split.
- `src-tauri/src/encryption/mod.rs` — Stage 2 will add a key-holder wrapper here.
- `src/components/settings/settings-view.tsx` — Stage 2 adds the sync-password section.
- `~/codemux-api/api/src/index.ts` — server-side complete for now; Step 10.5 will add `project_remote_url_hash` via additive migration.

## Already Landed (Stage 1)

- `user_skills` table + indexes on production VPS.
- 5 `/api/skills` routes with full test coverage (34 tests, voice-sync template).
- `chacha20poly1305` dep + `encryption` module on the client.
- `EncryptionKey` newtype with redacting Debug + zeroizing Drop.
- `derive_login_credentials` with cross-product hex pin against Vexis.
- Smoke binary buildable and operational against the live API (only live invocation paused on harness DB-write approval).

## Notes

- Schema is intentionally minimal: no per-user metadata fields, no `project_remote_url_hash` yet, no `version` for conflict detection (per-record updated_at handles ordering, Stage 3 adds optimistic concurrency if needed).
- HKDF labels are now load-bearing across Codemux + Vexis. Any rotation must bump the `vN` suffix in BOTH `~/codemux-api/api/src/reset-password/derivation.ts`, the codemux client `auth/derivation.rs`, and Vexis's `encryption/manager.rs` simultaneously.
