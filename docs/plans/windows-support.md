# Windows Support Work Plan

- Purpose: Track the remaining work before Windows support is considered polished.
- Audience: Anyone touching platform-conditional code, the release pipeline, or Windows-only behavior.
- Authority: Active work plan only. Current Windows reality lives in `docs/core/STATUS.md` § "Windows Support".
- Update when: A gating item lands or live Windows evidence changes the priority order.
- Read next: `docs/core/STATUS.md`, `docs/features/terminal.md`, `docs/archive/windows-support-2026-04-audit.md`
- Status: ACTIVE — Windows ships in the release matrix; Authenticode, OpenFlow portability, and live integration coverage remain.

## Goal

Turn the already-shipping Windows build into a polished, low-friction release by removing the remaining trust prompt, feature gate, and live-test gaps. Do not reopen completed porting work unless current Windows evidence demonstrates a regression.

## Active Priorities

1. **Authenticode signing.** Choose OV versus EV based on budget and SmartScreen requirements, provision the certificate securely in GitHub Actions, sign the NSIS artifact, and verify update signatures still merge correctly into `latest.json`.
2. **OpenFlow on Windows.** Replace the bash-dependent wrapper path in `openflow::prompts`, remove the frontend/backend Windows gate only after equivalent quoting, cancellation, and process-tree behavior is tested.
3. **Live integration coverage.** Add or schedule a real Windows runner matrix for PTY lifecycle, workspace/worktree creation, preset launch, agent spawn, restart/reattach, control named-pipe contention, and auto-update smoke.
4. **Validate the remaining PTY edge cases.** Reproduce before changing the waiter/runtime cleanup path or Windows reader batching; treat the archived audit's unchecked source lines as historical leads, not an automatic to-do list.

## Open Questions

- Is an OV certificate sufficient for the expected install volume, or does early SmartScreen reputation justify EV?
- Which Windows tests can run reliably on hosted CI, and which require a persistent self-hosted desktop runner?
- Should OpenFlow use native PowerShell command construction or remove shell wrappers entirely through a shared Rust process API?

## Likely Touch Points

- `.github/workflows/release.yml`
- `src-tauri/src/terminal/`
- `src-tauri/src/openflow/`
- `src-tauri/src/commands/openflow.rs`
- `src-tauri/src/os_input.rs`
- `src-tauri/src/control.rs`
- `src-tauri/src/agent_browser.rs`
- `docs/core/STATUS.md`

## Already Landed

- Windows has shipped since `v0.1.20`; the current release workflow builds Linux and `windows-latest` independently, publishes NSIS, and merges both updater entries into one `latest.json`.
- PowerShell-first shell selection and command injection, upstream `portable-pty 0.8.1`, parent-process console suppression, Windows scrollback flush backstop, and lazy active-workspace PTY hydration.
- Named-pipe control transport with `ERROR_PIPE_BUSY` retry, Windows port detection, Chromium/Edge discovery, browser argv/process cleanup, and native `SendInput` Tier-3 input injection.
- Undecorated-window controls on full-screen views, Windows editor discovery, Claude hooks, preset terminators, and cross-platform release/update plumbing.

The detailed April 2026 audit and first-machine smoke history are preserved at `docs/archive/windows-support-2026-04-audit.md`.
