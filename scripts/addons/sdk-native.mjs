// Real Preact -> Remote DOM -> QuickJS -> stdio -> callback -> host request.
// Broker results here are a transport fixture, not evidence of native composer integration.
import { build } from "../../packages/plugin-cli/node_modules/esbuild/lib/main.js";
import { bundleOptions } from "../../packages/plugin-cli/src/bundle.mjs";
import { spawn } from "node:child_process";
import { readFile, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createContext, runInContext } from "node:vm";
import assert from "node:assert/strict";
const dir = await mkdtemp(join(tmpdir(), "codemux-sdk-native-"));
const entry = resolve("packages/plugin-sdk");
const binary = resolve(
  process.argv[2] ??
    "src-tauri/addon-host/target/debug/codemux-addon-host" +
      (process.platform === "win32" ? ".exe" : ""),
);
const hello = JSON.parse(
  await readFile("src-tauri/addon-protocol/fixtures/hello.json", "utf8"),
);
// The same options as `codemux-plugin build`, pointed at the SDK sources.
async function bundle(fixture, extra = {}) {
  const result = await build({
    ...bundleOptions({
      entry: fixture,
      runtime: "./src/runtime.ts",
      resolveDir: entry,
    }),
    ...extra,
    write: false,
  });
  return result.outputFiles[0].text;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const hosts = [];
async function start(source, manifest) {
  const child = spawn(binary, [], {
    cwd: dir,
    env:
      process.platform === "win32"
        ? { SystemRoot: process.env.SystemRoot }
        : {},
    stdio: ["pipe", "pipe", "pipe"],
  });
  hosts.push(child);
  let diagnostics = "";
  child.stderr.on("data", (data) => (diagnostics += data));
  const exited = new Promise((r) => child.once("exit", r));
  const frames = [];
  // Every received frame with its arrival time, for quota assertions.
  const history = [];
  let resolveWait;
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    frames.push(message);
    history.push({ at: performance.now(), message });
    resolveWait?.();
  });
  let seq = 0;
  const write = (message) =>
    child.stdin.write(
      JSON.stringify({ jsonrpc: "2.0", generation: "sdk-native", ...message }) +
        "\n",
    );
  const host = {
    child,
    frames,
    history,
    exited,
    alive: () => child.exitCode === null && child.signalCode === null,
    send: (method, params) => write({ id: ++seq, method, params }),
    respond: (id, result) => write({ id, result }),
    reject: (id, code, message) =>
      write({ id, error: { code: -32000, message, data: { code } } }),
    async until(predicate, ms = 2000) {
      const end = Date.now() + ms;
      while (Date.now() < end) {
        const index = frames.findIndex(predicate);
        if (index >= 0) return frames.splice(index, 1)[0];
        if (!host.alive()) break;
        await new Promise((r) => {
          resolveWait = r;
          setTimeout(r, 20);
        });
      }
      throw Error("Native host deadline: " + diagnostics);
    },
    // Execute a command and wait until its synchronous work has yielded.
    async execute(id, context = "trusted-interaction") {
      host.send("command.execute", { id, kind: "commands", context });
      const accepted = seq;
      await host.until(
        (m) => m.method === "ready" && m.params.requestId === accepted,
      );
    },
    async stopped(ms = 2000) {
      await Promise.race([exited, sleep(ms)]);
      return !host.alive();
    },
  };
  host.send("initialize", {
    protocolVersion: 1,
    manifest,
    source,
    final: true,
  });
  await host.until((m) => m.result?.accepted);
  host.send("activate", {});
  return host;
}
const activated = (host) =>
  host.until((m) => m.method === "ready" && m.params.phase === "activated");
// Largest number of frames matching `predicate` inside any rolling window.
function peak(host, predicate, ms = 1000) {
  const times = host.history
    .filter(({ message }) => predicate(message))
    .map(({ at }) => at);
  let most = 0;
  for (let start = 0, end = 0; end < times.length; end++) {
    while (times[end] - times[start] >= ms) start++;
    most = Math.max(most, end - start + 1);
  }
  return most;
}
// The press callback of the one button a patch inserts.
function pressCallback(patch) {
  const found = [];
  const visit = (value) => {
    if (value?.element === "cmx-button")
      found.push(value.eventListeners.press.callbackId);
    if (value && typeof value === "object") Object.values(value).forEach(visit);
  };
  visit(patch.params.records);
  assert.equal(found.length, 1, JSON.stringify(patch.params.records));
  return found[0];
}
try {
  {
    const manifest = structuredClone(hello);
    manifest.permissions = ["composer.append"];
    manifest.contributes.panels = [
      { id: "test", title: "Test", icon: "file-text" },
    ];
    const host = await start(await bundle("./tests/fixture.tsx"), manifest);
    await activated(host);
    host.send("view.mount", {
      id: "test",
      kind: "panels",
      viewId: "view1",
      context: "context1",
    });
    const patch = await host.until((m) => m.method === "ui.patch");
    assert.match(JSON.stringify(patch), /Clicks:/);
    const button = patch.params.records[0][2].children.find(
      (n) => n.element === "cmx-button",
    );
    const callbackId = button.eventListeners.press.callbackId;
    // Hold the initial render acknowledgement while two independently completed
    // callbacks update state. A slow renderer must not fault a healthy SDK plugin.
    for (let n = 0; n < 2; n++) {
      host.send("ui.event", {
        viewId: "view1",
        callbackId,
        context: "trusted-interaction",
      });
      const request = await host.until((m) => m.method === "host.request");
      assert.equal(request.params.operation, "composer.appendText");
      assert.equal(request.params.params.context, "trusted-interaction");
      assert.equal(request.params.params.text, "From SDK");
      host.respond(request.id, 1);
      await sleep(100);
    }
    assert.equal(
      host.frames.filter((m) => m.method === "ui.patch").length,
      0,
      "SDK must wait for renderer acknowledgement",
    );
    host.send("ui.ack", { viewId: "view1", revision: 1 });
    const update = await host.until((m) => m.method === "ui.patch");
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
    host.send("ui.event", {
      viewId: "view1",
      callbackId: nextCallback,
      context: "trusted-interaction",
    });
    const pendingUpdate = await host.until((m) => m.method === "host.request");
    host.respond(pendingUpdate.id, 1);
    await sleep(100);
    assert.equal(host.frames.filter((m) => m.method === "ui.patch").length, 0);
    // Disposing a view drops pending records and ignores a late acknowledgement.
    host.send("view.unmount", { viewId: "view1" });
    host.send("ui.ack", { viewId: "view1", revision: 2 });
    await sleep(100);
    assert.equal(host.frames.filter((m) => m.method === "ui.patch").length, 0);
    host.send("deactivate", {});
    host.child.stdin.end();
    console.log(
      "PASS: native SDK view, callback, scoped request, delayed render acknowledgement, unmount",
    );
  }
  const policy = structuredClone(hello);
  policy.permissions = ["composer.append"];
  policy.contributes.commands = [
    "handled",
    "reject",
    "throw",
    "later",
    "dispose",
    "burst",
    "exhaust",
    "flood",
  ].map((id) => ({ id, title: "Run " + id, requiresWorkspace: false }));
  policy.contributes.panels = [
    { id: "form", title: "Form", icon: "list" },
    { id: "swap", title: "Swap", icon: "refresh-cw" },
    { id: "grid", title: "Grid", icon: "list" },
  ];
  const policySource = await bundle("./tests/policy.tsx");
  {
    const host = await start(policySource, policy);
    await activated(host);
    // A stable host rejection from a command handler is reported, not fatal.
    await host.execute("handled");
    const notify = await host.until((m) => m.method === "host.request");
    host.reject(notify.id, "RESOURCE_LIMIT", "Notification limit");
    const log = await host.until((m) => m.method === "log");
    assert.match(log.params.message, /commands\/handled.*RESOURCE_LIMIT/);
    // The same holds for a UI callback promise, and the plugin keeps working.
    host.send("view.mount", {
      id: "form",
      kind: "panels",
      viewId: "form1",
      context: "context1",
    });
    const form = await host.until((m) => m.method === "ui.patch");
    host.send("ui.ack", { viewId: "form1", revision: 1 });
    const nodes = JSON.stringify(form.params.records);
    const field = form.params.records[0][2].children.find(
      (n) => n.element === "cmx-text-field",
    );
    const add = form.params.records[0][2].children.find(
      (n) => n.element === "cmx-button",
    );
    assert.ok(field && add, nodes);
    host.send("ui.event", {
      viewId: "form1",
      callbackId: add.eventListeners.press.callbackId,
      context: "trusted-interaction",
    });
    const append = await host.until((m) => m.method === "host.request");
    assert.equal(append.params.operation, "composer.appendText");
    host.reject(append.id, "NO_COMPOSER", "No chat composer is available");
    assert.match(
      (await host.until((m) => m.method === "log")).params.message,
      /UI callback.*NO_COMPOSER/,
    );
    // A disposed command stays declared; invoking it is ignored, not fatal.
    await host.execute("dispose");
    await host.execute("later");
    assert.match(
      (await host.until((m) => m.method === "log")).params.message,
      /commands\/later was disposed/,
    );
    assert.ok(!host.frames.some((m) => m.method === "host.request"));
    // Subscriptions receive host notifications; a disposed listener does not.
    host.send("workspace.changed", { context: "context2" });
    const workspace = await host.until((m) => m.method === "host.request");
    assert.equal(workspace.params.params.message, "workspace context2");
    host.respond(workspace.id, null);
    host.send("settings.changed", { settings: { owner: "synthetic" } });
    const settings = await host.until((m) => m.method === "host.request");
    assert.equal(
      settings.params.params.message,
      'settings {"owner":"synthetic"}',
    );
    host.respond(settings.id, null);
    await sleep(100);
    assert.ok(!host.frames.some((m) => m.method === "host.request"));
    assert.ok(host.alive(), "handled rejections must not stop the plugin");
    console.log(
      "PASS: stable host rejections, disposed registrations and subscriptions keep the plugin running",
    );
  }
  {
    // The desktop checks each event against the tree it has applied, so a
    // click can cross the patch that released its callback, or an unmount.
    // Such a stale event is ignored; the plugin keeps running.
    const host = await start(policySource, policy);
    await activated(host);
    const press = (viewId, callbackId) =>
      host.send("ui.event", {
        viewId,
        callbackId,
        context: "trusted-interaction",
      });
    const log = (m) => m.method === "log";
    host.send("view.mount", {
      id: "swap",
      kind: "panels",
      viewId: "swap1",
      context: "context1",
    });
    const first = pressCallback(
      await host.until((m) => m.method === "ui.patch"),
    );
    host.send("ui.ack", { viewId: "swap1", revision: 1 });
    press("swap1", first);
    const replaced = await host.until((m) => m.method === "ui.patch");
    assert.ok(
      replaced.params.records.some((r) => r[0] === 1),
      "the pressed button must be removed",
    );
    const second = pressCallback(replaced);
    assert.notEqual(second, first);
    host.send("ui.ack", { viewId: "swap1", revision: 2 });
    // The first callback was released when that patch was sent.
    press("swap1", first);
    assert.match(
      (await host.until(log)).params.message,
      /released callback was ignored/,
    );
    press("never-mounted", second);
    assert.match((await host.until(log)).params.message, /closed view/);
    await sleep(100);
    assert.ok(
      !host.frames.some((m) => m.method === "ui.patch"),
      "an ignored event must not run a callback",
    );
    // A current callback still runs and renders.
    press("swap1", second);
    const next = await host.until((m) => m.method === "ui.patch");
    assert.match(JSON.stringify(next.params.records), /Round 2/);
    host.send("ui.ack", { viewId: "swap1", revision: 3 });
    host.send("view.unmount", { viewId: "swap1" });
    press("swap1", pressCallback(next));
    assert.match((await host.until(log)).params.message, /closed view/);
    // The same host still mounts and serves a new view.
    host.send("view.mount", {
      id: "swap",
      kind: "panels",
      viewId: "swap2",
      context: "context1",
    });
    const again = await host.until((m) => m.method === "ui.patch");
    assert.equal(again.params.viewId, "swap2");
    host.send("ui.ack", { viewId: "swap2", revision: 1 });
    press("swap2", pressCallback(again));
    assert.match(
      JSON.stringify((await host.until((m) => m.method === "ui.patch")).params),
      /Round 1/,
    );
    assert.ok(host.alive(), "stale UI events must not stop the plugin");
    console.log(
      "PASS: UI events for released callbacks and closed views are ignored",
    );
  }
  {
    // The desktop enforces the four-view limit, and a remount can deliver the
    // new view's mount before the unmount that freed its slot. Four views are
    // live when the fifth mount arrives; the plugin must serve it.
    const host = await start(policySource, policy);
    await activated(host);
    const mount = async (viewId) => {
      host.send("view.mount", {
        id: "swap",
        kind: "panels",
        viewId,
        context: "context1",
      });
      const patch = await host.until(
        (m) => m.method === "ui.patch" && m.params.viewId === viewId,
      );
      host.send("ui.ack", { viewId, revision: 1 });
      return pressCallback(patch);
    };
    for (let n = 1; n <= 4; n++) await mount("swap" + n);
    const fifth = await mount("swap5");
    host.send("view.unmount", { viewId: "swap1" });
    host.send("ui.event", {
      viewId: "swap5",
      callbackId: fifth,
      context: "trusted-interaction",
    });
    const update = await host.until(
      (m) => m.method === "ui.patch" && m.params.viewId === "swap5",
    );
    assert.match(JSON.stringify(update.params.records), /Round 1/);
    assert.ok(host.alive(), "a mount ahead of its unmount must not fault");
    // The desktop issues each view ID once; a repeated mount stays a fault.
    host.send("view.mount", {
      id: "swap",
      kind: "panels",
      viewId: "swap5",
      context: "context1",
    });
    assert.ok(await host.stopped(), "a repeated view ID must stop the plugin");
    console.log(
      "PASS: a mount that arrives before the unmount freeing its slot is served",
    );
  }
  {
    // The desktop also bounds live callbacks over the views it holds. Four
    // grids hold all 4,096; a view mounted before grid1's unmount arrives
    // stays within the desktop's count, so the plugin must render it.
    // Rendering 4,096 SDK buttons does not fit the host's CPU deadline, so
    // this block runs the same bundle on the host's bootstrap in a Node realm.
    const frames = [];
    // What the host treats as fatal: an unhandled rejection or a throwing timer.
    const faults = [];
    const fault = (error) => faults.push(error);
    process.on("unhandledRejection", fault);
    const timers = new Map();
    const epoch = performance.now();
    const realm = createContext({
      __nativeNow: () => Math.floor(performance.now() - epoch),
      __nativeSend: (line) => frames.push(JSON.parse(line)),
      __nativeTimer: (delay, repeat) => {
        const id = timers.size + 1;
        const tick = () => {
          try {
            realm.__tick(id);
          } catch (error) {
            fault(error);
          }
        };
        timers.set(
          id,
          repeat ? setInterval(tick, delay) : setTimeout(tick, delay),
        );
        return id;
      },
      __nativeClearTimer: (id) => clearTimeout(timers.get(id)),
    });
    try {
      runInContext(
        await readFile("src-tauri/addon-host/src/bootstrap.js", "utf8"),
        realm,
      );
      realm.__configure("sdk-native", JSON.stringify(policy));
      runInContext(policySource, realm);
      let seq = 0;
      const send = (method, params) =>
        realm.__dispatch(
          JSON.stringify({ jsonrpc: "2.0", id: ++seq, method, params }),
        );
      const until = async (predicate) => {
        const end = Date.now() + 5000;
        while (Date.now() < end && !faults.length) {
          const index = frames.findIndex(predicate);
          if (index >= 0) return frames.splice(index, 1)[0];
          await sleep(10);
        }
        throw Error("Realm deadline: " + faults.join("; "));
      };
      const patch = (viewId) =>
        until((m) => m.method === "ui.patch" && m.params.viewId === viewId);
      send("activate", {});
      await until((m) => m.method === "ready");
      const callbacks = new Set();
      const collect = (value) => {
        if (value?.callbackId) callbacks.add(value.callbackId);
        if (value && typeof value === "object")
          Object.values(value).forEach(collect);
      };
      for (let n = 1; n <= 4; n++) {
        const viewId = "grid" + n;
        send("view.mount", {
          id: "grid",
          kind: "panels",
          viewId,
          context: "c",
        });
        collect((await patch(viewId)).params.records);
        send("ui.ack", { viewId, revision: 1 });
      }
      assert.equal(callbacks.size, 4096);
      send("view.mount", {
        id: "swap",
        kind: "panels",
        viewId: "swap5",
        context: "c",
      });
      const fifth = pressCallback(await patch("swap5"));
      send("ui.ack", { viewId: "swap5", revision: 1 });
      send("view.unmount", { viewId: "grid1" });
      send("ui.event", {
        viewId: "swap5",
        callbackId: fifth,
        context: "trusted-interaction",
      });
      assert.match(
        JSON.stringify((await patch("swap5")).params.records),
        /Round 1/,
      );
      assert.deepEqual(
        faults,
        [],
        "callbacks of a departing view must not fault",
      );
    } finally {
      process.off("unhandledRejection", fault);
      for (const timer of timers.values()) clearTimeout(timer);
    }
    console.log(
      "PASS: callbacks of a view whose unmount is still on its way do not fault",
    );
  }
  for (const [change, reason] of [
    [(e) => ({ ...e, callbackId: Number(e.callbackId) }), "a numeric callback"],
    [(e) => ({ ...e, value: { text: "x" } }), "an object value"],
    [({ context, ...e }) => e, "a missing context"],
  ]) {
    // Malformed events remain faults, even for a live view and callback.
    const host = await start(policySource, policy);
    await activated(host);
    host.send("view.mount", {
      id: "swap",
      kind: "panels",
      viewId: "swap1",
      context: "context1",
    });
    const callbackId = pressCallback(
      await host.until((m) => m.method === "ui.patch"),
    );
    host.send(
      "ui.event",
      change({ viewId: "swap1", callbackId, context: "trusted-interaction" }),
    );
    assert.ok(await host.stopped(), `${reason} must stop the plugin`);
  }
  console.log("PASS: malformed UI events remain faults");
  {
    // Controlled input at typing speed: 60 changes/s for 2 s with prompt
    // acknowledgements must stay under the 30 batches/s native limit.
    const host = await start(policySource, policy);
    await activated(host);
    host.send("view.mount", {
      id: "form",
      kind: "panels",
      viewId: "form2",
      context: "context1",
    });
    const first = await host.until((m) => m.method === "ui.patch");
    const field = first.params.records[0][2].children.find(
      (n) => n.element === "cmx-text-field",
    );
    let revision = 1;
    let value = "";
    host.send("ui.ack", { viewId: "form2", revision });
    const acknowledge = () => {
      for (const m of host.frames.splice(0))
        if (m.method === "ui.patch")
          host.send("ui.ack", { viewId: "form2", revision: ++revision });
    };
    for (let n = 1; n <= 120; n++) {
      value = "x".repeat(n);
      host.send("ui.event", {
        viewId: "form2",
        callbackId: field.eventListeners.change.callbackId,
        context: "trusted-interaction",
        value,
      });
      acknowledge();
      await sleep(1000 / 60);
    }
    const latest = () =>
      host.history
        .filter(({ message }) => message.method === "ui.patch")
        .flatMap(({ message }) => message.params.records)
        .filter((r) => r[0] === 3 && r[1] === field.id && r[2] === "value")
        .at(-1)?.[3];
    for (let n = 0; n < 200 && latest() !== value && host.alive(); n++) {
      acknowledge();
      await sleep(25);
    }
    assert.equal(latest(), value, "the last input must be rendered");
    const most = peak(host, (m) => m.method === "ui.patch");
    assert.ok(most <= 30, `${most} UI batches arrived within one second`);
    assert.ok(host.alive(), "paced UI updates must not stop the plugin");
    // Sequential requests are paced under 20/s and 100/min; beyond the minute
    // budget the SDK rejects locally with RESOURCE_LIMIT instead of faulting.
    const requests = (m) => m.method === "host.request";
    const sent = () =>
      host.history.filter(({ message }) => requests(message)).length;
    const answer = async (done) => {
      const end = Date.now() + 15000;
      while (!done()) {
        assert.ok(Date.now() < end && host.alive(), "requests stalled");
        for (const m of host.frames.splice(0))
          if (requests(m)) host.respond(m.id, null);
        await sleep(10);
      }
    };
    await host.execute("burst");
    await answer(() => sent() === 40);
    await host.execute("exhaust");
    await answer(() =>
      host.history.some(
        ({ message }) =>
          message.method === "log" &&
          message.params.message === "rejected RESOURCE_LIMIT",
      ),
    );
    await sleep(200);
    assert.ok(
      sent() <= 100 && sent() >= 90,
      `${sent()} requests reached the host`,
    );
    const requestPeak = peak(host, requests);
    assert.ok(requestPeak <= 20, `${requestPeak} requests within one second`);
    // Console output is paced under five entries per second.
    await host.execute("flood");
    await sleep(1500);
    const logPeak = peak(host, (m) => m.method === "log");
    assert.ok(logPeak <= 5, `${logPeak} log entries within one second`);
    assert.ok(host.alive(), "paced requests and logs must not stop the plugin");
    console.log(
      `PASS: paced traffic (${most} UI batches, ${requestPeak} requests, ${logPeak} logs per second at most)`,
    );
  }
  for (const [command, reason] of [
    ["reject", "a rejection that is not a stable host error"],
    ["throw", "a synchronous throw"],
  ]) {
    const host = await start(policySource, policy);
    await activated(host);
    host.send("command.execute", {
      id: command,
      kind: "commands",
      context: "trusted-interaction",
    });
    assert.ok(await host.stopped(), `${reason} must stop the plugin`);
  }
  console.log("PASS: plain rejections and synchronous throws remain faults");
  for (const variant of ["undeclared", "duplicate", "missing"]) {
    const host = await start(
      await bundle("./tests/registration.ts", {
        define: {
          "process.env.NODE_ENV": '"production"',
          CASE: JSON.stringify(variant),
        },
      }),
      hello,
    );
    assert.ok(await host.stopped(), `${variant} registration must fail`);
    assert.ok(
      !host.history.some(
        ({ message }) =>
          message.method === "ready" && message.params.phase === "activated",
      ),
      `${variant} registration must not become ready`,
    );
  }
  console.log(
    "PASS: undeclared, duplicate and missing registrations fail activation",
  );
  for (const [example, calls] of [
    [
      "project-brief",
      [
        ["commands", "open", "panels.open"],
        ["composerActions", "insert", "workspace.current"],
      ],
    ],
    [
      "issue-companion",
      [
        ["commands", "open", "panels.open"],
        ["composerActions", "browse", "composerViews.open"],
      ],
    ],
  ]) {
    // The examples handle their own host failures, including a rate-limited
    // notification, so the SDK never has to report one for them.
    const sdk = resolve(entry, "src/index.ts").replaceAll("\\", "/");
    const source = await bundle(
      `../../examples/addons/${example}/src/index.tsx`,
      {
        alias: { "@codemux/plugin-sdk": sdk },
        nodePaths: [resolve(entry, "node_modules")],
      },
    );
    const manifest = JSON.parse(
      await readFile(`examples/addons/${example}/manifest.json`, "utf8"),
    );
    const host = await start(source, manifest);
    await activated(host);
    for (let n = 0; n < 5; n++)
      for (const [kind, id, operation] of calls) {
        host.send("command.execute", { id, kind, context: "stale-context" });
        const call = await host.until((m) => m.method === "host.request");
        assert.equal(call.params.operation, operation);
        host.reject(call.id, "CONTEXT_STALE", "Context is no longer current");
        const notify = await host.until((m) => m.method === "host.request");
        assert.equal(notify.params.operation, "ui.notify");
        host.reject(notify.id, "RESOURCE_LIMIT", "Notification limit");
      }
    await sleep(200);
    assert.ok(host.alive(), `${example} must survive failing host calls`);
    assert.ok(
      !host.history.some(({ message }) => message.method === "log"),
      `${example} must handle its own host failures`,
    );
  }
  console.log(
    "PASS: example commands and composer actions handle failing host calls",
  );
  {
    // Author dependencies without an exports map, reading NODE_ENV.
    const modules = join(dir, "node_modules");
    await mkdir(join(modules, "main-only/lib"), { recursive: true });
    await writeFile(
      join(modules, "main-only/package.json"),
      JSON.stringify({ name: "main-only", main: "lib/index.js" }),
    );
    await writeFile(
      join(modules, "main-only/lib/index.js"),
      "module.exports = { mode: process.env.NODE_ENV };\n",
    );
    const source = await bundle("./tests/dependency.ts", {
      nodePaths: [modules],
    });
    assert.ok(!source.includes("process.env"));
    const host = await start(source, hello);
    await activated(host);
    await host.execute("hello");
    const notify = await host.until((m) => m.method === "host.request");
    assert.equal(notify.params.params.message, "production");
    console.log(
      "PASS: main-only CommonJS dependency and NODE_ENV run in the host",
    );
  }
} finally {
  for (const child of hosts) {
    child.kill();
    await new Promise((r) =>
      child.exitCode !== null || child.signalCode !== null
        ? r()
        : child.once("exit", r),
    );
  }
  await rm(dir, { recursive: true, force: true });
}
