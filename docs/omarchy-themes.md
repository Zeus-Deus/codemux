# Following Omarchy

A fresh CodeMux desktop installation automatically follows Omarchy when it finds a valid desktop palette. It does not matter whether CodeMux was installed through Omarchy's menu, a package manager, or another installer. Existing theme choices are preserved when upgrading.

Open **Settings → Appearance → Change** (or search for `theme` in the command palette):

- **Follow Omarchy** uses the current desktop palette and updates without restarting CodeMux.
- Choosing a built-in or custom theme stops following. Omarchy theme changes will not override that choice.
- Select **Follow Omarchy** again to resume following the latest palette.
- **Customize a copy** opens an independent snapshot in Theme Studio. Saving and applying the copy stops following; canceling does not change the desktop or save a new theme.

Following is local to the desktop installation. It does not replace the account's saved manual theme or make a remote browser follow the server's desktop. Manual theme selection continues to use account settings and is saved locally when offline.

Both light and dark desktop palettes are supported. CodeMux keeps its own layout and typography. Terminals and syntax colors follow the app by default; the existing **Terminal → Color theme** override remains available.

## Compatibility

CodeMux reads the current palette under `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` (normally `~/.local/state/omarchy/current/theme/colors.toml`), with support for the legacy config location. Both semantic color names and legacy ANSI slots are supported. Palette files are data only; CodeMux does not execute theme scripts or modify Omarchy's configuration.

During a theme switch, incomplete or invalid data leaves the last good palette on screen. If Omarchy is unavailable when CodeMux starts, the saved manual theme is used; the following preference is retained.
