import { AsyncLocalStorage } from "node:async_hooks";
import { realpath } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const MAX_BYTES = 1_048_576;
const writeProtocol = process.stdout.write.bind(process.stdout);
if (process.argv.includes("--check")) {
  writeProtocol('{"protocol":1,"runtime":"codemux-plugin-host"}\n');
  process.exit(0);
}
const scope = new AsyncLocalStorage();
const lifetime = new AbortController();
const handlers = new Map();
const invocations = new Map();
const pending = new Map();
let counter = 0, logCount = 0, initialized = false, ready = false, module;
function send(message) {
  try {
    const encoded = JSON.stringify(message);
    if (!encoded || Buffer.byteLength(encoded) > MAX_BYTES) return false;
    if (process.stdout.writableLength + Buffer.byteLength(encoded) > 4 * MAX_BYTES) return false;
    writeProtocol(`${encoded}\n`);
    return true;
  } catch { return false; }
}
function log(level, message) {
  if (logCount++ < 100) send({ type: "log", level: ["debug", "info", "warn", "error"].includes(level) ? level : "info", message: String(message).slice(0, 2000) });
}
// Stdout belongs to the protocol. Console output gets a separate bounded log channel.
process.stdout.write = function (chunk, encoding, callback) {
  log("info", Buffer.isBuffer(chunk) ? chunk.toString("utf8") : String(chunk));
  if (typeof encoding === "function") encoding();
  else callback?.();
  return true;
};
for (const level of ["debug", "info", "warn", "error", "log"]) console[level] = (...items) => log(level, items.join(" "));
process.on("unhandledRejection", () => log("warn", "Await host calls inside live plugin handlers."));
function closeInvocation(id) {
  const invocation = invocations.get(id);
  invocations.delete(id);
  invocation?.controller.abort();
  for (const [callId, call] of pending) if (call.invocationId === id) {
    pending.delete(callId);
    call.reject(new Error("Plugin invocation ended."));
  }
}
function hostCall(method, input) {
  const current = scope.getStore();
  if (!current || !invocations.has(current.id) || current.controller.signal.aborted || lifetime.signal.aborted)
    return Promise.reject(new Error("Host APIs require an active plugin invocation."));
  if (pending.size >= 128 || ++current.hostCalls > 4096)
    return Promise.reject(new Error("Plugin host call limit exceeded."));
  const callId = String(++counter);
  return new Promise((resolve, reject) => {
    pending.set(callId, { invocationId: current.id, resolve, reject });
    if (!send({ type: "host-call", invocationId: current.id, callId, method, input: input ?? null })) {
      pending.delete(callId);
      reject(new Error("Plugin host call exceeded the IPC limit or disconnected."));
    }
  });
}
const identifier = value => typeof value === "string" && value.length > 0 && value.length <= 256;
function freeze(value) {
  if (value && typeof value === "object") {
    for (const item of Object.values(value)) freeze(item);
    Object.freeze(value);
  }
  return value;
}
function result(value) {
  if (value === undefined || value === null) return {};
  if (typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid result");
  const output = {};
  if (value.message !== undefined) {
    if (typeof value.message !== "string" || value.message.length > 16_000) throw new Error("Invalid message");
    output.message = value.message;
  }
  if (value.data !== undefined) output.data = value.data;
  if (value.card !== undefined) {
    const card = value.card;
    if (!card || !identifier(card.viewId) || typeof card.summary !== "string" || !card.summary.length || card.summary.length > 1000) throw new Error("Invalid card");
    output.card = { viewId: card.viewId, summary: card.summary, ...(card.data === undefined ? {} : { data: card.data }) };
    if (Buffer.byteLength(JSON.stringify(output.card)) > 4096 || (card.data !== undefined && Buffer.byteLength(JSON.stringify(card.data)) > 2048)) throw new Error("Oversized card");
  }
  if (value.intents !== undefined) {
    if (!Array.isArray(value.intents) || value.intents.length > 20) throw new Error("Invalid intents");
    output.intents = [];
    let unsupported = 0;
    for (const intent of value.intents) {
      if (!intent || typeof intent.type !== "string") throw new Error("Invalid intent");
      switch (intent.type) {
        case "composer.insert":
          if (typeof intent.text !== "string" || intent.text.length > 64_000 || (intent.quote !== undefined && typeof intent.quote !== "boolean")) throw new Error("Invalid composer intent");
          output.intents.push({ type: intent.type, text: intent.text, ...(intent.quote === undefined ? {} : { quote: intent.quote }) }); break;
        case "preview.open":
          if (typeof intent.url !== "string" || intent.url.length > 4096 || !/^https?:\/\//.test(intent.url)) throw new Error("Invalid preview intent");
          output.intents.push({ type: intent.type, url: intent.url }); break;
        case "thread.reveal":
          if (!identifier(intent.threadId) || (intent.messageId !== undefined && !identifier(intent.messageId))) throw new Error("Invalid thread intent");
          output.intents.push({ type: intent.type, threadId: intent.threadId, ...(intent.messageId === undefined ? {} : { messageId: intent.messageId }) }); break;
        case "view.open":
          if (!identifier(intent.viewId)) throw new Error("Invalid view intent");
          output.intents.push({ type: intent.type, viewId: intent.viewId }); break;
        default: unsupported++;
      }
    }
    if (unsupported) output.unsupportedIntentCount = unsupported;
  }
  if (value.images !== undefined) {
    if (!Array.isArray(value.images) || value.images.length > 4) throw new Error("Invalid images");
    output.images = value.images.map(image => {
      if (!image || typeof image.path !== "string" || image.path.length > 512 || !["image/png", "image/jpeg"].includes(image.mimeType)) throw new Error("Invalid image reference");
      return { path: image.path, mimeType: image.mimeType };
    });
  }
  if (Buffer.byteLength(JSON.stringify(output)) > 128_000) throw new Error("Oversized result");
  return output;
}
async function invoke(message) {
  if (!ready || !identifier(message.invocationId) || !identifier(message.operationId) || !identifier(message.handler)
    || !message.caller || typeof message.caller !== "object" || invocations.has(message.invocationId) || invocations.size >= 8) {
    send({ type: "failure", invocationId: message.invocationId }); return;
  }
  const handler = handlers.get(message.handler);
  if (!handler) { send({ type: "failure", invocationId: message.invocationId, message: "Plugin handler is unavailable." }); return; }
  const current = { id: message.invocationId, controller: new AbortController(), hostCalls: 0 };
  invocations.set(current.id, current);
  const invocation = Object.freeze({ signal: current.controller.signal, operationId: message.operationId,
    caller: freeze(message.caller), ...(message.context === undefined ? {} : { context: freeze(message.context) }) });
  try {
    let value;
    try { value = await scope.run(current, () => handler(message.input, invocation)); }
    catch (error) {
      let message;
      try { if (error instanceof Error && typeof error.message === "string") message = error.message.trim().slice(0, 2000) || undefined; } catch {}
      if (!current.controller.signal.aborted) send({ type: "failure", invocationId: current.id, ...(message ? { message } : {}) });
      return;
    }
    if (!current.controller.signal.aborted && !send({ type: "result", invocationId: current.id, result: result(value) }))
      send({ type: "failure", invocationId: current.id });
  } catch {
    if (!current.controller.signal.aborted) send({ type: "failure", invocationId: current.id });
  } finally { closeInvocation(current.id); }
}
async function init(message) {
  if (initialized) throw new Error("Repeated initialization");
  initialized = true;
  if (message.protocol !== 1 || typeof message.directory !== "string" || !path.isAbsolute(message.directory)
    || typeof message.entry !== "string" || message.entry.includes("\\") || message.entry.includes(":")
    || message.entry.split("/").some(p => !p || p === "." || p === "..") || !Array.isArray(message.capabilities)) throw new Error("Invalid initialization");
  const directory = await realpath(message.directory);
  const entry = await realpath(path.join(directory, message.entry));
  if (!entry.startsWith(directory + path.sep)) throw new Error("Entry escapes package");
  const handle = (name, handler) => {
    if (ready || handlers.size >= 256 || handlers.has(name) || typeof handler !== "function"
      || !/^(?:action:[\w.-]+|status:[\w.-]+|tree:[\w.-]+|view:[\w.-]+:[\w.-]+|event:run\.finalized)$/.test(name)) throw new Error("Invalid handler registration");
    handlers.set(name, handler);
  };
  const methods = names => Object.fromEntries(names.map(name => [name.split(".").at(-1), input => hostCall(name, input ?? {})]));
  const context = Object.freeze({ apiVersion: 1, signal: lifetime.signal, dataDirectory: message.dataDirectory,
    capabilities: Object.freeze([...message.capabilities]), handle, log,
    settings: { get: key => hostCall("settings.get", { key }) },
    secrets: { get: key => hostCall("secrets.get", { key }) },
    storage: { get: key => hostCall("storage.get", { key }), set: (key, value) => hostCall("storage.set", { key, value }), delete: key => hostCall("storage.delete", { key }) },
    projects: methods(["projects.list"]), workspaces: methods(["workspaces.list"]), models: methods(["models.list"]),
    threads: methods(["threads.list", "threads.metadata", "threads.read", "threads.send", "threads.create"]),
    workflow: { query: input => hostCall("workflow.query", input) },
    ui: { invalidate: topics => hostCall("ui.invalidate", { topics }) },
    trees: { register: (id, handler) => handle(`tree:${id}`, async (_input, invocation) => ({ data: await handler(invocation) })) },
    events: { onRunFinalized: handler => handle("event:run.finalized", handler) },
  });
  const imported = await import(pathToFileURL(entry).href);
  module = typeof imported.activate === "function" ? imported : imported.default;
  if (!module || typeof module.activate !== "function") throw new Error("Export activate(context) from the backend entry.");
  await module.activate(context);
  ready = true;
  send({ type: "ready", protocol: 1 });
}
let stopping = false;
async function shutdown() {
  if (stopping) return;
  stopping = true;
  lifetime.abort();
  for (const id of invocations.keys()) closeInvocation(id);
  try { await module?.deactivate?.(); } finally { process.exit(0); }
}
function receive(message) {
  if (!message || typeof message !== "object") throw new Error("Malformed protocol");
  switch (message.type) {
    case "init": void init(message).catch(() => { send({ type: "failure" }); void shutdown(); }); break;
    case "invoke": void invoke(message); break;
    case "host-result": {
      const call = pending.get(message.callId);
      if (call && call.invocationId === message.invocationId) {
        pending.delete(message.callId);
        if (message.ok) call.resolve(message.value);
        else call.reject(Object.assign(new Error(message.error?.message ?? "The host rejected this plugin operation."), message.error ?? {}));
      }
      break;
    }
    case "cancel": closeInvocation(message.invocationId); send({ type: "cancelled", invocationId: message.invocationId }); break;
    case "stop": void shutdown(); break;
    default: throw new Error("Unknown protocol message");
  }
}
let input = Buffer.alloc(0);
process.stdin.on("data", chunk => {
  input = Buffer.concat([input, chunk]);
  let newline;
  while ((newline = input.indexOf(10)) !== -1) {
    if (newline > MAX_BYTES) { process.exit(2); return; }
    const line = input.subarray(0, newline); input = input.subarray(newline + 1);
    try { receive(JSON.parse(line.toString("utf8"))); } catch { process.exit(2); return; }
  }
  if (input.length > MAX_BYTES) process.exit(2);
});
process.stdin.once("end", () => { void shutdown(); });
process.once("SIGTERM", () => { void shutdown(); });
send({ type: "booted", protocol: 1 });
