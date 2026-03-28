# Auth System

- Purpose: Describe the authentication subsystem, what it supports, and current constraints.
- Audience: Anyone working on auth, login, session handling, or API integration.
- Authority: Canonical auth feature doc.
- Update when: Auth methods, token handling, API endpoints, or security model change.
- Read next: `docs/features/settings-sync.md`, `docs/core/STATUS.md`

## What This Feature Is

Desktop authentication that gates the app behind a login screen. Supports GitHub OAuth and email/password with email verification. Tokens are encrypted on disk with a machine-bound key.

## Current Model

### API Server

The auth backend is a Better Auth API server at `api.codemux.org` (Bun + Hono + Drizzle + Postgres). The desktop app communicates with it over HTTPS. Override with `CODEMUX_API_URL` env var for local development.

### Auth Methods

- **GitHub OAuth**: Primary. Opens system browser, redirects through API, callback to localhost server.
- **Email/password**: Sign up with name + email + password. Requires email verification before first sign-in.
- **Forgot password**: Sends reset link. Response is generic (doesn't leak whether email exists).

### Desktop OAuth Flow

1. Frontend calls `start_oauth_flow()`
2. Backend generates CSRF state token (32 random bytes, base64url, 10-min TTL)
3. Backend starts TCP listener on `127.0.0.1:0` (random port, 5-min timeout)
4. Backend opens system browser to `api.codemux.org/api/auth/desktop/connect?provider=github&state={state}&port={port}`
5. User authorizes on GitHub
6. API redirects to `http://127.0.0.1:{port}/auth/callback?token=...&expiresAt=...&state=...`
7. Localhost server validates CSRF state (one-time use), saves encrypted token
8. Backend emits `auth-state-changed` event to frontend
9. Server responds with HTML success page and shuts down

### Token Storage

- **Algorithm**: AES-256-GCM (authenticated encryption)
- **Key derivation**: SHA-256 of machine ID + 16-byte random salt
- **Machine ID sources**: `/etc/machine-id` > `/var/lib/dbus/machine-id` > hostname > fallback
- **File format**: Binary — `salt (16B) || nonce (12B) || ciphertext + GCM tag`
- **Location**: `~/.local/share/codemux/auth-token.enc`
- **Content**: Encrypted JSON `{token, expires_at}`

### CSRF Protection

- HashMap of state token to generation timestamp, mutex-protected
- Tokens are one-time use (removed after validation)
- Auto-pruned after 10 minutes
- Cleanup runs on both generate and validate

### Session Recovery

- On app startup: `checkAuth()` loads token from disk, checks expiry, verifies with `/api/auth/desktop/verify`
- 200 response: authenticated, triggers background settings sync
- 401 response: token cleared locally
- Network error: token kept (offline tolerance), app enters dev bypass

### Dev Mode Bypass

When `checkAuth()` fails (API unreachable or any error), the frontend auto-bypasses auth with a dev placeholder user (`id: "dev-local"`, `email: "dev@localhost"`). No account needed for local development. The `devBypass` flag is set in the auth store.

### App Gating

The login screen is shown when `isAuthenticated: false`. While `isLoading: true`, a pulsing logo is shown to prevent flash of login UI. Once authenticated (or dev-bypassed), the main app renders.

## What Works Today

- GitHub OAuth with localhost callback server
- Email/password sign-in and sign-up
- Email verification flow (signup does not save token; user must verify then sign in)
- Forgot password with generic response
- AES-256-GCM encrypted token storage with machine-bound key
- CSRF state protection with 10-min TTL and one-time use
- Session recovery on startup with offline tolerance
- Dev mode bypass when API is unreachable
- Bearer token auth for all API calls
- `auth-state-changed` Tauri event for frontend reactivity
- Login screen with 4 views: sign-in, sign-up, verify-email, forgot-password
- Sign-out clears token file and settings cache

## Current Constraints

- No refresh token mechanism — expired tokens are cleared and user must re-authenticate
- Rate limiting is handled server-side, not in the desktop client
- OAuth uses localhost callback only (no `codemux://` deep link yet)
- Session recovery does not run on window focus — only on app startup
- Dev bypass is all-or-nothing: any `checkAuth()` failure triggers it

## Important Touch Points

- `src-tauri/src/auth.rs` — Encryption, token storage, CSRF state, machine key derivation
- `src-tauri/src/commands/auth.rs` — Tauri commands: OAuth flow, email sign-in/up, check auth, sign out
- `src/stores/auth-store.ts` — Zustand store: user state, dev bypass, loading flags
- `src/components/auth/login-screen.tsx` — Login UI with all 4 views
- `src/tauri/types.ts` — `AuthUser`, `AuthResponse` types
