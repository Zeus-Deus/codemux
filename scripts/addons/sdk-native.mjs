// Real Preact -> Remote DOM -> QuickJS -> stdio -> callback -> host request.
// Broker results here are a transport fixture, not evidence of native composer integration.
import { build } from "../../packages/plugin-cli/node_modules/esbuild/lib/main.js";
import { spawn } from "node:child_process";
import { readFile, mkdtemp, rm } from "node:fs/promises";
import { createInterface } from "node:readline";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";
const dir = await mkdtemp(join(tmpdir(), "codemux-sdk-native-"));
const entry = resolve("packages/plugin-sdk");
await build({
  stdin: {
    contents:
      "import plugin from './tests/fixture.tsx'; import {register} from './src/runtime.ts'; register(plugin);",
    resolveDir: entry,
  },
  bundle: true,
  format: "iife",
  platform: "neutral",
  target: "es2020",
  jsx: "automatic",
  jsxImportSource: "preact",
  outfile: join(dir, "plugin.js"),
});
const binary = resolve(
  process.argv[2] ??
    "src-tauri/addon-host/target/debug/codemux-addon-host" +
      (process.platform === "win32" ? ".exe" : ""),
);
const child = spawn(binary, [], {
  cwd: dir,
  env:
    process.platform === "win32" ? { SystemRoot: process.env.SystemRoot } : {},
  stdio: ["pipe", "pipe", "pipe"],
});
let diagnostics = "";
child.stderr.on("data", (data) => (diagnostics += data));
const frames = [];
let resolveWait;
const lines = createInterface({ input: child.stdout });
lines.on("line", (line) => {
  frames.push(JSON.parse(line));
  resolveWait?.();
});
let seq = 0;
const send = (method, params) =>
  child.stdin.write(
    JSON.stringify({
      jsonrpc: "2.0",
      generation: "sdk-native",
      id: ++seq,
      method,
      params,
    }) + "\n",
  );
async function until(predicate) {
  const end = Date.now() + 2000;
  while (Date.now() < end) {
    const index = frames.findIndex(predicate);
    if (index >= 0) return frames.splice(index, 1)[0];
    await new Promise((r) => {
      resolveWait = r;
      setTimeout(r, 20);
    });
  }
  throw Error("Native host deadline: " + diagnostics);
}
try {
  const manifest = JSON.parse(
    await readFile("src-tauri/addon-protocol/fixtures/hello.json", "utf8"),
  );
  manifest.permissions = ["composer.append"];
  manifest.contributes.panels = [
    { id: "test", title: "Test", icon: "file-text" },
  ];
  send("initialize", {
    protocolVersion: 1,
    manifest,
    source: await readFile(join(dir, "plugin.js"), "utf8"),
    final: true,
  });
  await until((m) => m.result?.accepted);
  send("activate", {});
  await until((m) => m.method === "ready" && m.params.phase === "activated");
  send("view.mount", {
    id: "test",
    kind: "panels",
    viewId: "view1",
    context: "context1",
  });
  const patch = await until((m) => m.method === "ui.patch");
  assert.match(JSON.stringify(patch), /Clicks:/);
  const button = patch.params.records[0][2].children.find(
    (n) => n.element === "cmx-button",
  );
  const callbackId = button.eventListeners.press.callbackId;
  // Hold the initial render acknowledgement while two independently completed
  // callbacks update state. A slow renderer must not fault a healthy SDK plugin.
  for (let n = 0; n < 2; n++) {
    send("ui.event", {
      viewId: "view1",
      callbackId,
      context: "trusted-interaction",
    });
    const request = await until((m) => m.method === "host.request");
    assert.equal(request.params.operation, "composer.appendText");
    assert.equal(request.params.params.context, "trusted-interaction");
    assert.equal(request.params.params.text, "From SDK");
    child.stdin.write(
      JSON.stringify({
        jsonrpc: "2.0",
        generation: "sdk-native",
        id: request.id,
        result: 1,
      }) + "\n",
    );
    await new Promise((r) => setTimeout(r, 100));
  }
  assert.equal(
    frames.filter((m) => m.method === "ui.patch").length,
    0,
    "SDK must wait for renderer acknowledgement",
  );
  send("ui.ack", { viewId: "view1", revision: 1 });
  const update = await until((m) => m.method === "ui.patch");
  assert.ok(update.params.records.some((r) => r[0] === 2 && r[2] === "2"));
  // Preact clears custom-element properties with an empty string.
  assert.ok(
    update.params.records.some(
      (r) => r[0] === 3 && r[2] === "direction" && r[3] === null,
    ),
  );
  // Queue another completed callback while this second render remains in flight.
  const nextCallback =
    update.params.records
      .filter((r) => r[0] === 3 && r[2] === "press" && r[3]?.callbackId)
      .at(-1)?.[3].callbackId ?? callbackId;
  send("ui.event", {
    viewId: "view1",
    callbackId: nextCallback,
    context: "trusted-interaction",
  });
  const pendingUpdate = await until((m) => m.method === "host.request");
  child.stdin.write(
    JSON.stringify({
      jsonrpc: "2.0",
      generation: "sdk-native",
      id: pendingUpdate.id,
      result: 1,
    }) + "\n",
  );
  await new Promise((r) => setTimeout(r, 100));
  assert.equal(frames.filter((m) => m.method === "ui.patch").length, 0);
  // Disposing a view drops pending records and ignores a late acknowledgement.
  send("view.unmount", { viewId: "view1" });
  send("ui.ack", { viewId: "view1", revision: 2 });
  await new Promise((r) => setTimeout(r, 100));
  assert.equal(frames.filter((m) => m.method === "ui.patch").length, 0);
  send("deactivate", {});
  child.stdin.end();
  console.log(
    "PASS: native SDK view, callback, scoped request, delayed render acknowledgement, unmount",
  );
} finally {
  child.kill();
  await new Promise((r) =>
    child.exitCode !== null ? r() : child.once("exit", r),
  );
  await rm(dir, { recursive: true, force: true });
}
