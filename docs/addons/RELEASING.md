# Releasing independent feature plugins

The desktop, author tools, feature packages and catalog are separate releases.
Themes and footer presets do not use this pipeline. The implementation remains
unreleased while any acceptance gate in [the ledger](IMPLEMENTATION.md) is open.

## Prepare reviewable assets

From a committed checkout, run:

```sh
node scripts/addons/prepare-release.mjs /absolute/path/to/new-release-directory
```

The destination must not exist. The script exports the exact commit into a new
temporary directory, builds and packs the public SDK and CLI, then builds both
examples outside the app checkout using those tarballs. It typechecks each
example and verifies repeat packing gives identical bytes. It never builds the
desktop, changes the checkout, executes a plugin, or publishes an asset.
The **Independent add-on author distributions** workflow performs the same build
and retains its output as a commit-named artifact.

The output contains four distributions: the SDK and CLI npm tarballs, Project
Brief, and Issue Companion. `provenance.json` records the full source commit,
versions, package manifests, SHA-256 hashes and sizes; `SHA256SUMS` permits an
independent byte check. The hostile test fixture is excluded. A CI artifact is a
review candidate, not an approved catalog release.

## Review and publish

1. Require the final-source native Linux and Windows gates, including the exact
   candidate example packages on the stock installers. Review source, dependencies,
   permissions, licenses and the recorded provenance.
2. Through the repository's normal authorized release process, create a versioned
   GitHub Release at the recorded full source commit. Attach the exact reviewed
   `.cmxaddon` bytes, author tarballs, provenance and checksums. Do not rebuild or
   overwrite accepted version bytes while publishing. Publishing the SDK/CLI to
   npm is a separate authorized operation; before that, authors install the
   exact tarballs as documented in their READMEs.
3. Submit catalog entries with the actual public release asset URLs, exact source
   commit, hashes, sizes and normalized capabilities. Follow
   [catalog review](../../catalog/addons/README.md). The validator downloads the
   assets and inspects them inertly; it does not grant publisher approval.
4. After the reviewed catalog PR merges, its existing workflow publishes the
   immutable `addons-catalog-r<revision>` artifact. In `codemux-sitev2`, run
   `node scripts/pin-addon-catalog.mjs <artifact-url> <reviewed-sha256>`, run affected
   catalog tests and the site build, and review the pin change.
5. Verify that the website's download and copied install link resolve to the same
   release digest in native Settings review. Complete the final release ledger.

No workflow here uploads npm packages or publishes feature releases automatically.
The existing catalog publisher runs only after its reviewed main-branch merge.
Keep the catalog empty until real reviewed release assets exist; do not create
placeholder download URLs or claim author-provided metadata proves ownership.
