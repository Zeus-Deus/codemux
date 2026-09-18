# Project Brief

Prepare project context for a chat draft.

Build independently with the public SDK: `npm install`, `npm run build`, `npm run check`, `npm run pack`. Import the resulting `.cmxaddon` in Settings → Add-ons, review its access and enable it. No app rebuild is needed.

Before SDK publication, install the packed SDK and CLI with `npm install --no-save --package-lock=false /path/to/codemux-plugin-sdk-1.0.0.tgz /path/to/codemux-plugin-cli-1.0.0.tgz`. Then run the same commands. Neither package imports app internals.

Open the panel from the right-panel + menu. Refresh reads local project metadata and a bounded Git summary. Add to draft preserves existing input; a panel needs one unambiguous mounted composer. The composer action targets its own draft. Include changed filenames is configured in Settings. Refresh stores a private last-used preference; uninstall removes it unless Keep data is selected.
