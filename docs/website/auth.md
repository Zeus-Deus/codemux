---
title: Authentication
description: Sign in with GitHub or email to sync settings across devices.
---

# Authentication

Codemux has an optional account system. Sign in to sync your settings across machines. The app works fully without an account — auth is only needed for cross-device sync.

## Creating an Account

### Email and Password

1. Open the login screen (shown on first launch, or via [Settings](settings.md) > Account)
2. Click "Sign up"
3. Enter your name, email, and password
4. Check your email for a verification link
5. After verifying, sign in with your email and password

### GitHub OAuth

1. Click "Sign in with GitHub"
2. Your system browser opens to authorize the Codemux app
3. After approval, the browser redirects to a local callback and you're signed in automatically

GitHub is the only OAuth provider currently supported.

## What an Account Gives You

**Settings sync** — Your personal settings (appearance, editor, terminal, git, notifications) sync across every machine you sign into. Change a setting on your desktop and it's there on your laptop.

That's the core value today. Future features may build on accounts, but right now it's about settings portability.

## What Stays Local

Not everything syncs. These stay on the current machine:

- Workspace layouts and tab arrangements
- Terminal presets
- Project setup/teardown scripts
- Sidebar state and UI preferences
- Agent tool configuration

Your workspace state is stored in a local SQLite database, not on the server. The server only stores your settings and account info.

## Session Handling

Once you sign in, you stay signed in. Sessions are long-lived — the token is stored encrypted on disk using AES-256-GCM with a machine-derived key.

Codemux re-verifies your session on launch and when the window regains focus (debounced to once per 5 minutes). If the token expires, you'll need to sign in again.

## Offline Behavior

The app works fine without an internet connection:

- **Cached auth** — If you've previously signed in, Codemux uses your cached credentials. All features work normally.
- **Settings changes** — Saved locally with a dirty flag. They sync back to the server automatically when connectivity returns.
- **No account** — Everything works except settings sync. No sign-in required.

## Signing Out

Go to [Settings](settings.md) > Account and click "Sign out". This clears your token and settings cache. Your local workspace state (layouts, projects, terminal history) is unaffected.

## Privacy

- **On the server**: Account info (email, name) and your synced settings
- **On your machine**: Everything else — workspace state, terminal sessions, project data, browser history, all stored in local SQLite
- **Auth token**: Encrypted on disk with a key derived from your machine ID, not stored in plaintext
