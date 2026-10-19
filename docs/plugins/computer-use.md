# Computer use

Computer use is an optional Official plugin. It starts off. Use a structured
bridge, such as browser tools or Unity tools, whenever one is available.

Enable **Computer use** in Settings → Plugins, then open its settings. Check the
installation, allow access in the current local workspace, choose an application
window, and select **Enable for this window**. The workspace choice is saved on
this device. The selected window and active control are never restored after a
restart. Switching workspaces stops control.

Install cua-driver yourself using the [official installation guide](https://cua.ai/docs/cua-driver/quickstart).
Codemux does not run the installer or grant operating-system permissions. This
plugin currently requires version **0.21.0**, whose policy and exact-window
manifest were evaluated. A different version needs another compatibility check.
Doctor warnings remain visible even when the driver's overall result is `ok`.

The title-bar indicator shows when control is enabled and when an agent is
calling its tools. The **Computer** chip beside the chat's browser status opens
the live view. **Stop** unregisters Codemux's direct input child using
immediate shutdown and waits for confirmed exit. It also closes the read-only
capture child. The indicator stays pending if shutdown cannot be confirmed.
The plugin never starts, stops, revokes, or reuses an existing driver daemon.

**Live view** opens the shared peek. It polls only the chosen window, only while
the view is mounted. The capture child is read-only and invisible to agents;
its snapshots do not replace the agent's element tokens or enter the chat.
Missing or refused capture shows a no-image state.

## Scope and limits

- Agent tools are registered only for the opted-in workspace and selected PID
  and native window ID. The manifest denies full-display capture.
- Click, type and scroll require explicit background delivery. Refused actions
  do not fall back to foreground input.
- Hold, drag, replay, key chords, clipboard, application launching and driver
  configuration changes are excluded from the tool roster. Click modifiers are
  restricted to an empty list.
- A driver policy filters `tools/list`; a separate capability manifest limits
  resources. Both files are private to this plugin and regenerated for each
  child. Driver configuration is read on child startup.
- Remote/SSH workspaces cannot control this computer.
- Capture and semantic background click/type worked for the owned XWayland GTK
  fixture on Hyprland with plain `DISPLAY=:0`. Capture showed only that window.
  Native Wayland capture was refused with `surface_identity_unproven` on the
  same host. Background scroll was refused in the isolated X11 fixture; support
  depends on the application and platform.
- macOS and Windows have not been tested with this plugin. Only direct mode is
  supported. Permission setups that require a daemon identity, and Windows
  SSH/autostart daemon workflows, are not supported by this integration.
- cua-driver's optional perception components are not bundled or enabled.

The direct child stopped typing when terminated in the isolated driver probe,
but terminating a held drag left the button down. A fresh process could not
release that runtime's held state. That is why hold/drag tools and modifier
chords remain unavailable. The daemon probe also showed input continuing after
proxy exit, revoke and `stop --socket`, so daemon mode is excluded.
