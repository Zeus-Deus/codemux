# Source Control Providers

- Purpose: Describe how Codemux decides which git *hosting* product serves a checkout, and how it talks to it.
- Audience: Anyone working on pull/merge request, issue, checks, or review integration.
- Authority: Canonical feature-level reality doc for the provider seam.
- Update when: Detection policy, adapters, capabilities, auth model, or the settings surface change.
- Read next: `docs/features/review-integration.md`, `docs/features/github-issues.md`, `docs/features/settings.md`

## What This Feature Is

Codemux's hosting integration was GitHub-only in two places at once: every call shelled out to `gh`, and the gate for "may we show change-request UI here?" was a substring match on `github.com` in `git remote -v`. This subsystem replaces both halves with a seam — offline detection of *which* product a checkout is hosted on, and a trait + registry that maps that answer to an implementation. GitHub and GitLab have adapters today. Bitbucket and Azure DevOps are recognised by name but not served.

The AI-agent "provider" (Claude / Codex / OpenCode) is a different axis entirely; this doc only covers hosting products. Components that hold both call this one `scProvider`.

## Current Model

### Detection (`src-tauri/src/git_provider/detect.rs`)

Detection is deliberately offline — `git remote -v` plus, when there is more than one remote, `git rev-parse --abbrev-ref @{upstream}`. No CLI, no auth, no network, so it can sit on the 5s active-workspace poller.

- **Remote parsing** handles `scheme://[user[:pass]@]host[:port]/path`, scp-style `[user@]host:path`, and bracketed IPv6. Filesystem remotes (`file://`, `/srv/repo.git`, `../sibling`) yield no host. The fetch URL wins over the push URL when they differ. `host` is a bare lowercased hostname with userinfo and port stripped — so nothing that reaches an error message can carry a token. `base_url` keeps a port only when an http(s) remote named one; an SSH port is dropped, because `ssh://…:2222` says nothing about where the web UI listens.
- **Classification order is load-bearing**: an explicit `source_control.custom_hosts` mapping from synced settings always wins; then exact well-known hosts (`github.com`, `gitlab.com`, `bitbucket.org`, `dev.azure.com`, and their `www.`/`ssh.`/`altssh.` forms); then `*.dev.azure.com` / `*.visualstudio.com`, then loose `contains("github")` / `contains("gitlab")` / `contains("bitbucket")` substring rules that cover the usual self-hosted naming (`gitlab.acme.com`).
- **Multi-remote policy**, stated once in `select_remote`: a classifiable remote always beats an unclassifiable one (the gate this replaced matched *any* remote, so a checkout whose `origin` is an unrecognised mirror and whose second remote is on github.com must keep lighting up). Within each group the order is upstream-tracked remote → `origin` → first listed. With nothing classifiable the tail is `origin` then the first remote, so `host`/`base_url` are still populated for the UI.
- **`ProviderKind`** is `github` / `gitlab` / `bitbucket` / `azure_devops` / `unknown`, serialized as flat snake_case. `Unknown` is a normal state — no remote, a local-only checkout, or a host nothing recognises — never an error.
- **Caching**: 60s TTL keyed by repo path, **success only**. An `Unknown` is re-probed every call, so `git remote add origin …` or a newly saved custom-host mapping takes effect on the next render rather than a minute later. `settings_sync::save_cache` clears the whole detection cache when the host mapping changed.

### The provider seam (`provider.rs`, `registry.rs`, `unsupported.rs`)

`SourceControlProvider` is exactly the operation set the review panel, the composer pickers, the workspace pollers and the worktree path already called on `crate::github` — pull requests (branch lookup, list, incoming, detail, create, merge, diff, checks, review threads, inline comments, submit review, deployments), issues (list, get, get-fresh), and one git-level hook (`fork_pr_fetch_refspec`). Signatures reuse the structs the UI already speaks (`PullRequestInfo`, `GitHubIssue`, `CheckInfo`, …) rather than a parallel type hierarchy: a second product populates the same shapes. Every method is synchronous because every implementation shells out; callers wrap in `spawn_blocking`.

The registry has two entry points, because the two families of caller want different things from an unclassifiable host:

- `provider_for_path` (**strict**) — for gates and pollers. Only a positively identified, implemented product gets a real provider. `repo_has_supported_provider` is the direct replacement for `github::is_github_repo` and backs the `check_github_repo` command.
- `provider_for_path_or_default` (**advisory**) — for the GitHub-named command surface. An unclassified host still reaches the GitHub adapter, because self-hosted GitHub Enterprise on a bespoke domain has always worked through those commands (`gh` resolves it from its own hosts config) and detection must not take that away. Only a repository identified as a *different* known product gets the null object.

`UnsupportedProvider` is a null object rather than an `Option`, so no caller branches on "is there a provider". `is_implemented()` is false so gate-shaped callers skip the work entirely, and every method fails with one sanitized sentence naming the product and bare host.

### Adapters

- **GitHub** (`github.rs`) — deliberately logic-free delegation to `crate::github` / `crate::github_cache`, including the choice of cached vs uncached wrapper, so routing a call site through the trait cannot change what GitHub users observe. Fork refspec `pull/<n>/head:<branch>`.
- **GitLab** (`gitlab.rs`) — owns its own logic; there was no pre-existing GitLab code to forward to. Reads go through `glab api` rather than the porcelain: `--output json` is glab's own Go struct (which has changed shape between releases) while `glab api` returns the documented REST payload, and discussions, pipelines and per-file diffs have no porcelain equivalent. `glab api` still resolves the host, the token and the project from the checkout's remote, which is the whole reason to shell out instead of speaking HTTP.

  Normalization onto the shared structs: merge-request `iid` (not the instance-wide `id`) becomes `number`, because the iid is what the web UI, the `refs/merge-requests/<n>/head` ref, and every `!123` reference mean; state maps onto gh's uppercase `OPEN`/`MERGED`/`CLOSED` with draft carried on `is_draft`; pipeline jobs map onto the same `pass`/`fail`/`pending`/`skipping`/`cancel` check buckets the icons already understand, and a pipeline with no jobs (an externally reported one) falls back to the commit statuses on its sha; discussions are split by the presence of a `position` into conversation threads and diff-anchored inline comments, with the first note's id standing in for GitHub's review id so the review tab's grouping still works; system notes and diff-anchored notes are dropped from the comment list. Branch selection applies the same open-beats-historical, newest-updated-wins preference as `github::select_branch_pr`. Fork refspec `refs/merge-requests/<n>/head:<branch>` — unlike GitHub's it needs the `refs/` prefix, because `merge-requests/…` is not one of the prefixes git's ref disambiguation searches.

  Behavioral notes: `create_pull_request` has no draft flag on GitLab, so a `Draft:` title prefix is used; `merge_pull_request` passes `--auto-merge=false` (glab defaults it on, which would queue behind the pipeline instead of merging) plus `--remove-source-branch`; `submit_pull_request_review` refuses `request-changes` outright rather than silently downgrading it to a comment, and refuses an empty non-approval body; `workspace_pull_request` has no side-branch reflog fallback, because that exists to work around `gh pr list --head` not matching fork branches and the GitLab branch query is already project-scoped and exact.

- **Shared plumbing.** `exec.rs` is one deadline-bounded subprocess runner used by every CLI shell-out, including `gh`'s. It drains both pipes on their own threads for the whole life of the child — the naive poll-then-read shape deadlocks once output exceeds a pipe buffer (a large diff), reporting a timeout that never happened. `cache.rs` is a generic TTL cache primitive re-exporting `github_cache`'s `LIST_TTL`/`DETAIL_TTL` so the two families cannot drift; keys are path-first so one repository's entries can be dropped without disturbing another's, and errors are never cached.

### Auth model

Codemux stores no hosting credentials. Each product is driven through its own vendor CLI using the credentials that tool already holds — `gh` for GitHub, `glab` for GitLab. Both are run with the keep-DBus environment sanitiser, because both pull their token from the secret-service keyring on Linux and a nulled `DBUS_SESSION_BUS_ADDRESS` makes a logged-in user read as logged out.

Auth is probed **per instance**, not globally. `glab auth status` is scoped with `--hostname host[:port]` (the port comes back from `base_url`, since a self-hosted instance on a non-default port is a separate entry in glab's hosts file); unscoped, its exit status is an AND across every configured instance, so one stale `gitlab.com` entry would report a working self-hosted login as failed. The printed report is parsed as a tri-state — "logged out" must be distinguishable from "the output did not look like anything we recognise", or an upstream cosmetic change would silently log everyone out. Ready verdicts are memoized per instance for a short TTL, **success only**, so a signed-out user always sees the actionable login command.

### Commands and the workspace snapshot

- `discover_source_control` → `Vec<ProviderDiagnostic>` — probes every named product for the settings pane. Infallible by construction (no `Err` arm); each row is built independently on its own blocking task, and a failed probe degrades to a well-formed "not installed" row. Carries `cli_installed`, first line of `<cli> --version` (control characters stripped, 120 chars max), `authenticated`, `account`, one sanitized `detail` sentence, and the adapter's declared `capabilities`. No token is ever read.
- `check_provider_auth(path)` → `ProviderAuthStatus` — host-scoped readiness for one checkout, strict resolution. Infallible: a wedged probe answers "nothing usable" rather than rejecting. The frontend wrapper `src/lib/provider-auth.ts` adds a 60s cache keyed by `path|kind`, storing usable verdicts only so a `glab auth login` shows up on the next render.
- Workspace snapshot gained **`provider_kind`** (`Option<String>`, serde-defaulted, snapshot-local, never synced). Re-derived by both pollers alongside the PR pill via `update_workspace_provider_kind`, which is kept separate from `update_workspace_pr_info` because the provider is known even on ticks where the PR lookup fails or is skipped. `Unknown` maps to `null` rather than the string `"unknown"`.
- The existing GitHub-named Tauri commands keep their names (they are the stable boundary) but route through the registry. `check_github_repo` now answers "does an implemented integration exist here?".

### Pollers

Both pollers stamp `provider_kind`, then resolve the provider and skip the workspace when it is unimplemented or its CLI is missing. The 60s PR poller's auth gate is keyed by `product|host` and memoized for the duration of one tick: with more than one product implemented, a logged-out GitHub would otherwise stall the GitLab workspaces too, and two self-hosted instances have independent logins. A timeout is a this-tick "unavailable" and is not remembered as a verdict; transitions are logged, steady state is not.

### Frontend presentation (`src/lib/source-control.ts`)

One table turns `provider_kind` into copy — product name, `PR`/`MR` short noun, `pull request`/`merge request` prose and title forms, the `#`/`!` reference sigil, CLI binary and label, login command, install URL, canonical hosts, icon, and whether Codemux supports it. Two rules keep the sweep safe:

- **Absent means GitHub.** Snapshots written before detection existed, and hosts detection cannot classify, have no `provider_kind`, so `resolveProvider(undefined)` is the GitHub row and every pre-existing GitHub surface renders byte-identical copy.
- **Unknown is neutral, not wrong.** A `provider_kind` that is *present* but has no row in this build gets generic wording ("change request", `CR`) rather than borrowed GitHub nouns — that copy only ever surfaces in unsupported states.

Issue references stay `#` on every product (GitLab uses `#` for issues too); only change-request references switch sigil.

The composer's hosting attach rows (`GitHub Issue…` / `GitHub PR…` and their per-product equivalents) gate on all three answers of the preflight — a served host, a CLI on PATH, and a signed-in CLI — because a picker backed by a missing binary can only error. The chat panes therefore hand `providerAuthenticated={false}` for a CLI that is not installed and carry the difference in `providerCliInstalled`, which flips the disabled-row hint from the login command to the download.

Provider-aware surfaces: the review panel (empty state, create form, all three status messages), the incoming list (header, row refs, "View all on <product>" with a provider-aware list-URL rewrite), the Context Row PR chip, sidebar inbox cards / settled rows / workspace rows (chip text and aria/tooltip copy), the workspace hover card (label, `MR branch` row, and a new **Hosting** row rendered only when `provider_kind` is set), the composer attach popup and `@issue:`/`@pr:` mention footers, the issue/PR pickers and issue detail popover, and the new-workspace dialog (its injected issue-context prompt now names the detected product).

### Settings → Source Control

`src/components/settings/source-control-section.tsx`, with `SubsectionHeader` extracted into `settings-primitives.tsx` so it can be shared with `settings-view.tsx`.

- **Providers** — one diagnostics row per product with a status dot (ready / attention / inert), status label, CLI version, click-to-reveal masked account, a "Serves …" line read off the backend's declared capabilities, and one actionable fix line (install beats sign-in). Products with no adapter are listed dimmed rather than omitted, so a user is not left wondering whether detection failed or the product is unsupported. A **Rescan** button drives every subsequent probe; the mount probe is the only automatic one.
- **Self-hosted servers** — the `source_control.custom_hosts` editor. Input is normalised to a bare hostname (a pasted clone URL, `user@host`, trailing slash, port and path are all accepted), and the product choices are the ones with adapters.

## What Works Today

- Offline provider detection from remotes, with an explicit self-hosted host mapping that outranks every heuristic
- GitHub served exactly as before, through the trait
- GitLab served through `glab`: branch/workspace MR lookup, MR list, incoming MRs, MR detail with description/comments/review decision/diff stats, create, merge (squash / rebase / merge commit), name-only and full diffs, pipeline checks, discussion threads and inline comments, approve + comment, issue list/detail, and the `refs/merge-requests/<n>/head` fork fetch
- Per-instance auth probes so two self-hosted deployments of the same product do not share a verdict
- Settings → Source Control diagnostics and self-hosted host mapping, synced across devices
- Provider-aware PR/MR nouns, sigils, CLI names and login commands across every hosting surface
- Clear, sanitized failure copy for a recognised-but-unserved product instead of a confusing CLI error

## Current Constraints

- **Bitbucket and Azure DevOps are recognised, not served.** They classify, appear in the settings pane dimmed, and route to the null object; every operation fails with "… is not supported yet".
- **GitLab does not serve deployments** (`has_deployments: false`). GitLab's environments model does not answer "which deployments belong to this merge request" the way the panel asks, so it is not served at all rather than served wrongly. GitLab also has no request-changes verb — that review event is refused, not downgraded.
- GitLab has no side-branch reflog fallback for the sidebar badge; that is a GitHub-path behavior.
- Detection classifies by hostname, so a self-hosted instance on a neutral domain needs a Settings → Source Control mapping before anything works. Conversely, a host containing `github` / `gitlab` / `bitbucket` is classified as that product — self-hosted GitHub Enterprise on `github.acme.internal` now classifies as GitHub, where the old `is_github_repo` gate answered false.
- Custom-host mappings are hostname-only: no wildcards, no per-path scoping.
- The Tauri command surface is still GitHub-named; only the implementations are provider-neutral.
- `Unknown` detections are never cached, so a checkout with no remote pays a `git remote -v` per call.

## The live GitLab test

`src-tauri/tests/gitlab_live.rs` is an `#[ignore]`d round trip against a real instance — it needs a reachable GitLab, an authenticated `glab`, and it creates a project:

```text
CODEMUX_GITLAB_TEST_TOKEN=<personal access token with `api` scope> \
  cargo test --manifest-path src-tauri/Cargo.toml \
  --test gitlab_live -- --ignored --nocapture
```

`CODEMUX_GITLAB_TEST_HOST` overrides the default `localhost:8929`. The token is used for the git push only — every adapter call authenticates through `glab`'s own stored credentials, which is the code path that matters. The test installs a `source_control.custom_hosts` entry for the instance (the only way `localhost` can classify), invalidates the detection cache, and restores the previous settings blob on drop so a real install is not disturbed. Everything it creates is left behind; the expectation is a disposable instance.

Everything else is covered by ordinary unit tests: remote parsing and the multi-remote policy, the registry's strict/advisory split, the subprocess runner's pipe-buffer and deadline behavior, glab auth-status parsing across multiple hosts, JSON mapping onto the shared structs, the diagnostics command's shape, and the frontend presentation map.

## Important Touch Points

- `src-tauri/src/git_provider/detect.rs` — classification, multi-remote policy, detection cache
- `src-tauri/src/git_provider/provider.rs` — the `SourceControlProvider` trait and `Capabilities`
- `src-tauri/src/git_provider/registry.rs` — strict vs advisory resolution, `provider_kind_field`
- `src-tauri/src/git_provider/github.rs` / `gitlab.rs` / `unsupported.rs` — adapters and null object
- `src-tauri/src/git_provider/exec.rs` — the shared timed subprocess runner
- `src-tauri/src/git_provider/cache.rs` — TTL cache primitive
- `src-tauri/src/commands/source_control.rs` — `discover_source_control`, `check_provider_auth`
- `src-tauri/src/settings_sync.rs` — `SourceControlSettings.custom_hosts`
- `src-tauri/src/lib.rs` — both workspace pollers
- `src/lib/source-control.ts` — provider presentation map
- `src/lib/provider-auth.ts` — cached host-scoped auth gate
- `src/components/settings/source-control-section.tsx`, `settings-primitives.tsx` — Settings → Source Control
- `src-tauri/tests/gitlab_live.rs` — the ignored live round trip
