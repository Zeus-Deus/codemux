# @codemux/plugin-cli

Run `codemux-plugin init my-plugin`, then in that directory run `npm install`,
`npm run build`, `npm run check`, and `npm run pack`. Before public npm publication,
install packed local SDK/CLI tarballs in place of the exact 1.0.0 dependencies.

- `build`: bundles TypeScript/Preact and the SDK adapter into `plugin.js`.
- `check`: validates declared metadata and required distribution files without
  evaluating plugin code. The desktop repeats authoritative native validation.
- `pack`: writes a deterministic gzip/tar `.cmxaddon` with only permitted files
  and prints its SHA-256. Install from Settings → Add-ons once available.
- `dev`: watches source and manifest changes, rebuilds and repacks. Desktop reload
  requires explicit Developer mode and package selection. It does not grant new
  permissions or silently enable a package.

Only the author machine needs Node or a compiler. The official app never runs
npm, dependency installation, package scripts, or a third-party native binary.
