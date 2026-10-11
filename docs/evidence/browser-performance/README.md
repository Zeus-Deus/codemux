# Browser performance and correctness — 2026-10-06

The optimized browser backend passed **45/45 measured tasks**, compared with
**29/45** before. Every task includes its final observation and page-state
assertion. Measurements use real Chromium, the same local fixture, and
sequential before/after runs on Linux.

| Task | Before median | After median | Before correct | After correct |
| --- | ---: | ---: | ---: | ---: |
| Navigate with a delayed script | 25,867 ms | 583 ms | 3/3 | 3/3 |
| Snapshot, fill, submit, observe a form | 1,330 ms | 966 ms | 7/7 | 7/7 |
| Type 220 characters at coordinates | 2,830 ms | 254 ms | 7/7 | 7/7 |
| Unicode and multiline text | Failed | 221 ms | 0/7 | 7/7 |
| Multiline text, Tab, Enter activation | Failed | 214 ms | 0/7 | 7/7 |
| Twelve coordinate clicks | 2,304 ms | 340 ms | 5/7 | 7/7 |
| Fill, screenshot, verify the field | 685 ms | 503 ms | 7/7 | 7/7 |

The twelve-click baseline missed two clicks in one run and one in another.
The old typing path consistently discarded newlines, and the keyboard workflow
did not activate its button. Failed runs retain their observed state and
expected state in [before.json](before.json); their durations include a
three-second verification deadline and are not successful task timings.
The click median includes all seven runs, including the two failures.

Navigation checks `document.readyState === 'complete'`, execution of a script
delayed by 250 ms, and the load-event marker. Removing the extra load wait did
not allow this task to finish before the required page content was ready.

Screenshot checks verify a nonempty PNG and the exact field value. The final
captures were visually inspected, and [before.png](before.png) and
[after.png](after.png) have identical pixels and SHA-256 hashes:
`4fcc45efc255233f9272a027006cccbff289e45c763dad6d6445fea9075edbac`.

The production changes remove the duplicate navigation load wait, cache the
browser version probe, resolve the executable without a `which` subprocess,
and replace typing/clicking delays with an ordered WebSocket ping/pong
completion barrier. Text-bearing Enter/Space events preserve newlines and
focused-control activation. The barrier keeps the socket alive until the
server has dispatched all preceding events, preventing premature-close input
loss. Drag movement timing remains as before.

Baseline production modules come from commit
`f74bcb2fde35c152a7ea11e9fc4e82ce1b9388cd`; the unchanged benchmark harness
was compiled against those files in an isolated temporary directory. After
measurements compile the worktree's production modules directly. Both use
`agent-browser 0.24.0` and Google Chrome for Testing `151.0.7922.71`.
Initial browser startup is recorded separately; each interaction task has one
discarded warmup and seven measured runs. Navigation has three measured runs.
The baseline exits 1 because of its correctness failures; the optimized run
exits 0. Full sample data is in [before.json](before.json) and
[after.json](after.json).

The [benchmark](../../../scripts/browser-benchmark/README.md) is reusable for
future comparisons. It excludes model inference, outer CLI/MCP transport,
public-network variability, and desktop-pane rendering. These measurements
support browser-backend improvements, not a claim that every agent task becomes
44 times faster. Native execution was verified on Linux; Windows/macOS runtime
verification is left to their platform checks.

Verification:

- `cargo check -j 2 --manifest-path src-tauri/Cargo.toml` passed.
- The two `stream_input::tests` passed in the repository test binary and in the
  standalone benchmark build. They check that success waits for dispatch,
  unrelated pong messages do not satisfy completion, and early disconnection
  is an error.
- `build_command_open_uses_navigation_load_wait_only` passed in the repository
  test binary and in the standalone benchmark build.
- The optimized real-browser benchmark passed all 45 measured assertions;
  its discarded warmup tasks passed as well.
- `git diff --check` passed. The repository test build emitted four unrelated
  unused-variable warnings in existing `src/git.rs` tests.
