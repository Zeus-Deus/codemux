# Frozen isolated v1 artifact

`plugin.mjs` is release input, not generated output. Its SHA256 is pinned both in `plugin.sha256` and the acceptance test:

```
5aec390d81a7e6cd1e99232276d02fd0a8ea9c14c611578aa56904b2bf4e5b70
```

Do not rebuild or rewrite it to make SDK updates pass. A future incompatible artifact needs a new versioned directory. It imports no current SDK and runs unchanged against `sidecar/plugin-host/main.mjs` or the bundled executable selected by `CODEMUX_PLUGIN_HOST_TEST_BINARY`.

The fixture discovers an unknown optional capability as absent, refuses activation-time host authority, preserves frozen caller/target distinctions and stable operation identity, persists counters across worker restart, strips author-supplied card provenance, keeps historical summary text usable with no worker, and handles cancellation without breaking the next invocation. Domain compatibility/grants and actual native card stamping are covered by native platform integration tests, not this deterministic host fixture.

```sh
node --test examples/plugins/platform-frozen-v1/test/*.test.mjs
CODEMUX_PLUGIN_HOST_TEST_BINARY=./src-tauri/binaries/codemux-plugin-host-x86_64-unknown-linux-gnu node --test examples/plugins/platform-frozen-v1/test/*.test.mjs
```
