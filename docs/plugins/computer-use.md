# Computer use

Computer use is an Official plugin enabled by default. A saved on/off choice in
Settings → Extensions → Plugins takes precedence. Enabling the plugin makes its
settings available; it does not install or start the driver or grant window
control. Use a
structured tool, such as browser tools or app-specific plugin tools, whenever one
is available.

Open **Computer use** from its **Settings** button in Settings → Extensions →
Plugins, or under **Plugin settings** in the Settings navigation. The plugin's
row shows whether the driver is ready, separately from its on/off switch.
Opening the page checks for the driver automatically. When it is ready, allow
access in the current local workspace, choose an application window, and select
**Enable for this window**. The workspace choice is saved on this device. The
selected window and active control are never restored after a restart.
Switching workspaces stops control. Control expires after ten minutes; the UI
shows the deadline, stops the children, and requires another window selection.
The UI deadline starts before child startup, so it precedes the driver’s
ten-minute lifetime and idle limits.

After enabling a window, send a new message in Claude. A turn already in
progress may not have the newly enabled tools; Claude refreshes them between
turns. In Codex, enabled computer-use calls have no per-tool approval and run
unprompted in the opted-in workspace.

## Driver setup

This plugin requires cua-driver **0.21.0**, whose policy and exact-window
manifest were evaluated. A different version needs another compatibility check
and is reported as incompatible rather than ready.

Detection checks PATH first, then the standard installation locations:
`~/.local/bin/cua-driver` on Linux,
`%LOCALAPPDATA%\Programs\Cua\cua-driver\bin\cua-driver.exe` on Windows, and
`/Applications/CuaDriver.app/Contents/MacOS/cua-driver` followed by
`~/.local/bin/cua-driver` on macOS. The [macOS installer](https://github.com/trycua/cua/blob/70db98d1bcd92890d778f4978e0eb107a4b66c1b/libs/cua-driver/scripts/_install-rust.sh#L933)
links the user-bin path into that application bundle. Codemux's own managed copy
is checked last, so a compatible existing installation keeps being used.

When the driver is missing or another version, **Install driver** (or **Install
compatible driver**) downloads the pinned 0.21.0 release archive for this
platform from the [official release](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.21.0).
Codemux, not the plugin, pins each archive's URL, size and SHA-256 from the
release's `checksums.txt`. The install:

1. downloads over HTTPS into a temporary file under Codemux's data folder
   (`tools/cua-driver/` beside `plugin-data/`);
2. verifies the size and checksum;
3. copies only the listed files from the archive into a staging folder, runs
   `cua-driver --version` there and requires `cua-driver 0.21.0`;
4. renames the staging folder to `tools/cua-driver/0.21.0-<platform>/`, then
   runs the normal diagnostics.

A failure leaves any earlier managed copy in place and offers **Retry
installation**. The install never changes PATH, never writes outside that
folder, and never replaces a driver you installed yourself. It does not allow a
workspace, choose a window, start a daemon or grant operating-system
permissions.

Managed installation covers Linux and Windows on x86_64 and ARM64, using the
release's `-binary` archives (the same files the official installers copy).
It is not available on macOS: the official installer places the signed
`CuaDriver.app` in `/Applications`, and macOS attributes its Accessibility and
Screen Recording permissions to that bundle. A copy unpacked elsewhere would
not keep that identity, and replacing the bundle would overwrite the user's own
install. On macOS the page links to the [official installation guide](https://cua.ai/docs/cua-driver/quickstart)
and checks again afterwards.

When the driver runs but its diagnostics report a problem, often a missing
operating-system permission, the page shows each failing item and a **Check
again** button. Doctor warnings remain visible even when the driver's overall
result is `ok`.

**Advanced** holds an optional custom executable path, the resolved executable
and the full diagnostics. A custom path takes precedence and must pass the same
version and diagnostic checks; **Use automatic detection** clears it. Installing
the managed driver also clears a saved custom path.

The title-bar indicator shows when control is enabled and when an agent is
calling its tools. On desktop, the **Computer** chip sits beside Browser in **Workspace details →
Live** and opens the live view. Phones keep the chip in their compact status cluster. **Stop** unregisters Codemux's direct input child using
immediate shutdown and waits for confirmed exit. It also closes the read-only
capture child. The indicator stays pending if shutdown cannot be confirmed.
The plugin never starts, stops, revokes, or reuses an existing driver daemon.

**Live view** opens the shared peek at the bottom-right, above any composer
under that corner. It polls only the chosen window, only while
the view is mounted. The capture child is read-only and invisible to agents;
its snapshots do not replace the agent's element tokens or enter the chat.
Missing or refused capture shows a no-image state.

## Scope and limits

- Agent tools are registered only for the opted-in workspace and selected PID
  and native window ID. Control exposes only `get_window_state`, `click`,
  `type_text` and `scroll`. Window discovery is plugin-only, for the user’s
  window picker; agents cannot list other windows or read driver/session config.
  The manifest denies full-display capture.
- Click, type and scroll require explicit background delivery. Refused actions
  do not fall back to foreground input.
- Hold, drag, replay, key chords, clipboard, application launching and driver
  configuration changes are excluded from the tool roster. Click modifiers are
  restricted to an empty list.
- A driver policy filters `tools/list`; a separate capability manifest limits
  resources. Both files are private to this plugin and regenerated for each
  child. A plugin-only probe verifies the actual tool roster with the exact
  same files before control is exposed; the control child is checked again.
  Any extra tool stops startup and disposes the children. Driver configuration
  is read on child startup. Stale files from an ended page are swept on plugin
  registration and before startup; pending/live registrations keep their files.
- Remote/SSH workspaces cannot control this computer.
- Capture and semantic background click/type worked for the owned XWayland GTK
  fixture on Hyprland with plain `DISPLAY=:0`. Capture showed only that window.
  Native Wayland capture was refused with `surface_identity_unproven` on the
  same host. Background scroll was refused in the isolated X11 fixture; support
  depends on the application and platform.
- The GTK fixture's background actions sometimes reported `effect: "unverifiable"`;
  typing also suggested a foreground retry even when text appeared. Verify
  uncertain results with a fresh window image. Foreground retries remain blocked.
- macOS and Windows have not been tested with this plugin. Only direct mode is
  supported. Permission setups that require a daemon identity, and Windows
  SSH/autostart daemon workflows, are not supported by this integration.
- The Windows 0.21.0 [installer](https://github.com/trycua/cua/blob/70db98d1bcd92890d778f4978e0eb107a4b66c1b/libs/cua-driver/scripts/install.ps1#L112)
  exposes `cua-driver.exe` through directory junctions under
  `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin`. Both x64 and ARM64
  [release archives](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.21.0)
  were checksum-checked and contain native PE executables. They require no
  `.cmd` or `.bat` launcher.
- cua-driver's optional perception components are not bundled or enabled.

On Linux, an isolated X11 test through the real plugin MCP registry interrupted
a long typing call with immediate Stop. The child and its stdio wrapper had
exited when Stop returned, the input count stayed unchanged for another second,
the in-flight call failed, and a subsequent call was denied. No keys or buttons
remained held in that test.

The real title-bar Stop was also measured during a Claude background-click task
on the Hyprland GTK fixture. Both input and capture children had exited when
the indicator disappeared, 134 ms after the click. The fixture count stayed
unchanged from the Stop request through completion and another second.

The direct child also stopped typing when terminated in the isolated driver probe,
but terminating a held drag left the button down. A fresh process could not
release that runtime's held state. That is why hold/drag tools and modifier
chords remain unavailable. The daemon probe also showed input continuing after
proxy exit, revoke and `stop --socket`, so daemon mode is excluded.
