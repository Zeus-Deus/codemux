# Plugin platform implementation ledger

Requirements: [engineering specification](BUILD-SPEC.md), revision 2, all 14 chapters.
Desktop integration baseline: `09966161` after the isolated ordered rebase
(original baseline `797a834c`). Website baseline: `27cfa19` on the existing
`feat/site-revamp` branch (includes the latest main). Both use isolated worktrees;
pre-existing Hermes work and website changes are preserved.

**Unreleased implementation. The acceptance matrix is not complete.** Passing
standalone host tests, browser mocks, or the research probe is not release approval.
The [acceptance evidence map](ACCEPTANCE.md) connects each matrix row to its
checks and outstanding installed-app evidence.

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

- Rebuilt Windows `312321e3` passes the [entire expanded native GUI flow](https://github.com/Zeus-Deus/codemux/actions/runs/35397531415),
  including active-host update, expanded-access review/cancellation and rollback
  with matching private data. This repeats the formerly failing old-installer
  case using the backpressure-fixed SDK packages. [Native evidence](evidence/native-ui-windows-updates-312321e3.json)
  records installer SHA-256 `e6342ccb5010295443ff4a2d262b202013000ffa2cb2f8423059d50d22cad3b9`.
  [Windows bundle evidence](evidence/packaged-windows-312321e3.json) records exact
  host payload identity and all five hostile deadlines. The native Strawberry
  Perl selection is now verified by an actual successful MSVC installer build,
  not merely its preflight. The original run's Linux UI step used the earlier
  removal-dialog harness and failed; its saved installer subsequently passes the
  expanded harness as recorded above.

- [Full desktop CI at `1e628eec`](https://github.com/Zeus-Deus/codemux/actions/runs/35396377918)
  passes all five jobs, including the corrected Windows real-child
  activation/removal test and explicit SQLite disposal. The subsequent
  credential-recovery UI regression fails before and passes after clearing its
  stale error on successful save. All four Settings tests and TypeScript pass;
  the actual Settings component was checked at localhost with synthetic IPC,
  including visible failure, opt-in session recovery, empty password field and
  removed alert. The temporary fixture, tab and server were removed. Final
  installer assertions additionally require correct virtual-row ARIA positions
  and keyboard access to row 500; those assertions remain pending on final source.

- [Linux native active-update acceptance](https://github.com/Zeus-Deus/codemux/actions/runs/35397049892)
  passes same-access watched-package update while its real panel is active,
  expanded-permission review/cancellation, and Settings rollback restoring the
  previous private checkbox value after it was changed in the updated release.
  Source identity stays fixed; previous/current data generations are checked.
  The test uses the public Developer mode and normal review/rollback controls,
  with the existing single-use chooser seam. [Exact evidence](evidence/native-ui-linux-updates-312321e3.json)
  records all other native gates passing on installer `312321e3` too.
  Windows's older `da835efb` installer failed during rollback with a real
  `UI update queue overflow`, invalidating its palette option. This installer
  and its saved examples predate the SDK backpressure fix; the gate must repeat
  with rebuilt Windows inputs. The harness only reacquires driver-rejected
  stale elements and does not suppress plugin failures.

- Native pane restoration, no empty accessory/footer spacing, credential
  removal/redaction and paired remote denial now all pass on
  [Linux](https://github.com/Zeus-Deus/codemux/actions/runs/35396542696) and
  [Windows](https://github.com/Zeus-Deus/codemux/actions/runs/35396547958).
  Both preserve the same core pane order and activate a core pane after pause
  and removal. The Windows run verifies actual OS credential save and deletion;
  Linux verifies missing-service failure followed by explicit session-only
  fallback. Warning-only driver logging passes the same secret scan.
  [Linux provenance](evidence/native-ui-linux-panes-312321e3.json) and
  [Windows provenance](evidence/native-ui-windows-panes-da835efb.json) retain the
  exact saved installer and harness revisions. Final-source repeat remains.

- `prepare-release.mjs` builds the SDK/CLI tarballs and both example packages
  from a committed export in a fresh directory outside the app checkout.
  The local four-asset preparation at `1e628eec` passes type/build/package checks,
  repeat-pack byte identity and every SHA256SUMS entry. No distribution is
  published. [Release instructions](RELEASING.md) explain the review/provenance,
  normal authorized publication and website catalog pinning sequence.

- The expanded [Linux installed-app run](https://github.com/Zeus-Deus/codemux/actions/runs/35395837249)
  passes on the newer `312321e3` installer, including masked credential entry,
  missing Secret Service with explicit session-only fallback, removal/redaction,
  and an actual paired loopback HTTP/WebSocket client. Core RPC/events remain
  available while every tested `addon_*` command and plugin event is denied.
  [Exact evidence](evidence/native-ui-linux-credentials-312321e3.json) also records
  the successful public HTTPS, five hostile workloads, rendering budget,
  restart, GUI-off and corrupt-registry checks. This is a saved installer, not
  the final source. Windows passed OS credential save/delete and remote checks,
  but verbose WebDriver logging captured its synthetic SendKeys value; the
  harness now uses warning-only diagnostics; its later successful rerun is recorded above.

- Final-source Windows CI at `ff086c77` exposed a private SQLite file handle
  surviving runtime stop while an activation caller retained `Arc<Running>`.
  Removal committed safely but reported pending file cleanup. Stop now explicitly
  closes storage under its operation mutex, and stale broker access fails closed.
  The concurrent native activation/removal regression keeps that stale reference
  alive and requires warning-free deletion. All six real-host tests pass locally;
  Windows verification of this correction is pending.

- The full expanded saved-installer GUI harness passes on
  [Linux](https://github.com/Zeus-Deus/codemux/actions/runs/35393863385) and
  [Windows](https://github.com/Zeus-Deus/codemux/actions/runs/35393865596), including
  successful HTTPS, all five hostile callbacks, keyboard/themes/updater checks,
  restart, GUI-off, removal and corrupt-registry startup. The 500-path rendering
  workload mounts 14 actual rows, refreshes real Git five times while typing,
  and records frame gaps on the specified runner hardware. Linux p95/max are
  16/28 ms (681 samples); Windows 15.7/15.8 ms (344 samples). Both meet the
  recorded shared-runner 100 ms p95 frame-gap ceiling; this is not a universal
  frame-rate guarantee or a substitute for the native validation budget.
  [Linux evidence](evidence/native-ui-linux-render-91bd6a2b.json) and
  [Windows evidence](evidence/native-ui-windows-render-da835efb.json) identify
  their exact source/digest and separate harness revision.

- Accessibility regressions failed before and pass after virtualized list items
  expose total size/absolute position and table row counts/indices include the
  header. TypeScript and all seven renderer tests pass. A temporary localhost
  fixture using the actual trusted renderer and bundled fonts was visually
  inspected; keyboard scrolling reached file 500, with correct absolute ARIA
  positions and bounded DOM rows. The preview and server were removed afterward.

- [Windows native keyboard/theme/updater acceptance](https://github.com/Zeus-Deus/codemux/actions/runs/35392870675)
  passes the full expanded harness on the saved `da835efb` installer: real dialog
  autofocus/Escape/focus restoration, light/dark theme changes, official updater
  checks with no plugins and while paused, plus the previously recorded example,
  fault, restart, GUI-off, removal and corruption cases.
  [Exact evidence](evidence/native-ui-windows-access-da835efb.json) is retained.
  The [Linux retry](https://github.com/Zeus-Deus/codemux/actions/runs/35392866707)
  passes those added checks, restart/removal/corruption and all five faults on
  `91bd6a2b`, but its public GitHub request hit a rate limit. That run is explicitly
  [failed](evidence/native-ui-linux-access-91bd6a2b.json); the earlier same-installer
  run provides the successful real HTTPS/draft evidence.
- Native Linux removal exposed a spurious credential-cleanup warning when an
  optional credential had never been saved. A regression fails before and passes
  after using the existing pre-write credential index for OS cleanup and clearing
  session-only values separately. Actual saved/retired credentials still retain
  retryable deletion tombstones. Five focused credential tests, 41 native add-on
  tests and all six real-host integrations pass; nine environment-dependent tests
  are reported separately from the focused run. The new real-child activation
  race proves removal waits for activation, reaps its child, deletes private state,
  and cannot resurrect the package after restart.

- The [Linux installer run](https://github.com/Zeus-Deus/codemux/actions/runs/35389012944)
  at merge `91bd6a2b` passes both example flows and all five hostile SDK callbacks
  after the acknowledgement/backpressure correction. The healthy Project Brief,
  controlled draft and real terminal remain usable after each fault. Measured
  GUI observations are 819–1029 ms on the recorded 4-vCPU Xeon runner.
  [Provenance](evidence/native-ui-linux-91bd6a2b.json) identifies the exact installer;
  this run predates the restart/corruption extensions and final design-token changes.
- The [Windows classic-interface run](https://github.com/Zeus-Deus/codemux/actions/runs/35391899586)
  additionally verifies that Settings and Project Brief's native Git panel work
  after restarting with chat GUI disabled, no composer/accessory is present,
  Add to draft reports `No chat composer is available`, the core terminal works,
  and enabling chat GUI and restarting restores the composer. It also repeats
  restart, removal and corrupted-plugin-registry startup checks;
  [exact evidence](evidence/native-ui-windows-classic-da835efb.json) is retained.

- The [Windows installed-app run](https://github.com/Zeus-Deus/codemux/actions/runs/35391050285)
  passes all five hostile SDK callback workloads on installer `da835efb`, plus
  restart with identical installation/grant/source/data-generation/settings,
  removal of all packages, and core startup with a deliberately corrupted plugin
  registry. The terminal and controlled draft remain usable with zero plugin
  hosts. No provider CLI is installed in this synthetic VM, so typing/persistence
  checks do not claim successful provider inference. GUI observations were
  707–794 ms including driver/input overhead on the recorded 4-vCPU EPYC runner.
  [Exact provenance and checks](evidence/native-ui-windows-da835efb.json) and
  [corrupt-registry core screenshot](evidence/native-corrupt-registry-windows.png)
  are retained. This saved installer predates SDK acknowledgement backpressure;
  the equivalent Linux five-workload run exposed the queue overflow that fix addresses.
- A real-host broker regression passes all six delayed-effect transitions:
  workspace change, composer close/replacement, disable, removal and pause.
  Forged-generation claims and late claims/results cannot complete the old draft
  operation; contexts and pending effects are disposed. This complements the
  frontend's delayed-claim/latest-draft tests, without claiming a GUI race run.
  A standalone native-process test also passes hostile shutdown cleanup
  (synchronous loop, endless promise jobs and throw), with termination within 2 s.
- After rebasing onto main, Windows packaging selected Git Bash's Perl, whose
  missing OpenSSL modules stopped the MSVC dependency build. Packaging and the
  ordinary desktop release workflow now explicitly select the runner's native
  Strawberry Perl through `OPENSSL_SRC_PERL`; the separate GNU plugin-host
  compiler is unchanged. The native Windows preflight passes in the run above;
  an actual rebuilt installer remains the verification gate.

- Stock installed-app GUI flows pass on [Linux](https://github.com/Zeus-Deus/codemux/actions/runs/35388617257)
  and [Windows](https://github.com/Zeus-Deus/codemux/actions/runs/35387729205),
  using saved installer `603a7352`. Both independently import the example archives,
  configure Issue Companion, render actual Git and public GitHub HTTPS results,
  append to an existing controlled draft without changing persisted messages,
  and verify zero hosts on clean startup, lazy enablement, and pause.
  Core Appearance and real terminal commands work while paused and after a
  blocking plugin fault; the healthy plugin and composer remain usable.
  GUI fault observations including input/driver overhead were 1195.5 ms Linux
  and 643.7 ms Windows. [Linux provenance](evidence/native-ui-linux-603a7352.json),
  [Windows provenance](evidence/native-ui-windows-603a7352.json),
  [native Linux HTTPS/draft screenshot](evidence/native-issue-companion-linux.png).
  These installers have one hostile command and predate subsequent fixes.
  Neither run establishes the final-source five-workload release gate.
- [Linux CI at `278df894`](https://github.com/Zeus-Deus/codemux/actions/runs/35386904270)
  passes actual ENOSPC injection in a disposable 16 MiB tmpfs at package-write,
  package-staged, journal-saved, data-snapshotted, registry-switched and activated.
  Each failed transaction recovers the previous release/grant/private-state tuple.
  The strengthened exact data-generation assertion passes in
  [CI at `9dfda479`](https://github.com/Zeus-Deus/codemux/actions/runs/35390059608),
  which also passes both full frontend and Rust platform suites. The Windows
  same-size Git edit regression now passes with the original index timestamp
  preserved; all five jobs in that full CI run are green.
- The rebase preserves current mobile Settings, responsive panel, remote command
  policy and composer delivery logic. TypeScript and 166 focused integration/UI
  tests pass. Native cargo check and 40 focused tests pass (7 explicit ignored
  environment/integration tests are counted separately). All four real-host
  manager integrations pass with the freshly packed SDK/example archives.
  Current main's design-token contract exposed raw plugin font/radius classes;
  these now use the shared tokens in their original UI/Settings milestones.
  TypeScript and 22 affected/token-contract tests pass, and the localhost mock
  Settings/catalog layout was visually checked with the bundled fonts.
- Real native SDK regression failed before and passes after render backpressure:
  two completed callbacks can wait behind an unacknowledged render without
  exhausting the host queue. Trusted acknowledgements release one ordered batch;
  closing a view discards pending mutations and ignores late acknowledgements.
  Native two-batch, 1000-mutation, traffic and memory bounds remain unchanged.
  The wire permits `ui.ack` only from host to child for the current generation.

- Full desktop [CI at `3045f47f`](https://github.com/Zeus-Deus/codemux/actions/runs/35376157828)
  passes on Linux and Windows, including 5,610 frontend tests on Linux. The same
  revision passes [all installer payload checks](https://github.com/Zeus-Deus/codemux/actions/runs/35376157749):
  Linux deb/AppImage and Windows NSIS. Maximum observed fault latencies are
  1007.0 ms and 1014.7 ms respectively. Recorded [Linux](evidence/packaged-linux-3045.json)
  and [Windows](evidence/packaged-windows-3045.json) results include source-run
  provenance. Commits through `33acb551` subsequently changed the CI harness,
  not production app code. A later renderer correction makes Markdown link
  controls explicitly non-submitting: its containing-form regression failed
  before the fix, then all six renderer tests and TypeScript passed. Installed
  desktop UI acceptance of the final revision remains separate and pending.

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

- Settings configuration controls wait for the stored values before accepting
  edits or submission. A disposed release's late response/error is ignored.
  TypeScript and three focused Settings tests pass for this guard.

- Native Git exclude regression failed before the fix and passes afterward for
  both ordinary and linked worktrees. All seven focused native Git tests and
  `cargo check -j 2` pass. The snapshot copies only bounded regular-file exclude
  data; repository programs remain disabled.
- The expanded public-SDK hostile fixture builds and packs independently. Its
  five commands terminate real local hosts in 1.7–253.0 ms (blocking loop, throw,
  endless promises, recursion, allocation). These standalone timings do not
  establish installed-app interactivity.

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

- Eleven lifecycle tests and `cargo check` pass with a new ignored real-ENOSPC
  test. CI mounts a dedicated 16 MiB tmpfs and fills it at six transaction
  boundaries. Both script and test refuse local/persistent runners, and the
  test rejects a non-tmpfs or volume larger than 32 MiB. The actual hosted six-checkpoint run passed at `278df894`, as recorded above;
  the strengthened data-generation assertion is tracked separately.

## Unresolved release gates

- Repeat the expanded installed-app acceptance on both platforms after the final
  storage cleanup, virtual-row accessibility and credential-error changes.
  Saved-installer evidence above records its exact earlier source and cannot
  establish the final binary. Full CI and the independent distribution workflow
  must also pass for the final review head.
- The context/authority matrix uses both real-child native broker cancellation
  and focused controlled-composer tests with injected timing and transport seams
  allowed by chapter 12. Integrated example flows use real stock desktop IPC.
  Delayed effects across every project/thread/surface transition have not all
  been driven through WebDriver; this distinction remains explicit in the
  acceptance map rather than treating mocked effect completion as native UI.
- Publish reviewed independent package assets and the first immutable catalog
  artifact through normal authorized workflows, then pin that artifact in the
  website and verify website-to-desktop digest identity. The prepared release
  distributions and instructions are reviewable; nothing has been published,
  merged or listed using fabricated assets.

See [the acceptance evidence map](ACCEPTANCE.md) and [release procedure](RELEASING.md).
No milestone with an unresolved exit gate is represented as complete.
