# Browser performance and correctness

Build the pinned, authenticated sidecar and place it beside the benchmark
executable. The production browser manager rejects other sidecar versions.
Chromium must also be installed:

```sh
bash scripts/copy-agent-browser.sh
cargo build -j 2 --manifest-path scripts/browser-benchmark/Cargo.toml
BROWSER_HOST_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
BROWSER_BENCH_TARGET_DIR="${CARGO_TARGET_DIR:-scripts/browser-benchmark/target}"
BROWSER_EXE_SUFFIX=""
case "$BROWSER_HOST_TRIPLE" in *windows*) BROWSER_EXE_SUFFIX=".exe" ;; esac
cp "src-tauri/binaries/agent-browser-$BROWSER_HOST_TRIPLE$BROWSER_EXE_SUFFIX" "$BROWSER_BENCH_TARGET_DIR/debug/agent-browser$BROWSER_EXE_SUFFIX"
cargo run -j 2 --manifest-path scripts/browser-benchmark/Cargo.toml -- results.json
```

This builds the actual `agent_browser.rs`, `stream_input.rs`, and
`browser_viewport.rs` modules, starts its own loopback fixture server, and uses
a unique browser session. It does not start the desktop app or access its live
workspaces. Synced viewport settings are deliberately disabled in the harness.
The browser manager used for screenshots and cleanup is created without its
startup reconciliation/cleanup path.

The output includes every measured duration, observed page state, expected
state for failures, and median/minimum/maximum summaries. Any correctness
failure makes the process exit with status 1 after writing the report and
closing its browser session.

Each interaction task has one discarded warmup and seven measured runs:

- Structured form: snapshot, fill two fields, submit, snapshot, then verify
  the exact submitted strings and one submission.
- Coordinate text: type 220 ASCII characters; verify the exact value and
  trusted input events.
- Unicode text: type accented characters, Japanese, emoji, and newlines;
  verify the exact value and trusted input events.
- Keyboard controls: type multiline text, Tab to a button, press Enter;
  verify both the exact text and one button activation.
- Coordinate clicks: click a button twelve times; verify exactly twelve
  trusted clicks.
- Screenshot: fill a fixture field, capture a nonempty PNG, and verify the
  field's exact value.

Navigation has three measured runs after initial startup. Each loads a page
with a script delayed by 250 ms and checks `document.readyState`, the script's
marker, and the page's load-event marker.

Timing includes each task's final observation and correctness check. Failed
checks poll for up to three seconds and record the actual state; their times
must not be presented as successful task performance. Page reset and coordinate
lookup happen before the interaction timer. Initial navigation, which includes
browser startup, is recorded separately.

These are browser-backend measurements, excluding model inference, outer
CLI/MCP transport, public-network variability, and desktop-pane rendering.
Run before and after sequentially with the same browser and fixture; do not
run compilation, tests, or another benchmark concurrently with the comparison.
