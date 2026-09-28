# @codemux/plugin-cli

Run `codemux-plugin init my-plugin`, then in that directory run `npm install`,
`npm run build`, `npm run check`, and `npm run pack`. Before public npm publication,
install packed local SDK/CLI tarballs in place of the exact 1.0.0 dependencies.
Requires Node.js 20 or later.

- `init <directory> [--id publisher.name]`: writes a starter with pinned
  dependencies, strict TypeScript, a `workspace.read` sample permission, a panel
  with a command that opens it, the MIT license text and a `.gitignore`. Without
  `--id` the ID is `example.<directory name>`; replace the `example` publisher
  before publishing, because the ID cannot change once users install it.
- `build [--sourcemap]`: bundles `src/index.tsx`, whose default export is the
  `definePlugin` result, with the SDK adapter into one `plugin.js`. Dependencies
  resolve through `module` or `main` when they have no exports map, and
  `process.env.NODE_ENV` is `"production"`. `--sourcemap` also writes `source.map`
  for developer diagnostics; a build without it removes a previous one.
- `check`: validates the manifest with the same rules as the desktop, and the
  required distribution files, without evaluating plugin code. Versions are
  SemVer without a `v`; `api` uses comma-separated comparators such as `^1.0.0`
  or `>=1.0.0, <2.0.0`. The desktop repeats authoritative native validation.
- `pack [--out <file>]`: writes a deterministic gzip/tar `.cmxaddon` with only
  permitted files, by default `<id>-<version>.cmxaddon`, and prints its SHA-256.
  Install it from Settings → Add-ons.
- `dev [--sourcemap] [--out <file>]`: rebuilds and repacks on every change to
  `src/`, `manifest.json`, `README.md`, `LICENSE` or `NOTICE`, and keeps watching
  after a failed build. The package always goes to the same path, by default
  `dist/<id>.cmxaddon`, replaced atomically; select that file in the app's
  Developer mode. Reloading requires explicit Developer mode and package
  selection. It does not grant new permissions or silently enable a package.

Only the author machine needs Node or a compiler. The official app never runs
npm, dependency installation, package scripts, or a third-party native binary.
