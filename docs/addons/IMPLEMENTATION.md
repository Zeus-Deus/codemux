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
| 8 | Release hardening and complete acceptance evidence | In progress; see unresolved gates below |

Later PRs stack on their specified prerequisites. The native catalog reader and
transactional installer are included in the manager foundation because its grant,
activation, removal and recovery paths must share one authority boundary. Catalog
publication and Settings remain separate deliverables.

## Verified evidence (Linux x86_64 unless stated otherwise)

- PR 1 hosted CI: Linux and Windows GNU host, manifest contracts, SDK callback
  integration all passed. Schema comparison normalizes Windows CRLF only.
- Protocol: 10 focused tests pass, including atomic malformed UI rejection,
  callback disposal, catalog ownership/release history and strict manifests.
- Independent host: 3 real-process tests pass, covering absent ambient authority,
  parent EOF, stale/oversized IPC and hostile synchronous/asynchronous workloads.
- SDK native integration passes: Preact view, batched Remote DOM updates,
  callback, scoped request, property removal and unmount.
- Desktop manager: 24 focused native tests passed after credential origin binding,
  locked-service fixtures, retired-credential cleanup, and real development watch
  review/cancellation. A subsequent real SQLite page-limit test passed, preserving
  the original tuple/state after SQLITE_FULL. Supervision changes await final rerun.
- Three explicitly enabled real-child tests passed again after credential changes:
  broker scope/disposal, Project Brief installer/Git/view/composer request, and
  Issue Companion installer/view/recorded HTTP states/explicit draft action.
  These tests supply the frontend effect result; they are not stock-app GUI E2E.
- Frontend TypeScript passes. 87 affected tests passed, plus 10 new tests for
  delayed effects during disable/remove/pause/workspace/thread/failure, stale
  inventory responses, update/rollback remounting and explicit retry. Composer
  tests include preservation of user input. No theme or footer customization subsystem was repurposed.
- Both examples build/check/pack with packed SDK/CLI tarballs. Issue Companion
  also builds/checks/packs outside the app checkout using those packages only.
- Actual Linux Secret Service round-trip and deletion passed using a synthetic
  token in a random installation namespace. A public GitHub /zen request through
  the production HTTP broker passed with pinned DNS/TLS and no credential/data.
- Standalone release-host timing: 20 samples each of five hostile workloads;
  maximum 1010.4 ms, activation-loop p95 1007.2 ms on Ryzen 5 7600 / Linux.
  See [machine-readable evidence](evidence/host-timing-linux.json). These are
  standalone activation measurements, not GUI or packaged-platform evidence.
- Website: 5 catalog tests and production build pass. The existing waitlist
  module needs a Resend key at build time; verification supplied a synthetic
  non-secret placeholder, without calling its email route.
- Browser evidence: Settings empty/paused/offline states; desktop catalog;
  mobile catalog at 390px without horizontal overflow; synthetic detail fixture
  and exact-version install-link UI. Fixtures are removed from the catalog seed.

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
- Frontend stop actions fence effects synchronously until inventory refresh
  completes. Older inventory responses cannot restore stale enabled state.
  Native generation changes remount views; failed releases still need explicit Retry.

## Unresolved release gates

- Stock built-app E2E for both examples, including real frontend/native IPC,
  actual controlled draft insertion and core UI operation during hostile plugins.
- Linux AppImage/deb and Windows NSIS installation/launch and bundled-host smoke
  evidence, with no dependency on a development directory or compiler.
- Complete failure-injection matrix: disk-full boundaries, every journal/crash
  point, unexpected child exit/ignored shutdown, concurrent update/uninstall,
  and delayed workspace/thread/composer races through the actual desktop.
- Windows OS credential backend execution (added to desktop CI); complete native
  HTTPS/DNS-rebinding/decoded-body/timeout fixtures. Linux OS round-trip and
  recorded locked-service/deletion-retry tests are passing.
- Complete keyboard/screen-reader, light/dark, small-window, chat-GUI-off and
  core pane restoration evidence; measured runtime fault and UI budgets with
  hardware/workload details.
- Publish reviewed independent package releases and the first immutable catalog
  artifact through the normal release workflow; then pin that published artifact
  in the website and verify website-to-desktop digest identity. The current seed
  deliberately contains no fabricated release URLs or installable listings.

No milestone with an unresolved exit gate is represented as complete.
