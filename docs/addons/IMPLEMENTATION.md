# Plugin platform implementation ledger

Requirements: [engineering specification](BUILD-SPEC.md), revision 2, all 14 chapters.
Desktop baseline: `797a834c`. Website baseline: `27cfa19` on the existing
`feat/site-revamp` branch (includes the latest main). Both use isolated worktrees;
pre-existing Hermes work and website changes are preserved.

**Unreleased implementation. The acceptance matrix is not complete.** Passing
standalone host tests, browser mocks, or the research probe is not release approval.

## Ordered delivery

| PR | Scope | State |
| --- | --- | --- |
| 1 | Contracts, independent QuickJS host, public SDK and CLI | Draft [#390](https://github.com/Zeus-Deus/codemux/pull/390); Linux and Windows GNU CI passed |
| 2 | Native manager, scoped broker, private persistence, installer foundations | Draft [#392](https://github.com/Zeus-Deus/codemux/pull/392) |
| 3 | Trusted UI, palette, panel deck, controlled composer | Draft [#393](https://github.com/Zeus-Deus/codemux/pull/393) |
| 4 | Settings, lifecycle/recovery, review, developer watch | Draft [#394](https://github.com/Zeus-Deus/codemux/pull/394) |
| 5 | Independent examples and native resource packaging | Draft [#395](https://github.com/Zeus-Deus/codemux/pull/395); packaged gates pending |
| 6 | Reviewed catalog schema, validator, immutable artifact publication | Draft [#396](https://github.com/Zeus-Deus/codemux/pull/396); empty seed, no published example releases |
| 7 | Website pinned catalog, search/detail/handoff, author documentation | Draft [website #7](https://github.com/Zeus-Deus/codemux-sitev2/pull/7); build and affected tests pass |
| 8 | Release hardening and complete acceptance evidence | Draft [#397](https://github.com/Zeus-Deus/codemux/pull/397); see unresolved gates below |

Later PRs stack on their specified prerequisites. Full desktop CI was enabled
for the stack in PR 8; earlier draft heads do not each have full desktop CI
evidence and must be revalidated as they are prepared for merging. The native
catalog reader and transactional installer are included in the manager foundation because its grant,
activation, removal and recovery paths must share one authority boundary. Catalog
publication and Settings remain separate deliverables.

## Verified evidence (Linux x86_64 unless stated otherwise)

- PR 1 hosted CI: Linux and Windows GNU host, manifest contracts, SDK callback
  integration all passed. Schema comparison normalizes Windows CRLF only.
- Protocol: 14 focused tests pass, including atomic malformed UI rejection,
  callback disposal, mutation/tree limits, catalog ownership/release history and
  strict manifests. Raw archive validation covers traversal, absolute/Windows/UNC
  paths, alternate data streams, case collisions, links and special entries.
- Independent host: 3 real-process tests pass, covering absent ambient authority,
  parent EOF, stale/oversized IPC and hostile synchronous/asynchronous workloads.
- SDK native integration passes: Preact view, batched Remote DOM updates,
  callback, scoped request, property removal and unmount.
- Desktop manager: 39 focused native tests pass, including repository filter
  isolation, dropped/unresponsive child supervision, real SQLite page-limit
  failure, and recovery after interruption at eight durable update transitions.
  Git cancellation also owns and reaps the child before releasing its permit,
  and metadata snapshots reject nonregular files and symlinks.
  The interruption matrix preserves the old tuple before completion and the new
  tuple after the atomic completion marker; matching private data is checked.
  Uninstall waits for an update at three transaction stages, then removes the
  committed candidate and private state; restart does not restore it.
- Four explicitly enabled real-child tests passed after the final changes:
  broker scope/disposal, Project Brief installer/Git/view/composer request, and
  Issue Companion installer/view/recorded HTTP states/explicit draft action,
  and quarantine of five hostile workloads plus unexpected child exit while a
  second native plugin remains usable. Each failing generation is reaped within
  two seconds, its context is revoked, and implicit restart is denied.
  The example tests supply the frontend effect result; they are not stock-app GUI E2E.
- Frontend TypeScript passes. 87 affected tests passed, plus 10 new tests for
  delayed effects during disable/remove/pause/workspace/thread/failure, stale
  inventory responses, update/rollback remounting and explicit retry. Composer
  tests include preservation of user input. Six repository theme/UUID checks and
  Settings dialog Escape/focus and late composer-registration regression tests
  also pass. Panels react to replacement or ambiguity in their workspace
  composer registry. No theme or footer customization subsystem was repurposed.
- Both examples build/check/pack with packed SDK/CLI tarballs. Issue Companion
  also builds/checks/packs outside the app checkout using those packages only.
- Actual Linux Secret Service round-trip and deletion passed using a synthetic
  token in a random installation namespace. A public GitHub /zen request through
  the production HTTP broker passed with pinned DNS/TLS and no credential/data.
- Five native loopback TLS tests pass through the real HTTP client: pinned
  hostname/credential request, trust and hostname rejection, mixed/private DNS
  rejection, redirect/encoding/declared/streamed body limits, timeout/cancellation,
  concurrency release and private rolling quotas. The local fixture CA is trusted
  only by the test's client; no system trust or real credentials are used.
- Hosted native desktop jobs passed on Linux and Windows, including Windows native OS
  keyring and the installed independent packages. Initial frontend CI identified
  semantic-color and UUID-helper violations; both are corrected and checked.
  The next run exposed Vitest discovering a Node-only ELF test: it is now named
  outside Vitest's discovery pattern and still runs explicitly with Node. The
  native TLS/recovery tests passed on Windows at `5a643feb`.
- Windows NSIS installation, exact bundled-host digest, clean-environment SDK
  callbacks and five hostile workloads passed in [packaged CI](https://github.com/Zeus-Deus/codemux/actions/runs/35371843758).
  Maximum measured fault latency was 1016.5 ms on the 4-vCPU AMD EPYC runner.
  See [Windows payload evidence](evidence/packaged-windows.json), collected from
  commit `5a643feb`; this does not establish desktop GUI behavior.
- Linux deb and AppImage payloads passed at `5a643feb` in
  [packaged CI](https://github.com/Zeus-Deus/codemux/actions/runs/35371843758):
  exact deb digest, AppImage ELF provenance, SDK callbacks and five hostile
  workloads in each. Maximum fault latency was 1005.4 ms on the 4-vCPU AMD EPYC
  runner. See [Linux payload evidence](evidence/packaged-linux.json).
  This resolves the linuxdeploy RPATH check failure; it is not desktop GUI E2E.
- Standalone release-host timing: 20 samples each of five hostile workloads;
  maximum 1010.4 ms, activation-loop p95 1007.2 ms on Ryzen 5 7600 / Linux.
  See [machine-readable evidence](evidence/host-timing-linux.json). These are
  standalone activation measurements, not GUI or packaged-platform evidence.
- Website: 5 catalog tests and production build pass. The existing waitlist
  module needs a Resend key at build time; verification supplied a synthetic
  non-secret placeholder, without calling its email route.
- Browser evidence: Settings empty/paused/offline states; desktop catalog;
  mobile catalog at 390px without horizontal overflow; synthetic detail fixture
  and exact-version install-link UI. Add-on Settings works at 800×600 in light
  and dark themes, remains accessible with chat GUI off, and preserves keyboard
  focus when Escape closes its install dialog. These remain browser-preview
  checks, not native screen-reader/desktop E2E. Fixtures are removed from the seed.

## Evidence-backed implementation clarifications

- The sample uses `includeFiles`, but chapter 5 requires
  `[a-z][a-z0-9-]{0,39}`. Packages use `include-files`; validation enforces the
  stated grammar.
- Hermes contribution ownership, scoped registration and host cleanup informed
  this implementation. Its main-renderer code loader is not reused.
- The permitted 5 MiB JS bundle is initialized in bounded source chunks, preserving
  the separate 1 MiB IPC-frame ceiling and the absence of JS filesystem access.
- Remote DOM emits an empty string when Preact removes a custom-element prop.
  Native package testing exposed this for non-string props; the SDK normalizes
  known removals to null and batches mutations before sending them. The native
  validator still rejects unknown props, including unknown null-valued props.
- Plugin-provided log text may contain credential-derived response bodies.
  Diagnostics record log byte counts rather than persisting that untrusted text;
  native structured errors and failure state remain available.
- Catalog history cannot delete accepted ownership/release records: otherwise a
  later revision could reassign them without detection. Withdrawals use blocked
  entries while retaining their history. No mutable release replacement is allowed.
- Rollback copies the recorded private data snapshot into a fresh writable
  generation, probes, switches the tuple, and normally activates through a
  durable journal. Failed activation preserves the original rollback snapshot.
- OS keyring operations keep their serialization lock inside the blocking task;
  cancelling IPC cannot let an uninstall race ahead of an unfinished OS write.
  Keys bind credential ID to exact origin, so an update cannot redirect a saved
  bearer token. Uninstall queues all indexed historical keys, including credentials
  removed from later manifests; locked-service cleanup remains retryable.
- A full 2 s watchdog left no time for reaping. The response/queue watchdog now
  fires at 1.5 s, reserving cleanup within the 2 s ceiling. A cancelled instance
  remains registered if the OS delays termination; no new generation can overlap.
  The final host handle owns a cancellation lease, including failed initialization.
- A native fixture proved that ordinary `git status` executes repository clean
  filters despite disabled hooks/fsmonitor. The bounded runner now discovers inert
  metadata, snapshots the index/refs into a private Git directory, and copies only
  safe status/tracking configuration. No filter, hook, include, remote URL, promisor,
  or external helper configuration reaches the status child. The whole operation
  retains a 5 s deadline; index snapshots are bounded to 128 MiB. Linked/split-index,
  detached and unborn repositories are covered. A workspace below the repository
  root is rejected rather than expanding authority outside its authorized root.
  Status compares file bytes without custom filter transformations; repositories
  whose filters transform content can therefore report different changed files.
- linuxdeploy rewrites an AppImage host's ELF RPATH, so its packaged bytes cannot
  equal the staged release digest. AppImages instead require identical program
  sections and symbol/dynamic-link semantics, allowing only the `$ORIGIN` RPATH
  relocation. Tests accept a real patchelf rewrite and reject changed program
  data or added dependencies. Deb and NSIS retain exact SHA-256 comparison.
- A dropped Git broker future cancels a separately owned job; its concurrency
  permit remains held until the child is killed and reaped. Unix metadata opens
  reject symlinks and use nonblocking mode to prevent a swapped FIFO from
  stranding the filesystem worker.
- Release-mode validation of 1,000 text mutations in a valid 1,999-node,
  222,790-byte tree measured 302–499 ms on this Linux Ryzen 5 7600 host.
  Revalidating and serializing the entire tree after each content change made
  the advertised batch ceiling too expensive. Content updates now account for
  exact serialized-size deltas and validate only the changed value. Structural
  and callback changes still validate the whole candidate. A 50 ms wall-clock
  validation budget rejects an expensive batch atomically and quarantines the
  generation through the existing resource-failure path. This extra bound is
  justified by the measured native CPU cost; it does not replace desktop frame
  timing or claim a universal latency on all machines. The same workload now
  measures 2.5–3.9 ms; [before/after samples](evidence/ui-validation-linux.json)
  and the reproducible protocol example are included.
- Frontend stop actions fence effects synchronously until inventory refresh
  completes. Older inventory responses cannot restore stale enabled state.
  Native generation changes remount views; failed releases still need explicit Retry.

## Unresolved release gates

- Stock built-app E2E for both examples, including real frontend/native IPC,
  actual controlled draft insertion and core UI operation during hostile plugins.
- Latest-commit packaged rerun after the final integration follow-ups. Linux
  AppImage/deb and Windows NSIS payload behavior passed on recorded revisions;
  stock desktop GUI launch and integration still require separate evidence.
- Complete failure-injection matrix: disk-full boundaries, every journal/crash
  point, unexpected child exit/ignored shutdown, concurrent update/uninstall,
  and delayed workspace/thread/composer races through the actual desktop. Native
  update/uninstall serialization and dropped Git requests are covered, but those
  do not establish all installed-app race cases.
- Latest-commit hosted native checks after the cancellation and concurrency
  follow-ups. Native TLS, recovery and OS credential checks have passed on both
  platforms; installed-app cancellation races and external network behavior
  remain part of desktop E2E.
- Complete keyboard/screen-reader, light/dark, small-window, chat-GUI-off and
  core pane restoration evidence; measured runtime fault and UI budgets with
  hardware/workload details.
- Publish reviewed independent package releases and the first immutable catalog
  artifact through the normal release workflow; then pin that published artifact
  in the website and verify website-to-desktop digest identity. The current seed
  deliberately contains no fabricated release URLs or installable listings.

No milestone with an unresolved exit gate is represented as complete.
