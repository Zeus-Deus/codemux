# Step 10 — Skills Sync: E2E Research Spike

> **RESEARCH NOTE.** Pre-implementation research or a spike. Some conclusions
> here were later revised or reversed by what actually shipped — read it as
> reasoning history, never as current behavior. Current truth lives in
> `docs/features/*`.

> **Outcome reversed (PR #112):** this spike recommended GO-E2E and Step 10 shipped that way, but skills sync was later moved to **server-side plaintext storage** because the E2E model forced GitHub OAuth / SSO users to invent a sync password before they could sync at all. Read this as the original design rationale, not current behavior — see `docs/features/skills-sync.md`.

- Purpose: Decide whether Codemux's user-skill sync should ship with end-to-end encryption (E2E) or server-side encryption only.
- Audience: Whoever owns Step 10 implementation.
- Authority: Pre-implementation research. No code yet.
- Read next: `~/projects/vexis/SECURITY.md`, `docs/features/settings-sync.md`, `docs/features/auth.md`.

## TL;DR

**Recommendation: GO E2E by reusing the Vexis pattern.** The protocol was already designed cross-product (`codemux-api-*` HKDF labels) and the backend code already lives in the same `~/codemux-api/api/` server that Vexis talks to. Lifting the crypto and derivation modules into Codemux is a 1–2 day port, not a greenfield E2E build. The marginal cost over server-side-only sync is roughly +1 week, and it earns Codemux the same "even a malicious server can't read your skills" property Vexis ships with today. **Total budget: ~2 weeks.**

Sections 1–4 below justify that recommendation; section 5 states it formally; section 6 gives effort estimates for all four options the user is choosing between.

---

## 1. Vexis E2E Architecture

### 1.1 Crypto primitives

- Module: `~/projects/vexis/src-tauri/src/encryption/mod.rs` (lines 1-103) and `encryption/manager.rs` (lines 1-342).
- AEAD: **XChaCha20-Poly1305** via the `chacha20poly1305` crate v0.10. 24-byte nonces, 16-byte auth tag, OS-RNG nonce per encryption (`mod.rs:67-79`, `mod.rs:198-211`).
- KDF: **Argon2id** via the `argon2` crate v0.5. Parameters: `m=64 MiB`, `t=3 iterations`, `p=4 parallelism`, 32-byte output. ~300-500 ms on modern hardware (`manager.rs:295-316`).
- HKDF split: HKDF-SHA256 fans the master material into two domain-separated 32-byte secrets (`manager.rs:323-342`).
- Pure-Rust dependency stack — no libsodium, no Web Crypto. Builds clean on every Tauri target.

### 1.2 Master-key derivation

```
password ─┐
          ├─► Argon2id(salt = SHA256("codemux-api-master-v1\0" || normalize(email)))
          │          │
          │          └─► master_material  (32 bytes)
          │                     │
          │                     ├─► HKDF-Expand("codemux-api-auth-secret-v1")    → AuthSecret  (32 bytes, base64)
email ────┘                     │
                                └─► HKDF-Expand("codemux-api-encryption-key-v1") → encryption_key (32 bytes, raw)
```

Implementation: `derive_login_credentials()` at `manager.rs:276-284`. Domain labels are constants at the top of `manager.rs`. The salt is deliberately deterministic per email — that is what makes multi-device sync work without device pairing.

**Storage contract:**

- Server stores: `bcrypt(AuthSecret)` (via Better Auth's password column) and ciphertext blobs. Nothing else.
- Client stores: `encryption_key` cached in `~/.local/share/vexis/sync-key.enc`, itself wrapped with AES-256-GCM under a machine-bound key derived from `/etc/machine-id` (`auth/token_store.rs:96-105`). Cannot be copied to a different machine.
- In-memory: `EncryptionManager::key: Mutex<Option<[u8; 32]>>` (`manager.rs:125-214`), zeroed on logout (`manager.rs:168-180`). No `zeroize` crate yet — flagged but not blocking.

### 1.3 SSO + password coexistence

This is the part that matters most for Codemux because Codemux's primary auth is GitHub OAuth.

- GitHub OAuth alone gets you a session, not a sync key. `login_github` (`commands/auth.rs:222-226`) leaves `sync_available=false`.
- The user runs `setup_sync_password` exactly once (`commands/auth.rs:527-575`). It takes a password ≥ 8 chars, runs `derive_login_credentials`, and calls `set_password_api` to register that password against the existing OAuth account on the server. After this, the user is a hybrid OAuth + password account.
- On subsequent devices the user signs in via GitHub again, then runs `setup_sync_password` again with the same password — the deterministic derivation produces the same `encryption_key`, so the new device can decrypt blobs the first device wrote.
- Key UX point: Vexis does **not** route Better Auth's own password hashing. It treats the 32-byte `AuthSecret` as a high-entropy opaque "password" string that Better Auth bcrypts. The server never sees the raw user password.

### 1.4 Recovery and lockout

There is no recovery code, no trusted-device flow, no escrow. Lost password = lost data. This is documented loud and clear (`PRIVACY.md:74-76`, `SECURITY.md:42-55`) and surfaced in the UI as "This will delete all synced data from the server… This cannot be undone." (`settings-view.tsx:1830-1834`).

The "forgot password" path is a Better Auth email reset, after which the user accepts that their old ciphertext is now garbage and runs `resetSync` (`commands/auth.rs` → `commands.ts` → `settings-view.tsx:1606`) which truncates the server-side blobs. That is the entire recovery story.

### 1.5 Multi-device

Pure password-based. Same `(password, email)` → same `encryption_key` on every device, by construction. No QR pairing, no device approval. The salt travels via the email itself.

The cached `sync-key.enc` is machine-bound, so a stolen file from another laptop is useless without that machine's `/etc/machine-id`.

### 1.6 Server-side data shape

Vexis's sync tables (deployed on `api.codemux.org`, schema mirrored from `infrastructure doc:281-315`):

- `voice_dictionary_entries(id, user_id, trigger_ciphertext, trigger_nonce, replacement_ciphertext, replacement_nonce, category, created_at, updated_at)`
- `voice_transcriptions(id, user_id, text_ciphertext, text_nonce, raw_text_ciphertext, raw_text_nonce, duration_ms, created_at)`

Encrypted **per field**, not per row. Plaintext metadata: `category`, `duration_ms`, timestamps. Server can sort and paginate but cannot read the actual content.

Wire format: `{ciphertext: base64, nonce: base64}` JSON pairs. Nothing fancy.

### 1.7 UX evidence from commits and docs

- `0d788c9 feat(auth): zero-knowledge password split via codemux-api-* derivation` — the redesign that introduced the cross-product protocol. Commit message explicitly calls out "future drift is caught" by the cross-product compat tests.
- `8afad2f feat(sync): GDPR account deletion + 429 rate-limit handling` — operational hardening.
- `f724f50 docs(legal): privacy policy, terms of service, security disclosure` — the moment they froze the threat model.
- `23045a5 test(auth): wire-format leak checks + multi-input cross-product compat pins` — the cross-product compatibility tests that would catch any drift between Vexis and Codemux derivation.

No commit messages in the post-launch tree mention users complaining about the password prompt or losing data — but Vexis is also pre-launch (sync endpoints aren't deployed yet per `sync/mod.rs:8-10`), so this is "no field evidence" rather than "no problems."

---

## 2. Reusability Assessment

### 2.1 What lifts wholesale

These three files can be copied into Codemux's `src-tauri/src/` with minimal edits:

1. **`encryption/mod.rs`** — `encrypt`, `decrypt`, `EncryptedData`. Generic. 47 tests. Zero Vexis-specific code. Verbatim copy.
2. **`encryption/manager.rs::derive_login_credentials` + helpers** — the full Argon2id + HKDF split. The domain labels are *already* `codemux-api-*` so no relabeling needed for cross-product key compatibility.
3. **`auth/token_store.rs` machine-bound AES-GCM file storage** — already pattern-matches Codemux's existing token persistence in `src-tauri/src/auth.rs:184-213` (which already uses AES-256-GCM with a machine-bound key, just for the bearer token).

### 2.2 What needs adaptation

- **`commands/auth.rs::setup_sync_password` flow** — the logic is generic but the IPC wiring is Tauri-command-shaped and assumes specific `AuthStatus` return types. Adapt, don't lift.
- **`commands/auth.rs::provide_password_for_sync`** — the "I have the auth token but lost the sync key" recovery path. Same: adapt the logic, rewire the IPC.
- **Settings UI for the password setup + recovery panels** — `settings-view.tsx:1719-1850` is the reference. Codemux's settings panel is React + shadcn just like Vexis, so the components transfer in spirit but need to be rewritten against Codemux's design tokens.

### 2.3 What needs new code (Codemux-specific)

- **Skill payload shape**: skills are markdown files with a path scope (`~/.claude/skills/`, `~/.codemux/skills/`, project-scoped). The encrypted-record schema needs to encode `(provider, scope, name)` as plaintext metadata so the server can list and paginate without decrypting, while the markdown body stays opaque.
- **Sync orchestration**: `src-tauri/src/skills/sync.rs` (new) modelled on the existing `src-tauri/src/settings_sync.rs:168-194` cache-with-dirty-flag pattern.
- **Backend endpoints**: `GET/POST/PUT/DELETE /api/skills` on `api.codemux.org`. Mirrors `/api/settings` (settings_sync.rs:222-327 client side) but adds list pagination and per-record ops.

### 2.4 Pitfalls a port has to respect

- **The `codemux-api-*` domain labels are now load-bearing across Vexis and Codemux**. Changing them invalidates every existing Vexis user's data. Codemux must adopt them as-is.
- **Email normalization must match Vexis exactly** — lowercase + trim, no NFKC/IDN. If Better Auth normalizes differently on the server, multi-device sync breaks silently. Vexis's `manager.rs:624-676` normalization is the canonical source.
- **Password change rotates the encryption key** — there is no DEK wrapping layer, so changing the password re-encrypts everything. Vexis accepts this; Codemux should too unless it wants to add a key-wrapping layer (more complexity, not recommended for v1).
- **Lazy key verification** — Vexis defers password validation to first sync attempt to avoid a 300 ms cold-start hit (`commands/auth.rs:387-403`). Worth keeping; document the trade-off.

---

## 3. Migration Path for Codemux

### 3.1 Today's state

- Codemux client has **no E2E encryption anywhere**. Settings sync is plaintext JSON over HTTPS (`src-tauri/src/settings_sync.rs:222-327`). The only client-side crypto is AES-256-GCM token-at-rest in `src-tauri/src/auth.rs:184-213`.
- Skills are local-only. No backend table, no sync code, no plan doc. Discovery via filesystem scan in `src-tauri/src/commands/skills.rs:21-55` and a watcher at `src-tauri/src/skills/watcher.rs`.
- Backend is the same Better Auth + Hono + Postgres + Bun stack on `api.codemux.org` that Vexis already talks to. The `user_settings` table is plaintext JSONB; no `user_skills` table exists yet.
- Better Auth supports both GitHub OAuth and email/password in parallel — a user can have both credentials on one account (this is what makes Vexis's `setup_sync_password` work).

### 3.2 Concrete migration plan

**Phase A — Crypto port (1-2 days)**

1. Copy `vexis/src-tauri/src/encryption/mod.rs` → `codemux/src-tauri/src/encryption/mod.rs` verbatim.
2. Copy `vexis/src-tauri/src/encryption/manager.rs` → `codemux/src-tauri/src/encryption/manager.rs`. Domain labels (`codemux-api-*`) already match.
3. Add the cross-product compat test from Vexis's `23045a5` commit so any future drift between Codemux and Vexis derivation is caught at CI time.
4. Wire `EncryptionManager` into the existing `AuthState` in `src-tauri/src/auth.rs`.

**Phase B — Backend (2-3 days)**

1. Add `user_skills` table to `~/codemux-api/api/` migrations:
   ```sql
   CREATE TABLE user_skills (
     id BIGSERIAL PRIMARY KEY,
     user_id TEXT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
     provider TEXT NOT NULL,        -- 'claude' | 'codex' | 'opencode' | 'codemux'
     scope    TEXT NOT NULL,        -- 'user' | 'project'
     name     TEXT NOT NULL,        -- e.g. 'codemux-release'
     content_ciphertext TEXT NOT NULL,
     content_nonce      TEXT NOT NULL,
     created_at TIMESTAMPTZ DEFAULT NOW(),
     updated_at TIMESTAMPTZ DEFAULT NOW(),
     UNIQUE (user_id, provider, scope, name)
   );
   CREATE INDEX user_skills_listing ON user_skills (user_id, provider, scope, name);
   ```
2. Add `GET /api/skills`, `POST /api/skills`, `PUT /api/skills/:id`, `DELETE /api/skills/:id` mirroring the `/api/settings` pattern. Bearer auth via the existing `authenticateBearer()` helper.
3. Per-user cap (suggest 500 skills, 64 KiB each) and 429 rate-limiting like Vexis (`8afad2f`).

**Phase C — Client sync layer (2-3 days)**

1. New `src-tauri/src/skills/sync.rs` modelled on `settings_sync.rs:168-194`.
2. Encrypt `name` and `content` per-record before upload using the existing `encrypt()` from Phase A. Plaintext metadata: `provider`, `scope`, timestamps. (Decision: encrypt `name` because skill names can themselves leak workflow detail. Open question — see §4.)
3. Local cache at `~/.local/share/codemux/skills-cache.json` with dirty flag.
4. Tauri commands: `sync_skills`, `push_skill`, `pull_skills`, mirroring the `settings_sync` shape.

**Phase D — Auth flow extension (2-3 days)**

1. Port Vexis's `setup_sync_password`, `provide_password_for_sync`, `reset_sync` commands into `src-tauri/src/commands/auth.rs`.
2. Settings UI: new "Sync password" section in `src/components/settings/settings-view.tsx` modelled on Vexis's `settings-view.tsx:1706-1850`. Three states: not-set / set / lost-key-recovery.
3. Onboarding nudge: a one-line banner the first time a user opens the Skills panel after GitHub OAuth, asking them to set a sync password to enable sync (skips for users who picked email/password at signup since their key already exists).

**Phase E — Polish (1-2 days)**

1. The "lost password = lost skills" warning copy. Steal Vexis's verbatim.
2. Sync status indicator in the Skills panel.
3. Empty-state UX: what new-device first-launch looks like before the user enters their sync password.

**Total: 8-13 working days, call it ~2 weeks.**

### 3.3 Key API server change

The `codemux-api/api/` server already authenticates against the shared user table. The only meaningful additions are the new `user_skills` table and the four endpoints. No Better Auth feature flags need toggling — `set-password` for OAuth-only users already works (Vexis uses it).

---

## 4. UX Comparison

### 4.1 What Vexis got right

- **One-time `setup_sync_password` after OAuth.** Users sign in with GitHub, then are prompted exactly once to set a sync password. After that, the password is invisible — it doesn't gate normal use, only re-syncs on a new device.
- **Lazy key verification on cold start.** Saves a 300 ms network round-trip on every launch. Wrong-password discovery happens at first sync attempt and routes through the recovery panel.
- **Machine-bound cached key.** `sync-key.enc` survives app restarts, so the password prompt is genuinely once-per-device, not once-per-launch.
- **Loud "lost password = lost data" warnings** in the docs and UI. Vexis chose clarity over recovery complexity.

### 4.2 What's unproven

- **No field evidence.** Vexis sync endpoints aren't live yet (per `sync/mod.rs:8-10`). The UX has been tested manually but not by real users at scale. Codemux can't borrow lessons learned that haven't been learned.
- **No recovery code escape hatch.** Some users will absolutely lose their password. The Vexis answer is "you reset, you lose your sync data." For voice transcriptions that's acceptable; for skills (which represent real authoring effort) the consequence is heavier.

### 4.3 What Codemux should consider doing differently

- **Skills are not voice transcriptions.** A user who loses their sync password loses their custom skills, which is more painful than losing voice dictionary entries. Two mitigations worth considering:
  - **Local export before reset.** Before running `resetSync`, the client should write all decrypted skills to a local backup directory. Cheap to add, removes most of the data-loss anxiety.
  - **Optional recovery code.** Generate a 24-word recovery phrase at password setup, displayed once. Encrypts the master key under a second derivation. Adds complexity but matches user expectations from password managers. Recommend deferring to v1.5.
- **Encrypted name vs plaintext name.** Vexis encrypts dictionary `trigger`/`replacement` per-field. For skills, the *name* itself can leak workflow context (`codemux-release.md`, `internal-vault-unlock.md`). Recommend encrypting the name. The plaintext metadata is sufficient for listing.
- **Don't gate skills sync on password setup.** First-time GitHub-OAuth users should see skills sync as opt-in via the password setup banner, not a hard wall. Match Vexis's model: sync is available after setup, the app works without it.

---

## 5. Recommendation

**GO E2E (reuse Vexis).**

Three reasons:

1. **The protocol is already cross-product.** Vexis's domain labels are `codemux-api-*` by deliberate design — the cross-product compatibility test in `23045a5` will catch any drift. Codemux is the second product the protocol was always meant to serve.
2. **The backend is already the same server.** `api.codemux.org` already runs the auth and Postgres that Vexis uses. Adding a `user_skills` table and four endpoints alongside `/api/settings` and `/api/voice/*` is incremental work, not a new system.
3. **The marginal cost over server-side-only is small** — about +1 week in total. Skipping E2E for v1 saves a week now but commits Codemux to either (a) shipping a worse threat model than its sister product, or (b) doing a painful migration later when users have plaintext skills already on the server.

The only honest argument against E2E for skills specifically is the recovery-pain trade-off: skills represent authoring effort, and "lost password = lost skills" is heavier than "lost password = lost dictionary entries." Mitigate this with the local-export-before-reset behavior in §4.3 — it's a 30-line addition that converts catastrophic loss into a recoverable inconvenience.

---

## 6. Effort Estimates

| Option | Description | Estimate | Confidence |
|---|---|---|---|
| **1. No E2E** | Add `user_skills` table (plaintext JSONB) + four endpoints + `skills_sync.rs` modelled on `settings_sync.rs`. Skip all crypto and password-flow work. | **~1 week** (5-7 days) | High — pattern is identical to existing settings sync. |
| **2. E2E with Vexis reuse** | Phase A-E from §3.2. Crypto module copy, derivation copy, sync layer, password flow port, polish. | **~2 weeks** (8-13 days) | High — every component has a working analog in Vexis. |
| **3. E2E with new pattern** | Greenfield E2E without reusing Vexis. Pick a different KDF/AEAD, design own derivation, build own multi-device flow, build own recovery story. | **~3-4 weeks** (15-25 days) | Medium — most uncertainty is in the multi-device + recovery design that Vexis already solved. |
| **4. Defer Step 10** | Don't ship skills sync now. Skills stay local-only until users ask. | **0 weeks** | High — but kicks the can. |

**Recommendation as a sentence:** Pick Option 2. The +1 week over Option 1 buys a stronger threat model, parity with Vexis, and a backend story that doesn't need re-architecting later. Option 3 is hard to justify when the cross-product protocol is already there. Option 4 is the right call only if the user count for sync is currently zero and the team has higher-priority work.

---

## Open Questions Before Implementation

- Does the team want the optional recovery-code path (§4.3) in v1, or defer to v1.5?
- Encrypt skill *names* or leave them plaintext for server-side search? (Recommendation: encrypt; rely on client-side search.)
- Per-user storage cap. Vexis hasn't published one; for skills, suggest 500 skills × 64 KiB = ~32 MiB per user.
- Project-scoped skills (`./.codemux/skills/`) — sync them too, or only user-scope (`~/.codemux/skills/`)? Project skills usually live in git repos already, so syncing them via Codemux would duplicate. Recommend: user-scope only for v1.
- Does the local-export-before-reset behavior land in v1 or v1.1? (Recommendation: v1, it's small.)
