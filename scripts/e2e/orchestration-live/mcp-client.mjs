// A small MCP client library for live scenarios: the outside doors an
// outside agent uses, driven from a script.
//
//   StdioMcpClient.spawn(env, argv, opts)  `codemux mcp …` inside the sandbox (sandbox-run.sh)
//   new HttpMcpClient(url, token)          the agent connector's /mcp
//   codemuxCli(env, args, opts)            any `codemux` verb on the host, against the root
//   toolResult(result)                     a tools/call result → its value, or throws McpToolError
//   settle(client, receipt)                poll operation_status until the receipt settles
//   finishRun(client, …)                   thread_wait until the run is no longer running
//
// Every request is id-correlated, so answers may arrive in any order; the
// stdio client records the order they arrived in, which scenarios use to
// prove one connection served calls concurrently.
import { execFile, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";

const here = dirname(fileURLToPath(import.meta.url));
export const SANDBOX_RUN = join(here, "sandbox-run.sh");
/** The revision these clients ask for (any supported one works). */
export const PROTOCOL_VERSION = "2025-06-18";

/** A tool call that answered `isError`: the shared `code: message` shape. */
export class McpToolError extends Error {
  constructor(code, message, result) {
    super(`${code}: ${message}`);
    this.code = code;
    this.detail = message;
    this.result = result;
  }
}

/** A JSON-RPC error instead of a result (unknown tool, refused name, …). */
export class McpRpcError extends Error {
  constructor(error) {
    super(`JSON-RPC ${error?.code}: ${error?.message}`);
    this.code = error?.code;
    this.data = error?.data;
  }
}

/** An HTTP status other than 200 from the connector. */
export class HttpStatusError extends Error {
  constructor(status, body) {
    super(`HTTP ${status}: ${JSON.stringify(body)}`);
    this.status = status;
    this.body = body;
  }
}

/** One door's tools/call result, the same shape on every door: `isError`
 *  and `structuredContent` (`error {code, message, retryable}` on failure).
 *  Legacy stdio tools answer text only, `code: message` on failure. */
export function toolResult(result) {
  const text = result?.content?.[0]?.text ?? "";
  if (result?.isError) {
    const error = result.structuredContent?.error;
    if (error?.code) throw new McpToolError(error.code, error.message ?? "", result);
    const bare = text.replace(/^Error: /, "");
    const at = bare.indexOf(": ");
    throw new McpToolError(at > 0 ? bare.slice(0, at) : "", at > 0 ? bare.slice(at + 2) : bare, result);
  }
  if (result?.structuredContent !== undefined && result.structuredContent !== null) return result.structuredContent;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** The isolated root's XDG layout, as drive.mjs `codemux()` and lib.sh `cmx` use it. */
export function xdgEnv(env) {
  return {
    XDG_CONFIG_HOME: `${env.ROOT}/config`,
    XDG_DATA_HOME: `${env.ROOT}/data`,
    XDG_CACHE_HOME: `${env.ROOT}/cache`,
    XDG_STATE_HOME: `${env.ROOT}/state`,
    XDG_RUNTIME_DIR: `${env.ROOT}/runtime`,
  };
}

/** `codemux <args>` on the host against the root (no sandbox; it reaches only
 *  the root's socket). Resolves to `{code, stdout, stderr}` whatever the exit
 *  code; rejects only when the command could not run or was aborted. */
export function codemuxCli(env, args, { signal, extraEnv = {}, input } = {}) {
  return new Promise((resolve, reject) => {
    const child = execFile(env.BIN, args, {
      signal,
      env: { HOME: process.env.HOME, PATH: "/usr/bin:/bin", ...xdgEnv(env), ...extraEnv },
      maxBuffer: 64 << 20,
    }, (error, stdout, stderr) => {
      if (error && typeof error.code !== "number") {
        reject(error);
        return;
      }
      resolve({ code: error ? error.code : 0, stdout, stderr });
    });
    if (input !== undefined) child.stdin.end(input);
    else child.stdin.end();
  });
}

/** `argv` inside the sandbox (sandbox-run.sh), to completion: `{code,
 *  stdout, stderr}` whatever the exit code. `input` is written to its stdin,
 *  which is then closed. The sandbox's `timeout -s TERM` stops it; an
 *  aborted `signal` closes its stdin, nothing more (never a kill). */
export function sandboxRun(env, argv, { signal, timeoutSec = 600, cwd, extraEnv = {}, input = "" } = {}) {
  const args = ["--root", env.ROOT, "--cwd", cwd ?? env.ROOT, "--timeout", String(timeoutSec)];
  for (const [key, value] of Object.entries(extraEnv)) args.push("--env", `${key}=${value}`);
  return new Promise((resolve, reject) => {
    const child = spawn(SANDBOX_RUN, [...args, "--", ...argv], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { HOME: process.env.HOME, PATH: process.env.PATH ?? "/usr/bin:/bin", USER: process.env.USER ?? "", LANG: process.env.LANG ?? "C.UTF-8" },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-200_000); });
    child.on("error", reject);
    child.on("close", (code, sig) => resolve({ code: code ?? sig, stdout, stderr }));
    signal?.addEventListener("abort", () => child.stdin.end(), { once: true });
    child.stdin.end(input);
  });
}

/** The JSON a CLI run printed, or null. */
export function printed(run) {
  try {
    return JSON.parse(run.stdout);
  } catch {
    return null;
  }
}

/** `codemux mcp …` as an outside client spawns it, inside the sandbox. */
export class StdioMcpClient {
  /** `argv` is the whole command, e.g. `[env.BIN, "mcp", "--client", "hermes"]`. */
  static spawn(env, argv, { signal, timeoutSec = 600, cwd, extraEnv = {} } = {}) {
    const args = ["--root", env.ROOT, "--cwd", cwd ?? env.ROOT, "--timeout", String(timeoutSec)];
    for (const [key, value] of Object.entries(extraEnv)) args.push("--env", `${key}=${value}`);
    const child = spawn(SANDBOX_RUN, [...args, "--", ...argv], {
      stdio: ["pipe", "pipe", "pipe"],
      // sandbox-run.sh builds the sandbox's own allow-listed env.
      env: { HOME: process.env.HOME, PATH: process.env.PATH ?? "/usr/bin:/bin", USER: process.env.USER ?? "", LANG: process.env.LANG ?? "C.UTF-8" },
    });
    return new StdioMcpClient(child, signal);
  }

  constructor(child, signal) {
    this.child = child;
    this.nextId = 0;
    this.waiting = new Map();
    this.early = new Map();
    /** Arrival order of every answer, by request id. */
    this.arrival = new Map();
    this.arrivals = 0;
    this.stderr = "";
    this.exited = new Promise((resolve) => child.on("exit", (code, sig) => resolve(code ?? sig)));
    createInterface({ input: child.stdout }).on("line", (line) => {
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        return;
      }
      if (message.id === undefined || message.id === null) return;
      this.arrival.set(message.id, (this.arrivals += 1));
      const waiter = this.waiting.get(message.id);
      if (waiter) {
        this.waiting.delete(message.id);
        waiter(message);
      } else {
        this.early.set(message.id, message);
      }
    });
    child.stderr.on("data", (chunk) => { this.stderr = (this.stderr + chunk).slice(-20_000); });
    // An aborted scenario hangs up, which is how a stdio server is stopped.
    signal?.addEventListener("abort", () => this.child.stdin.end(), { once: true });
    this.signal = signal;
  }

  write(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  /** Sends a request without waiting; returns its id. */
  send(method, params = {}) {
    const id = (this.nextId += 1);
    this.write({ jsonrpc: "2.0", id, method, params });
    return id;
  }

  notify(method, params = {}) {
    this.write({ jsonrpc: "2.0", method, params });
  }

  /** The order an answer arrived in (1-based), or undefined if it has not. */
  arrivedAt(id) {
    return this.arrival.get(id);
  }

  /** The answer to `id`, whatever order answers arrive in. */
  response(id, ms = 60_000) {
    if (this.early.has(id)) {
      const message = this.early.get(id);
      this.early.delete(id);
      return Promise.resolve(message);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting.delete(id);
        reject(new Error(`no answer to request ${id} within ${ms} ms; stderr: ${this.stderr.slice(-2_000)}`));
      }, ms);
      this.waiting.set(id, (message) => {
        clearTimeout(timer);
        resolve(message);
      });
    });
  }

  async request(method, params = {}, ms = 60_000) {
    this.signal?.throwIfAborted();
    const reply = await this.response(this.send(method, params), ms);
    if (reply.error) throw new McpRpcError(reply.error);
    return reply.result;
  }

  async initialize(clientInfo = { name: "orch-live", version: "1" }) {
    this.initialized = await this.request("initialize", { protocolVersion: PROTOCOL_VERSION, capabilities: {}, clientInfo }, 30_000);
    this.notify("notifications/initialized");
    return this.initialized;
  }

  async list() {
    return (await this.request("tools/list", {}, 30_000)).tools;
  }

  /** A tool's value, or throws McpToolError / McpRpcError. */
  async call(tool, args = {}, { timeoutMs = 120_000 } = {}) {
    return toolResult(await this.request("tools/call", { name: tool, arguments: args }, timeoutMs + 30_000));
  }

  /** Ends stdin and waits up to 10 s for the server to exit: its exit code,
   *  or null if it is still running (it then ends at sandbox-run's timeout). */
  async close() {
    this.child.stdin.end();
    return Promise.race([this.exited, sleep(10_000).then(() => null)]);
  }
}

/** The connector's /mcp with a bearer token. */
export class HttpMcpClient {
  constructor(url, token, { signal } = {}) {
    this.url = url;
    this.token = token;
    this.signal = signal;
    this.nextId = 0;
  }

  /** One JSON-RPC request: `{status, body}` (body null when not JSON). */
  async rpc(method, params = {}) {
    const response = await fetch(this.url, {
      method: "POST",
      signal: this.signal,
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        "mcp-protocol-version": PROTOCOL_VERSION,
        authorization: `Bearer ${this.token}`,
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: (this.nextId += 1), method, params }),
    });
    const text = await response.text();
    let body = null;
    try {
      body = JSON.parse(text);
    } catch { /* not JSON */ }
    return { status: response.status, body };
  }

  async request(method, params = {}) {
    const { status, body } = await this.rpc(method, params);
    if (status !== 200) throw new HttpStatusError(status, body);
    if (body?.error) throw new McpRpcError(body.error);
    return body?.result;
  }

  initialize(clientInfo = { name: "orch-live", version: "1" }) {
    return this.request("initialize", { protocolVersion: PROTOCOL_VERSION, capabilities: {}, clientInfo });
  }

  async list() {
    return (await this.request("tools/list", {})).tools;
  }

  async call(tool, args = {}) {
    return toolResult(await this.request("tools/call", { name: tool, arguments: args }));
  }

  /** The HTTP status of a `ping`. */
  async ping() {
    return (await this.rpc("ping", {})).status;
  }
}

/** Polls `operation_status` until the receipt is no longer accepted or running. */
export async function settle(client, receipt, { timeoutMs = 300_000, intervalMs = 500, signal } = {}) {
  const deadline = Date.now() + timeoutMs;
  let current = receipt;
  while (current?.state === "accepted" || current?.state === "running") {
    if (Date.now() > deadline) throw new Error(`operation ${receipt.operation_id} did not settle: ${JSON.stringify(current)}`);
    await sleep(intervalMs, undefined, { signal });
    current = await client.call("operation_status", { operation_id: receipt.operation_id });
  }
  return current;
}

/** The run a settled launch or send receipt started, if it names one. */
export function turnOf(receipt) {
  return receipt?.result?.turn?.turn_id || null;
}

/** `thread_wait` until the run is not running (or `timeoutMs` passes). */
export async function finishRun(client, { workspaceId, threadId, turnId = null, waitMs = 120_000, timeoutMs = 300_000, signal } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    signal?.throwIfAborted();
    const args = { workspace_id: workspaceId, thread_id: threadId, timeout_ms: waitMs };
    if (turnId) args.turn_id = turnId;
    const waited = await client.call("thread_wait", args, { timeoutMs: waitMs });
    if (waited.state !== "running") return waited;
    if (Date.now() > deadline) throw new Error(`run still running after ${timeoutMs} ms: ${JSON.stringify(waited)}`);
  }
}

/** The thread's status once its run needs input. A wait without `since` does
 *  not return for input already pending when it starts, so read the status,
 *  then wait for the next change. */
export async function untilInput(client, { workspaceId, threadId, timeoutMs = 300_000, signal } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    signal?.throwIfAborted();
    const status = await client.call("thread_status", { workspace_id: workspaceId, thread_id: threadId });
    if (status.state === "needs_input") return status;
    if (status.state !== "running") throw new Error(`the run ended (${status.state}) instead of asking: ${JSON.stringify(status)}`);
    if (Date.now() > deadline) throw new Error(`the run never asked for input: ${JSON.stringify(status)}`);
    await client.call("thread_wait", { workspace_id: workspaceId, thread_id: threadId, since: status.wait_cursor, timeout_ms: 60_000 },
      { timeoutMs: 60_000 });
  }
}
