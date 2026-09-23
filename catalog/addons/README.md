# CodeMux feature add-on catalog

Submit entries in `entries/<publisher.plugin>.json` through a pull request to this
public repository. The catalog is curated metadata; validation never loads a
plugin's JavaScript or runs package scripts. Themes and footer presets are separate.

Each entry follows `schema/catalog-v1.json`'s `Plugin` definition. Releases must use
a public GitHub Release asset in the declared repository, an immutable stable
semver, a full source commit, the exact SHA-256 and compressed byte count, API and
platform support, license, and normalized manifest capabilities. Sort permissions,
HTTP origins/methods, and credential IDs. The release tag must resolve to its
declared source commit. Publisher/source ownership and the official/community tier
are assigned by maintainers, not by a package manifest.

Before approving an entry or update, maintainers review the source commit and diff,
dependency changes, permission changes, publisher's control of the source repo,
release build provenance, and license. Automated checks verify the public source
commit and tag, download and hash the exact asset, and run the same inert archive
validator as the desktop. No raw branch installs or build-on-install are supported.
Record that review in the PR; an automated green check is not publisher approval.

Run:

```sh
cargo run -j 2 --locked --manifest-path src-tauri/addon-catalog/Cargo.toml -- generate catalog/addons/catalog-v1.json catalog/addons/entries
cargo run -j 2 --locked --manifest-path src-tauri/addon-catalog/Cargo.toml -- online catalog/addons/catalog-v1.json [previous-catalog-v1.json]
```

`online` downloads and hashes every unblocked release. With a previous catalog it
resolves tag provenance only for releases added since then, because accepted
releases are immutable. Set `GITHUB_TOKEN` (or `GH_TOKEN`) to authenticate those
GitHub API requests and avoid the anonymous rate limit; the token is sent only to
`api.github.com`, never with release downloads.

Every shipped v1 desktop parses `catalog-v1.json` strictly and rejects the whole
file, including its blocklist, if it contains an unknown field, platform,
permission, HTTP method, credential type, or tier. `catalog-v1.json` must
therefore stay readable by v1 desktops. Publish releases that need new fields or
values in a new `catalog-vN.json` that newer desktops fetch, and keep publishing
`catalog-v1.json` with its v1-compatible releases and the complete blocklist. The
protocol contract test `catalog_v1_fields_and_values_are_pinned` fails when the
v1 shape changes.

Increment the envelope revision for every change, including revocations. Never
reuse a catalog revision or release version with changed bytes. Keep historical
release records so ownership and digest continuity can be checked. To revoke a
package, add a `blocked` entry with either `pluginId` or `sha256`, a bounded reason,
and an RFC3339 date. Desktop refresh disables matching installations; offline
devices cannot learn new revocations until they reconnect. Local imports are also
checked against the last accepted blocklist.

A reviewed merge is validated but never published by itself. A maintainer then
dispatches the catalog workflow on `main` with that revision, through its approved
`addon-catalog` environment, to publish `catalog-v1.json` and its SHA-256 as assets
on the immutable `addons-catalog-r<revision>` release. That release is never marked
Latest, which stays the desktop `v*` release. The website pins that artifact
and digest in a separate reviewed change; it never reads a mutable branch during
page requests. New desktop installs and updates require a successful online refresh.

The initial catalog is intentionally empty. Project Brief and Issue Companion are
independent example packages in `examples/addons/`; add entries only after their
source commits and real release assets exist and the acceptance gates pass. Do not
invent release URLs or label locally built packages as reviewed catalog releases.

Report a listing or security concern through the repository's issue/security links.
Catalog review is a contribution review signal, not a guarantee against defects.
