# Agent chat provider updates

Chat panes and draft chats check their selected provider on mount, then hourly
while visible. Results and in-flight checks are shared across panes. Dismissing
an update hides that installed/latest version pair for the current app session.
Network failures are unknown, not evidence that a provider is current.

The local desktop offers checks for Claude, Codex, Cursor, Grok, Hermes, and
OpenCode. Hermes uses the installation selected by its profile and its own
configured update channel. Remote browser clients do not offer host maintenance.

The backend detects the executable's owner again when Update is clicked. It
builds argument arrays itself, serializes maintenance operations, reports process
failures, and verifies the installed version afterward. The frontend cannot
supply an updater executable or shell command. A successful process exit alone
is insufficient to report success. Existing Codemux sessions are not restarted;
the success notice asks the user to restart Codemux to pick up updated runtimes.

## Installation methods

- **mise:** match the actual executable to an active tool in `mise ls --json`.
  Recognize Omarchy's original and locked `mise x` wrappers without launching
  their `mise use` install step during a passive check. Use the configured version
  selector for checks, preserving pins and ranges. On update, clear only that
  tool's metadata cache, run `mise upgrade <tool>`, then `mise reshim`. Probe the
  new `mise which --tool <tool>` path instead of an old inherited PATH entry.
  `MISE_UPGRADE_AUTO_PRUNE=false` preserves files needed by running sessions.
- **Omarchy:** identify the installed distribution or existing Omarchy theme
  paths. For mise tools, follow `omarchy update mise`'s
  `MISE_MINIMUM_RELEASE_AGE=0` policy. Invalidate mise release and aqua registry
  metadata for fresh discovery. Do not run the full system updater to update a
  single mise tool.
- **npm, pnpm, Bun:** use the global manager detected from the executable's real
  path. npm's active global prefix must also own that path. npm allows only the
  selected package's install scripts, including with npm 12.
- **Homebrew:** recognized formula/cask paths use `brew upgrade`.
- **Native installs:** Claude, Cursor, OpenCode and Hermes use their own updater.
  Claude version probing stays in the bundled sidecar.
- **System packages and unrecognized installations:** show the release notice
  with manual guidance. In particular, Omarchy's pacman-owned Cursor install
  directs users to `omarchy update`; a native updater must not overwrite files
  owned by pacman. This avoids creating a shadow npm installation or triggering
  an unrelated full system upgrade from a provider button.

Claude, Codex, Grok and OpenCode release discovery uses their npm release
metadata; Cursor uses the version in its official installer (read, never
executed); Hermes uses `hermes update --check`. mise-managed providers use
mise's configured release source instead.

## Verification

Frontend fixtures: `?providerUpdate=mise`, `npm`, `manual`, or `error` on the Vite
mock. The mock never runs an installer. Tests cover all six notice identities,
progress, retry, success, dismissal, hourly checks, deduplication and Hermes
installation isolation. Rust tests exercise fake executables for scoped cache
clearing, environment settings, re-resolution and verification after success,
failed commands and no-op installers.

References: [mise cache behavior](https://mise.jdx.dev/cache-behavior.html),
[mise settings](https://mise.jdx.dev/configuration/settings.html),
[Hermes updates](https://hermes-agent.nousresearch.com/docs/getting-started/updating),
[Cursor installer metadata](https://cursor.com/install). Omarchy behavior was
checked against the installed `omarchy-update-mise` and `omarchy-mise-install`
scripts; no system configuration is changed by this feature.
