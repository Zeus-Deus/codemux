# Acceptance evidence map

This maps chapter 12 of [the requirements](BUILD-SPEC.md) to executable checks.
It is a review aid, not a declaration that the release gates pass. Recorded
revisions, results and remaining release gates are in [the ledger](IMPLEMENTATION.md).
Native UI harness targets below remain unverified until their installed-app CI
run passes. Browser mocks and a native broker test that supplies frontend effect
results cannot establish the corresponding desktop behavior.

| Requirement | Checks and evidence | Remaining desktop/release evidence |
| --- | --- | --- |
| Core independence | Existing full desktop CI; Settings browser evidence; native UI harness pause/resume | Clean/paused startup, zero host processes before use, terminal/projects/themes/update controls and pane restoration in installed apps |
| Functional SDK | Real-host package integrations in `manager.rs`; `sdk-native.mjs`; installed-app `native-ui.mjs` | Both packages' actual UI/draft/HTTPS operations must pass on Linux and Windows |
| Author independence | Both examples consume packed public SDK/CLI; Issue Companion built outside the checkout | Reviewed SDK distribution and final documentation against the published version |
| Runtime failure | `addon-host/tests`, native `protocol.rs`, manager fault isolation; packaged five-workload evidence | Installed-app responsiveness for all hostile workloads, including exit and ignored shutdown; the UI harness currently covers a blocking command |
| UI hostility | Protocol `tests/ui.rs` and `src/ui.rs`; renderer/view tests; native validation benchmark | Installed-app adversarial rendering and measured frame timing under load |
| Authority | `permissions.rs`, manager broker tests, generation/context checks | Installed-app forged/stale interaction scenarios in combination with real UI transitions |
| Context races | `platform.test.ts`, composer adapter/registry tests; real native broker cancellation, Git child reaping and update/uninstall serialization | Delayed request during project/thread/surface changes, uninstall during activation and HTTP/Git disable through the installed desktop |
| Package validation | `package.rs`: raw traversal/link/reserved-path fixtures, expansion limits, digest/identity checks; protocol manifest contracts | Supported-platform acceptance must retain these checks; installed import UX evidence is pending |
| Network | `http.rs`, `http_native_tests.rs`: real TLS, pinned DNS, limits, redirects, method/header/origin denial, timeout/cancellation and public HTTPS | Issue Companion's production HTTPS request and rendered result in the stock app |
| Secrets | `credentials.rs`: real Linux/Windows OS backends, locked-store fallback, source/origin isolation, historical-key cleanup and retry tombstones | Installed Settings credential/fallback/removal interaction and end-to-end redaction inspection |
| Persistence | Storage persistence, retained-source checks, corrupt registry and unclean-session tests | Installed app restart with grants/settings and core startup under corrupt plugin state |
| Updates | Lifecycle tests: eight durable interruptions, actual SQLite full, candidate failure, matching-data rollback, cancelled review, uninstall serialization | Filesystem disk-full boundaries and active-host desktop update/rollback races |
| UI/accessibility | Renderer/view/Settings tests and light/dark/small-window/GUI-off browser screenshots | Native keyboard/screen-reader checks, pane restoration, empty accessory/footer spacing |
| Remote boundary | Native RPC add-on rejection before rewriting and event forwarding; frontend remote/workspace checks | Installed desktop/remote client combination with active plugins |
| Website/catalog | Protocol/catalog validator, website catalog tests/build and desktop/mobile screenshots | Reviewed package releases and immutable catalog artifact; website/native digest identity against that artifact |
| Packaged platforms | deb/AppImage/NSIS payload provenance, clean-environment SDK callbacks and hostile-host deadlines | Stock desktop GUI acceptance is additional to payload checks |

## Running the installed-app harness

`addon-packaged.yml` builds the normal release installers and independent
packages, verifies their payloads, then runs external `tauri-driver` against the
installed desktop. It does not enable a production test server or change the
application binary. The harness refuses local and self-hosted runners.

The account API is a synthetic loopback fixture. The single-use native file
chooser seam returns the chosen archive path; all package validation, review,
permissions, installation, native host execution and effects remain real.
Issue Companion uses unauthenticated read-only public GitHub HTTPS through the
production broker. A rate limit is a failed gate, not a substituted success.

`addon-native-ui.yml` can retry the harness against the same run's saved
installer inputs, choosing Linux, Windows or both. Its ordinary PR job checks
only syntax and the refusal guard; that green check is **not native UI evidence**.
Every execution records the installer build revision, harness revision and
installer SHA-256 separately, plus per-step screenshots and text. A harness-only
retry never implies a newly compiled app was tested. Failure artifacts must be
read before changing either the harness or the implementation.
