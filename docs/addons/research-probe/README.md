# Isolated plugin architecture probe

Executed on Linux x86_64 during the planning task, 18 September 2026. This is a small compatibility experiment, not a plugin manager or a production sandbox implementation.

It checks that Preact and Remote DOM run inside rquickjs, UI callback IDs can cross JSON, a callback can emit an illustrative host-operation request, and a state change emits a follow-up patch. The Rust harness prints messages; it does not edit a real CodeMux draft or render a real CodeMux pane.

It separately checks an infinite loop against a 250 ms deadline and a 128 MiB allocation against a 64 MiB heap limit. `probe-results.json` records the actual outputs. These tests passed locally. Windows and integration testing remain release gates in BUILD-SPEC.md.

## Reproduce

Use Node/npm, Rust, and a C compiler in a disposable development directory. No CodeMux checkout is required.

```sh
npm ci --ignore-scripts --no-audit --no-fund
node node_modules/esbuild/bin/esbuild probe-ui.js --bundle --format=iife --platform=neutral --target=es2020 --outfile=probe-ui.bundle.js
cargo build -j 2 --locked
./target/debug/codemux-sdk-probe ui
./target/debug/codemux-sdk-probe spin
./target/debug/codemux-sdk-probe memory
```

Expected: UI records contain a text node, a button callback ID, an illustrative `composer.appendText` request, and an updated text node. Global checks report `undefined` for process/require/fetch/Tauri. Spin reports an interrupted engine exception, memory reports out of memory, and each mode ends with `PROBE_OK`.

The production runtime must add the broker, bounded transport, permissions, cleanup, resource supervision, package lifecycle, and host UI validator specified in the build document. Do not ship this harness as that implementation.
