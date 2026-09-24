# Releasing independent feature plugins

The desktop, author tools, feature packages and catalog are separate releases.
Themes and footer presets do not use this pipeline. The implementation remains
unreleased while any acceptance gate in [the ledger](IMPLEMENTATION.md) is open.
Merging to `main` publishes nothing; every publication below is an explicit
maintainer action.

## Release namespaces

Desktop releases own the `v*` tag namespace and the repository's **Latest**
release. The desktop updater (`releases/latest/download/latest.json`),
`scripts/install.sh` and the hosted-client deploy all read Latest, and pushing a
`v*` tag starts the desktop Release workflow. Every other release in this
repository uses its own tag prefix and is created with `--latest=false`:

| Release | Tag |
| --- | --- |
| Feature package | `addon-<plugin id>-v<version>`, e.g. `addon-codemux.project-brief-v1.0.0` |
| Author tool | `plugin-sdk-v<version>` and `plugin-cli-v<version>` |
| Catalog | `addons-catalog-r<revision>`, created only by the catalog workflow |

After creating any of them, confirm that
`gh api repos/Zeus-Deus/codemux/releases/latest --jq .tag_name` still prints the
current desktop `v*` tag. Never pass `--latest` for these releases.

## Prepare reviewable assets

From a committed checkout of the exact `main` commit you intend to tag, run:

```sh
node scripts/addons/prepare-release.mjs /absolute/path/to/new-release-directory
```

The destination must not exist. The script exports the exact commit into a new
temporary directory, builds and packs the public SDK and CLI, then builds both
examples outside the app checkout using those tarballs. It typechecks each
example and verifies repeat packing gives identical bytes. It never builds the
desktop, changes the checkout, executes a plugin, or publishes an asset. It
warns when the commit is not on a fetched remote `main`.

The **Independent add-on author distributions** workflow performs the same build.
A `workflow_dispatch` run on `main` retains its output as
`addon-author-release-<commit>`. A pull-request run builds the proposed head
commit, never the synthetic merge commit, and retains it as
`addon-author-candidate-pr-<number>-<commit>` for review only. Release assets
come only from a dispatch run, or a local run, on the exact commit that will be
tagged, and that commit must be reachable from `main`.

The output contains four distributions: the SDK and CLI npm tarballs, Project
Brief, and Issue Companion. `provenance.json` records the full source commit,
versions, package manifests, SHA-256 hashes and sizes; `SHA256SUMS` permits an
independent byte check. The hostile test fixture is excluded. A CI artifact is a
review candidate, not an approved catalog release.

## Review and publish

1. Require the final-source native Linux and Windows gates, including the exact
   candidate example packages on the stock installers. Review source, dependencies,
   permissions, licenses and the recorded provenance.
2. Through the repository's normal authorized release process, create one GitHub
   Release per distribution at the recorded full source commit, using the tags
   above. Attach the exact reviewed bytes with `provenance.json` and `SHA256SUMS`.
   Do not rebuild or overwrite accepted version bytes while publishing. For
   example:

   ```sh
   dir=/absolute/path/to/reviewed-release-directory
   commit=$(jq -er .sourceCommit "$dir/provenance.json")
   (cd "$dir" && sha256sum --check SHA256SUMS)
   gh release create addon-codemux.project-brief-v1.0.0 \
     "$dir/codemux.project-brief-1.0.0.cmxaddon" "$dir/provenance.json" "$dir/SHA256SUMS" \
     --target "$commit" --latest=false --title "Project Brief 1.0.0" \
     --notes "Reviewed feature add-on package. Not a desktop release."
   gh api repos/Zeus-Deus/codemux/releases/latest --jq .tag_name
   ```

   Verify a downloaded release with `sha256sum --check --ignore-missing SHA256SUMS`.
3. Publishing the SDK/CLI to npm is a separate authorized operation; before
   that, authors install the exact tarballs as documented in their READMEs.
   Publish the reviewed tarballs, never a fresh pack, SDK first because the CLI
   starter depends on it, then read the registry bytes back:

   ```sh
   dir=/absolute/path/to/reviewed-release-directory
   (cd "$dir" && sha256sum --check SHA256SUMS)
   npm publish "$dir/codemux-plugin-sdk-1.0.0.tgz" --access public --ignore-scripts
   npm publish "$dir/codemux-plugin-cli-1.0.0.tgz" --access public --ignore-scripts
   cd "$(mktemp -d)"
   npm pack @codemux/plugin-sdk@1.0.0 @codemux/plugin-cli@1.0.0
   sha256sum --check --ignore-missing "$dir/SHA256SUMS"
   ```

   `--access public` is required for the first publication of a scoped
   package. A maintainer-machine publication has no npm provenance attestation;
   `--provenance` works only from a configured CI publisher, and none exists here.
4. Submit catalog entries with the actual public release asset URLs, exact source
   commit, hashes, sizes and normalized capabilities. Follow
   [catalog review](../../catalog/addons/README.md). The validator downloads the
   assets and inspects them inertly; it does not grant publisher approval.
5. After the reviewed catalog PR merges, the **Reviewed add-on catalog** workflow
   validates `main` but does not publish. To publish, run that workflow from
   `main` with **Run workflow**, entering the catalog revision. Configure the
   `addon-catalog` environment with required reviewers so that a dispatch also
   needs approval. The job checks the catalog against the newest published
   revision, creates the immutable `addons-catalog-r<revision>` release with
   `--latest=false`. It refuses to start unless Latest is a desktop `v*`
   release, and fails, restoring the previous Latest, if publishing moved Latest
   to anything but a desktop release; a desktop release published at the same
   time keeps it. In `codemux-sitev2`, run
   `node scripts/pin-addon-catalog.mjs <artifact-url> <reviewed-sha256>`, run affected
   catalog tests and the site build, and review the pin change.
6. Verify that the website's download and copied install link resolve to the same
   release digest in native Settings review. Complete the final release ledger.

## Desktop releases

A desktop `v*` release verifies its own installers. The Release workflow builds
each platform once through `scripts/addons/verified-tauri-build.mjs`, which runs
the packaged add-on runtime gate against those exact deb/rpm/AppImage or NSIS
files before tauri-action creates the release or uploads them. Before tagging,
confirm that **Packaged add-on runtime** passed for the release commit; it runs
for pull requests touching add-on and core integration files, and can be
dispatched on `main` otherwise.

No workflow here uploads npm packages or publishes feature releases automatically.
The catalog publisher runs only on an approved dispatch from `main`.
Keep the catalog empty until real reviewed release assets exist; do not create
placeholder download URLs or claim author-provided metadata proves ownership.
