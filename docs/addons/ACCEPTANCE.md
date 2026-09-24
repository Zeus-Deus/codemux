# Acceptance evidence map

This maps chapter 12 of [the requirements](BUILD-SPEC.md) to executable checks.
Recorded revisions, results and the remaining publication steps are in
[the ledger](IMPLEMENTATION.md). Installed-app evidence below comes from the
stock installers built from the final source ([run 35916722628](https://github.com/Zeus-Deus/codemux/actions/runs/35916722628); [Linux](evidence/native-ui-linux-final-81b4e666.json) and [Windows](evidence/native-ui-windows-final-81b4e666.json) results). Browser mocks
and a native broker test that supplies frontend effect results are unit
coverage only; they are never counted as desktop evidence.

| Requirement | Checks | Installed-app evidence on Linux and Windows |
| --- | --- | --- |
| Core independence | Full desktop CI; Settings tests; pause/resume and registry tests | Zero hosts at clean start and while paused; lazy start; terminal, draft typing and projects keep working; themes; read-only official updater check with and without add-ons; core panes restored after pause and removal; no empty accessory space |
| Functional SDK | Real-host package tests in `manager.rs` (panels, commands, composer actions, composer view, links); `sdk-native.mjs` | Both examples install from their packages; Project Brief's command, panel and composer action; Issue Companion's public HTTPS, accessory, attributed link and draft insertion; setting and private preference survive restart |
| Author independence | Both examples and all CI fixtures built only with the packed SDK/CLI; starter built outside the checkout (`author.test.mjs`) | The installed examples and fixtures are those packed builds |
| Runtime failure | `addon-host/tests` (CPU, timers, stack, heap, quotas, forged frames); `protocol.rs`; manager fault isolation and unexpected exit | Throw, loop, endless promises, recursion and allocation each quarantine only the fixture while terminal, draft and Project Brief keep working; packaged deadlines for deb, rpm, AppImage and NSIS |
| UI hostility | `addon-protocol/tests/ui.rs`, manager UI limits (batch rate, queue, views, callbacks per plugin, malformed IDs), renderer and view tests | 500-row virtualized list with frame budget and ARIA positions; stale events stay inline |
| Authority | `permissions.rs`; broker tests for forged context, wrong generation, expired or reused interaction, undeclared method and denied capability | Paired remote client cannot invoke or subscribe to add-ons |
| Context races | Real-host delayed-effect transitions, Git child reaping, HTTP cancellation, uninstall during activation; adapter and platform tests | Typing during a delayed append keeps user text; project switch, thread close and composer replacement reject with `CONTEXT_STALE`; disable cancels; removal during activation leaves no host and no insertion |
| Package validation | `package.rs` raw archive fixtures, expansion limits, digest and identity; shared manifest case corpus for desktop and CLI | Native import, review and source-replacement confirmation |
| Network | `http.rs`, `http_native_tests.rs`: real TLS, pinned DNS, private and mapped addresses, redirects, header and body limits, timeout, cancellation, public HTTPS | Production HTTPS request, rendering and draft insertion |
| Secrets | `credentials.rs`: OS backends (Windows in CI, Linux Secret Service in CI), locked store, session-only fallback, isolation, cleanup tombstones, clearing | Credential states in Settings, explicit session-only fallback on Linux, OS save and delete on Windows, redaction of driver logs, removal |
| Persistence | Storage, retained-source, registry corruption and reset, unclean-session tests | Restart keeps installations, grants, settings and data; corrupt registry keeps core startup and offers reset; reset restores management |
| Updates | Lifecycle tests: eight interruption points, SQLite full, candidate failure, rollback with matching data, blocklist disable, catalog cache and fresh-fetch rules; six real ENOSPC checkpoints on Linux | Active-host update, expanded-access review and cancellation, rollback with matching private data |
| UI and access | Renderer, view, composer and Settings tests (keyboard, focus, dialogs, errors inside dialogs, screen-reader labels) | Keyboard dialog focus, light and dark themes, chat GUI off, small window, core pane restoration |
| Remote boundary | `addon_*` rejection before rewriting and event forwarding; remote client and remote workspace tests | Paired loopback HTTP/WebSocket client with active add-ons |
| Website/catalog | Protocol and catalog-crate validation (duplicates, ownership, rollback, blocked, schema); website tests and build | Publication deferred: the first catalog artifact and website-to-desktop digest check follow a maintainer release |
| Packaged platforms | Exact host provenance, clean-environment SDK callbacks and hostile deadlines for every built installer | The same installers run the full installed-app suite above |

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

The race fixtures (`scripts/addons/fixtures/context-races` and
`activation-race`) are ordinary packages built with the packed SDK and CLI by
`build-examples.sh`, or by `build-ci-fixtures.sh` for harness-only retries.
They report outcomes in their own panel, so the notification limit cannot hide
a result. On Linux the harness puts a logging `xdg-open` first on the app's
`PATH` to prove an attributed link reached the system opener; on Windows only
the attribution toast is asserted. Clicks the driver did not dispatch (a stale,
covered or scrolled-away element) are retried; every other failure is a failed
gate.
