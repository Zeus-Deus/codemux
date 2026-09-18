# Acceptance evidence map

This maps chapter 12 of [the requirements](BUILD-SPEC.md) to executable checks.
It is a review aid, not a declaration that the release gates pass. Recorded
revisions, results and remaining release gates are in [the ledger](IMPLEMENTATION.md).
Native UI harness targets below remain unverified until their installed-app CI
run passes. Browser mocks and a native broker test that supplies frontend effect
results cannot establish the corresponding desktop behavior.

| Requirement | Checks and evidence | Remaining desktop/release evidence |
| --- | --- | --- |
| Core independence | Existing full desktop CI; Settings browser evidence; native UI harness pause/resume | Saved installers pass clean/paused zero-host checks, terminal/input/projects, pane restoration, themes and read-only official updater checks; final-source repeat remains |
| Functional SDK | Real-host package integrations in `manager.rs`; `sdk-native.mjs`; installed-app `native-ui.mjs` | Both pass on saved Linux/Windows installers; repeat on final-source installers |
| Author independence | Both examples consume packed public SDK/CLI; Issue Companion built outside the checkout | Reviewed SDK distribution and final documentation against the published version |
| Runtime failure | `addon-host/tests`, native `protocol.rs`, manager fault isolation; packaged five-workload evidence | Both saved installers pass all five SDK callback workloads; Linux includes the backpressure fix. Native real-child tests cover unexpected exit and hostile shutdown; final-source platform checks remain |
| UI hostility | Protocol `tests/ui.rs` and `src/ui.rs`; renderer/view tests; native validation benchmark | Native pre-render rejection tests pass; both saved installers pass 500-path virtualization/frame budgets. Final-source repeat remains |
| Authority | `permissions.rs`, manager broker tests, generation/context checks | Installed-app forged/stale interaction scenarios in combination with real UI transitions |
| Context races | `platform.test.ts`, composer adapter/registry tests; real native broker cancellation, Git child reaping and update/uninstall serialization | Delayed request during project/thread/surface changes, uninstall during activation and HTTP/Git disable through the installed desktop |
| Package validation | `package.rs`: raw traversal/link/reserved-path fixtures, expansion limits, digest/identity checks; protocol manifest contracts | Both saved installers pass native archive import/review; retain inert rejection tests on the final source |
| Network | `http.rs`, `http_native_tests.rs`: real TLS, pinned DNS, limits, redirects, method/header/origin denial, timeout/cancellation and public HTTPS | Production HTTPS/render/draft passes on saved Linux/Windows installers; final-source repeat pending |
| Secrets | `credentials.rs`: real Linux/Windows OS backends, locked-store fallback, source/origin isolation, historical-key cleanup and retry tombstones | Installed Settings fallback/removal/redaction passes on Linux and native OS save/delete/redaction passes on Windows; final-source repeat remains |
| Persistence | Storage persistence, retained-source checks, corrupt registry and unclean-session tests | Both saved installers pass actual restart/grants/settings, removal and core startup with corrupt registry; final-source repeat pending |
| Updates | Lifecycle tests: eight durable interruptions, actual SQLite full, candidate failure, matching-data rollback, cancelled review, uninstall serialization | Six actual 16 MiB tmpfs ENOSPC checkpoints, including exact recovered data generation, pass on Linux; active-host desktop update/rollback races remain |
| UI/accessibility | Renderer/view/Settings tests and light/dark/small-window/GUI-off browser screenshots | Both saved installers pass chat-GUI-off panel/terminal behavior, keyboard dialog focus, native themes, core pane restoration and absence of empty accessory/footer spacing; corrected virtual row ARIA needs final-source native inspection |
| Remote boundary | Native RPC add-on rejection before rewriting and event forwarding; frontend remote/workspace checks | Paired installed-desktop HTTP/WebSocket with active plugins passes on Linux and Windows; final-source repeat remains |
| Website/catalog | Protocol/catalog validator, website catalog tests/build and desktop/mobile screenshots | Reviewed package releases and immutable catalog artifact; website/native digest identity against that artifact |
| Packaged platforms | deb/AppImage/NSIS payload provenance, clean-environment SDK callbacks and hostile-host deadlines | Stock desktop GUI acceptance is additional to payload checks |

## Running the installed-app harness

`addon-packaged.yml` builds the normal release installers and independent
packages, verifies their payloads, then runs external WebDriver against the
installed desktop (`tauri-driver` on Linux; Microsoft WebView2 attach on Windows). It does not enable a production test server or change the
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
