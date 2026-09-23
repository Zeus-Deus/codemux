# Project Brief

Prepare project context for a chat draft.

Build independently with the public SDK: `npm install`, `npm run build`, `npm run check`, `npm run pack`. Import the resulting `.cmxaddon` in Settings → Add-ons, review its access and enable it. No app rebuild is needed.

Before SDK publication, install the packed SDK and CLI with `npm install --no-save --package-lock=false /path/to/codemux-plugin-sdk-1.0.0.tgz /path/to/codemux-plugin-cli-1.0.0.tgz`. Then run the same commands. Neither package imports app internals.

Open the panel from the right-panel + menu. Refresh reads local project metadata and a bounded Git summary. Add to draft preserves existing input; a panel needs one unambiguous mounted composer. The composer action targets its own draft.

The **Include changed filenames** setting (Settings → Add-ons, on by default) decides whether Add to draft and the composer action list up to 20 changed paths in the brief. The **Show changed paths** checkbox is a private preference in the plugin's own storage and only controls the panel's path list. Both survive restarts; uninstalling removes them unless Keep data is selected.
