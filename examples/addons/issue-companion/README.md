# Issue Companion

Browse GitHub issues and add a selected issue to a chat draft.

Build independently with the public SDK: `npm install`, `npm run build`, `npm run check`, `npm run pack`. Import the resulting `.cmxaddon` in Settings → Add-ons, review its access and enable it. No app rebuild is needed.

Before SDK publication, install the packed SDK and CLI with `npm install --no-save --package-lock=false /path/to/codemux-plugin-sdk-1.0.0.tgz /path/to/codemux-plugin-cli-1.0.0.tgz`. Then run the same commands. Neither package imports app internals.

Configure a GitHub repository owner/name in Settings. The optional bearer token stays in the host credential store. Issue loading sends only the configured repository path to api.github.com; it never uploads workspace data. Open in browser and Add to draft require separate explicit clicks. Public unauthenticated requests are supported, subject to GitHub rate limits.
