// Outside agents drive Codemux end to end through every outside door: a
// scripted MCP client over `codemux mcp` (the guide's end-to-end flow), the
// network connector with a personal token and an OAuth sign-in, the CLI
// verbs, an in-app agent's own shell, a real outside agent (`claude -p`),
// the user-scope setup seen from in-app chats, and optionally Hermes. Each
// creates a worktree, launches a thread, waits, reads the reply and closes
// up; the store and the agent's own tool trace must agree.
//
//   ORCH_LIVE_OUTSIDE_PARTS=flow,http,cli,agent-shell,claude,user-scope,hermes
//
// Default: every part but hermes, which needs ORCH_LIVE_PROVIDERS to include
// hermes. Unknown or empty selections are errors. The connector part runs it
// on a free port and restores the previous settings afterwards.
//
// Process rules: `codemux mcp` children stop when their stdin closes;
// sandboxed agents stop at sandbox-run.sh's `timeout -s TERM`. Nothing is
// ever killed, and no credential file is copied or written.
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import {
  HttpMcpClient, HttpStatusError, McpRpcError, McpToolError, StdioMcpClient, codemuxCli, finishRun, printed,
  sandboxRun, settle, toolResult, turnOf, untilInput,
} from "../mcp-client.mjs";

export const name = "outside-agent";
export const description =
  "Outside MCP clients (scripted stdio, HTTP token and OAuth, CLI, claude -p, optional Hermes) create a worktree, " +
  "launch a Codex thread, wait, read and report; in-app agents are refused admin commands; the store and the traces agree.";
/** Seven parts with real model turns; run.mjs honours this over its 10 min. */
export const budgetMs = 1_500_000;

const CLAUDE_MODEL = process.env.ORCH_LIVE_CLAUDE_MODEL ?? "claude-haiku-4-5";
const CODEX_MODEL = process.env.ORCH_LIVE_CODEX_MODEL ?? "gpt-6-luna";
const PARTS = ["flow", "http", "cli", "agent-shell", "claude", "user-scope", "hermes"];
const DEFAULT_PARTS = PARTS.filter((part) => part !== "hermes");

const q = (s) => `'${String(s).replaceAll("'", "''")}'`;
const tagOf = () => randomUUID().slice(0, 8);

function selectedParts(env) {
  const raw = process.env.ORCH_LIVE_OUTSIDE_PARTS;
  const wanted = raw === undefined ? DEFAULT_PARTS : raw.split(",").map((part) => part.trim()).filter(Boolean);
  const unknown = wanted.filter((part) => !PARTS.includes(part));
  if (unknown.length) throw new Error(`ORCH_LIVE_OUTSIDE_PARTS: unknown part(s) ${unknown.join(", ")} (known: ${PARTS.join(", ")})`);
  if (!wanted.length) throw new Error("ORCH_LIVE_OUTSIDE_PARTS selects no part");
  const providers = (env.ORCH_LIVE_PROVIDERS ?? "").split(",");
  if (wanted.includes("hermes") && !providers.includes("hermes")) {
    throw new Error("part hermes needs ORCH_LIVE_PROVIDERS to include hermes (up.sh mounts Hermes only then)");
  }
  return wanted;
}

/** A prompt whose expected reply never appears in the prompt itself. */
function challenge(tag) {
  const a = 11 + Math.floor(Math.random() * 40);
  const b = 11 + Math.floor(Math.random() * 40);
  const expected = `OUTSIDE-${a + b}-${tag}`;
  const text = `Compute ${a} plus ${b}. Reply with exactly one token and nothing else: the word OUTSIDE, a hyphen, ` +
    `the sum as digits, a hyphen, then ${tag}.`;
  if (text.includes(expected)) throw new Error("challenge contains its own answer");
  return { text, expected };
}

/** A sandbox path under HOME, as the host sees it (<root>/home). */
const onHost = (ctx, path) => path.replace(process.env.HOME, `${ctx.env.ROOT}/home`);

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

/** Every process below the sandbox init, read from the host's /proc. */
function below(initPid) {
  const children = new Map();
  for (const pid of readdirSync("/proc").filter((p) => /^\d+$/.test(p))) {
    try {
      const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
      const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
      if (!children.has(ppid)) children.set(ppid, []);
      children.get(ppid).push(Number(pid));
    } catch { /* exited */ }
  }
  const found = [];
  const stack = [Number(initPid)];
  while (stack.length) {
    for (const child of children.get(stack.pop()) ?? []) { found.push(child); stack.push(child); }
  }
  return found;
}

const cmdline = (pid) => {
  try {
    return readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
  } catch {
    return [];
  }
};

/** Pids of agent commands whose argv contains `marker` (a unique sleep). */
const running = (marker) => readdirSync("/proc").filter((pid) => /^\d+$/.test(pid) && cmdline(pid).some((arg) => arg.includes(marker)));

/** A chat pane bound to a fresh thread with a started session and no turn,
 *  through the paired browser (a receiver costs no model call). */
async function openChat(ctx, { provider, model = null, permissionMode = null }) {
  const threadId = randomUUID();
  const paneId = await ctx.driver.invoke("agent_chat_create_pane", {
    workspaceId: ctx.env.WORKSPACE_ID, provider, cwd: ctx.env.REPO, launchMode: "new_tab", threadId,
  });
  await ctx.driver.invoke("agent_chat_start_session", {
    paneId, provider, expectedThread: null,
    input: { thread_id: threadId, cwd: ctx.env.REPO, model, resume_cursor: null, permission_mode: permissionMode, effort: null,
      context_window: null, service_tier: "default", additional_directories: [], env: null, extra: null },
  });
  return { threadId, paneId };
}

/** The tab holding `paneId` in any workspace (ctx.tabOf reads only the seeded one). */
async function tabIn(ctx, workspaceId, paneId) {
  const state = await ctx.codemux("get_app_state", {});
  const workspace = state.workspaces.find((w) => w.workspace_id === workspaceId);
  const contains = (node) => !!node && (node.pane_id === paneId
    || Object.values(node).some((child) => child && typeof child === "object" && contains(child)));
  const surface = workspace?.surfaces.find((s) => contains(s.root));
  return workspace?.tabs.find((t) => t.surface_id === surface?.surface_id)?.tab_id ?? null;
}

const workspaceIds = async (ctx) => (await ctx.codemux("get_app_state", {})).workspaces.map((w) => w.workspace_id);

/** Closes a worktree workspace and its checkout from the owner, as the
 *  scenarios' `finally` blocks do, ignoring one that is already gone. */
async function ownerClose(ctx, workspaceId) {
  if (!workspaceId || !(await workspaceIds(ctx)).includes(workspaceId)) return;
  await ctx.codemux("close_workspace", { workspace_id: workspaceId, delete_worktree: true }).catch((error) => {
    ctx.observe(`cleanup ${workspaceId}`, error.message);
  });
}

/** `workspace_close` with the documented retry: an agent still stopping
 *  refuses the delete once; retry after it settles. */
async function closeWorktree(ctx, client, workspaceId, key) {
  const args = { workspace_id: workspaceId, client_request_id: key, delete_worktree: true };
  try {
    return await settle(client, await client.call("workspace_close", args), { signal: ctx.signal });
  } catch (error) {
    if (!(error instanceof McpToolError) || !/still stopping|did not confirm/.test(error.message)) throw error;
    ctx.observe(`close ${workspaceId} retried`, error.message);
    await sleep(5_000, undefined, { signal: ctx.signal });
    return settle(client, await client.call("workspace_close", { ...args, client_request_id: `${key}-retry` }), { signal: ctx.signal });
  }
}

async function receiptsFor(ctx, tag) {
  return ctx.assert.sql(`SELECT principal, tool, state, workspace_id, thread_id, request_key FROM agent_control_operations ` +
    `WHERE request_key LIKE ${q(`%-${tag}`)} ORDER BY created_at`);
}

// ── flow: the guide's end-to-end flow over one stdio connection ───────────

async function flow(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  const outside = join(env.ROOT, "outside");
  mkdirSync(outside, { recursive: true });
  const client = StdioMcpClient.spawn(env, [env.BIN, "mcp", "--client", "hermes"], { signal, timeoutSec: 900, cwd: outside });
  let workspaceId = null;
  try {
    // --client names the principal; clientInfo does not override it.
    const init = await client.initialize({ name: "orch-live-script", version: "1" });
    ctx.check("flow: initialize answers as codemux with instructions", init?.serverInfo?.name === "codemux" && !!init.instructions, init?.serverInfo);
    const tools = (await client.list()).map((tool) => tool.name);
    const steps = ["workspace_list", "worktree_create", "operation_status", "agent_capabilities", "thread_launch", "thread_wait",
      "thread_read", "thread_send", "thread_status", "thread_interrupt", "workspace_close"];
    ctx.check("flow: tools/list offers every step's tool, each once",
      steps.every((tool) => tools.includes(tool)) && new Set(tools).size === tools.length, { count: tools.length });
    const listed = await client.call("workspace_list", {});
    ctx.check("flow: workspace_list has the seeded workspace", listed.workspaces.some((w) => w.workspace_id === env.WORKSPACE_ID));

    const created = await settle(client, await client.call("worktree_create", {
      repo_path: env.REPO, branch: `outside-flow-${tag}`, client_request_id: `of-wt-${tag}`,
    }), { signal });
    workspaceId = created.workspace_id;
    const checkout = onHost(ctx, created.cwd);
    ctx.check("flow: worktree_create made a checkout from the default branch",
      created.state === "succeeded" && created.base_branch === "main" && existsSync(checkout), { created, checkout });

    const { text, expected } = challenge(tag);
    const launchArgs = { workspace_id: workspaceId, provider: "codex", model: CODEX_MODEL, effort: "low", permission_mode: "workspace-write",
      title: `Outside flow ${tag}`, message: text, client_request_id: `of-launch-${tag}` };
    const receipt = await client.call("thread_launch", launchArgs);
    const again = await client.call("thread_launch", launchArgs);
    ctx.check("flow: an identical retry returns the same operation", again.operation_id === receipt.operation_id, { receipt, again });
    const launched = await settle(client, receipt, { signal });
    ctx.check("flow: the launch settled", launched.state === "succeeded", launched);
    const threadId = launched.thread_id;
    ctx.observe("flow thread", { workspaceId, threadId, expected });
    // Listing reads cached catalogs only; after the launch Codex's is known.
    const caps = await client.call("agent_capabilities", { workspace_id: workspaceId, provider: "codex" });
    const codex = caps.providers?.[0];
    ctx.check("flow: agent_capabilities advertises the model and workspace-write",
      JSON.stringify(codex?.capabilities ?? {}).includes(CODEX_MODEL) && codex.allowed_permission_modes.includes("workspace-write"),
      { modes: codex?.allowed_permission_modes, error: codex?.discovery_error });

    const done = await finishRun(client, { workspaceId, threadId, turnId: turnOf(launched), signal });
    ctx.check("flow: thread_wait returns done with the computed token",
      done.state === "done" && (done.final_text ?? "").includes(expected), { state: done.state, failure: done.failure, final_text: done.final_text });
    const page = await client.call("thread_read", { workspace_id: workspaceId, thread_id: threadId });
    ctx.check("flow: thread_read has the reply", page.messages.some((m) => m.role === "assistant" && m.content.includes(expected)));
    const receipts = await receiptsFor(ctx, tag);
    ctx.check("flow: the receipts belong to local:hermes",
      ["worktree_create", "thread_launch"].every((tool) => receipts.some((r) => r.tool === tool))
        && receipts.every((r) => r.principal === "local:hermes"), receipts);
    const [first] = await ctx.assert.sql(`SELECT json_extract(payload,'$.created_by') AS created_by, ` +
      `json_extract(payload,'$.creation_source') AS source, json_extract(payload,'$.sender_title') AS sender ` +
      `FROM agent_chat_messages WHERE thread_id=${q(threadId)} AND json_extract(payload,'$.type')='user_message' ORDER BY id LIMIT 1`);
    ctx.check("flow: the first message is attributed to the outside assistant",
      first?.created_by === "agent" && first?.source === "mcp" && first?.sender === "hermes", first);

    await provenanceShot(ctx, workspaceId, launched.result?.pane_id);
    await agentEnvironment(ctx);

    // Interrupt during a wait, on the same connection.
    const SLEEP = "time.sleep(240.0173)";
    const sleeping = await settle(client, await client.call("thread_send", {
      workspace_id: workspaceId, thread_id: threadId, client_request_id: `of-sleep-${tag}`,
      message: `Run this exact shell command with your shell tool and wait for it to finish: python3 -c 'import time; ${SLEEP}'. Then reply DONE.`,
    }), { signal });
    const turn = turnOf(sleeping);
    // The command runs, or Codex asks to run it; either way the run is busy.
    await ctx.waitFor(async () => running(SLEEP).length > 0
      || (await client.call("thread_status", { workspace_id: workspaceId, thread_id: threadId })).state === "needs_input" || null,
    { timeout: 240_000, interval: 2_000, label: "the agent runs its command" });
    const wait = client.send("tools/call", { name: "thread_wait",
      arguments: { workspace_id: workspaceId, thread_id: threadId, turn_id: turn, timeout_ms: 180_000 } });
    const interrupt = client.send("tools/call", { name: "thread_interrupt",
      arguments: { workspace_id: workspaceId, thread_id: threadId, turn_id: turn, client_request_id: `of-int-${tag}` } });
    toolResult((await client.response(interrupt, 30_000)).result);
    const interruptedAt = Date.now();
    const ended = toolResult((await client.response(wait, 60_000)).result);
    ctx.check("flow: the interrupt's receipt came before the wait's answer", client.arrivedAt(interrupt) < client.arrivedAt(wait));
    ctx.check("flow: the parked wait reports the interrupt within 20 s",
      ended.state === "failed" && ended.failure === "interrupted" && Date.now() - interruptedAt < 20_000,
      { state: ended.state, failure: ended.failure, ms: Date.now() - interruptedAt });

    // A cancelled wait gets no answer; the connection keeps serving.
    const SLEEP2 = "time.sleep(240.0179)";
    const held = await settle(client, await client.call("thread_send", {
      workspace_id: workspaceId, thread_id: threadId, client_request_id: `of-sleep2-${tag}`,
      message: `Run this exact shell command with your shell tool and wait for it to finish: python3 -c 'import time; ${SLEEP2}'. Then reply DONE.`,
    }), { signal });
    // Park the wait only once the run is busy: its start, or an approval
    // opening after the wait, would answer a wait that the cancel then misses.
    await ctx.waitFor(async () => running(SLEEP2).length > 0
      || (await client.call("thread_status", { workspace_id: workspaceId, thread_id: threadId })).state === "needs_input" || null,
    { timeout: 240_000, interval: 2_000, label: "the agent runs its second command" });
    const cancelled = client.send("tools/call", { name: "thread_wait",
      arguments: { workspace_id: workspaceId, thread_id: threadId, turn_id: turnOf(held), timeout_ms: 60_000 } });
    await sleep(500, undefined, { signal });
    client.notify("notifications/cancelled", { requestId: cancelled, reason: "orch-live" });
    const ping = await client.response(client.send("ping", {}), 5_000);
    ctx.check("flow: ping answers after a cancelled wait", !!ping.result && !ping.error, ping);
    await settle(client, await client.call("thread_interrupt", {
      workspace_id: workspaceId, thread_id: threadId, turn_id: turnOf(held), client_request_id: `of-int2-${tag}`,
    }), { signal });
    await finishRun(client, { workspaceId, threadId, turnId: turnOf(held), signal });
    // A wait that was never cancelled answers when its run ends, so only
    // after the interrupt can silence prove the cancel. The pause covers an
    // answer written just after finishRun's own.
    await sleep(1_000, undefined, { signal });
    ctx.check("flow: the cancelled wait stayed unanswered after its run ended", client.arrivedAt(cancelled) === undefined);

    // `since` returns at its timeout when nothing changes, without spinning.
    const status = await client.call("thread_status", { workspace_id: workspaceId, thread_id: threadId });
    const quietFrom = Date.now();
    const quiet = await client.call("thread_wait", { workspace_id: workspaceId, thread_id: threadId, since: status.wait_cursor, timeout_ms: 2_000 });
    const quietMs = Date.now() - quietFrom;
    ctx.check("flow: a since-wait with nothing new times out at about 2 s", quiet.timed_out === true && quietMs >= 1_500 && quietMs < 10_000, { quietMs });

    // The socket's own thread_wait takes the 600 s maximum.
    const short = challenge(tag);
    const sent = await settle(client, await client.call("thread_send", {
      workspace_id: workspaceId, thread_id: threadId, message: short.text, client_request_id: `of-short-${tag}`,
    }), { signal });
    const socketWait = await codemuxCli(env, ["json", "thread_wait", JSON.stringify({
      workspace_id: workspaceId, thread_id: threadId, turn_id: turnOf(sent), timeout_ms: 600_000,
    })], { signal });
    const socketReply = printed(socketWait);
    ctx.check("flow: `codemux json thread_wait` with timeout_ms 600000 returns on completion",
      socketWait.code === 0 && socketReply?.data?.state === "done" && (socketReply.data.final_text ?? "").includes(short.expected),
      { code: socketWait.code, state: socketReply?.data?.state, final_text: socketReply?.data?.final_text });

    const closed = await closeWorktree(ctx, client, workspaceId, `of-close-${tag}`);
    ctx.check("flow: workspace_close removed the worktree", closed.worktree_removed === true && !existsSync(checkout), closed);
    workspaceId = null;
    ctx.check("flow: codemux mcp exits 0 when the client hangs up", (await client.close()) === 0);
  } finally {
    await client.close();
    await ownerClose(ctx, workspaceId);
  }
}

/** The paired browser shows the outside assistant's message with its label. */
async function provenanceShot(ctx, workspaceId, paneId) {
  await ctx.driver.clickSelector('[aria-label="Activity view"]').catch(() => {});
  const card = `[data-inbox-card="${workspaceId}"]`;
  await ctx.waitFor(`!!document.querySelector('${card}')`, { timeout: 30_000, label: "the worktree's card" });
  await ctx.driver.clickSelector(card);
  const tab = paneId ? await tabIn(ctx, workspaceId, paneId) : null;
  if (tab) await ctx.openTab(tab);
  const label = `[...document.querySelectorAll('[data-user-message-attribution]')]
    .some((el) => el.offsetParent !== null && el.getAttribute('data-user-message-attribution') === 'outside' && el.innerText.includes('hermes'))`;
  await ctx.waitFor(label, { timeout: 60_000, label: "the outside-assistant label" });
  ctx.check("flow: the paired browser labels the message as sent by hermes, an outside assistant", true);
  await ctx.shot("outside-agent-provenance");
}

/** A live agent process's marker, replayed on a host CLI call, is refused
 *  an admin command (the env-marker path; agent-shell covers ancestry).
 *  Only the two variables the socket reads as a marker are replayed: the
 *  per-chat gateway pair is not one, so a process that carries only that
 *  would make the host CLI the owner. The probe changes nothing, so a miss
 *  cannot pair a device or touch the connector. */
async function agentEnvironment(ctx) {
  const MARKER = /^(CODEMUX_AGENT_PROCESS|CODEMUX_MCP_PROVIDER_ORIGIN)=/;
  let found = null;
  for (const pid of below(ctx.env.INIT_PID)) {
    let environ;
    try {
      environ = readFileSync(`/proc/${pid}/environ`, "utf8").split("\0");
    } catch {
      continue; // not dumpable (Codex marks itself so)
    }
    const vars = Object.fromEntries(environ.filter((entry) => MARKER.test(entry))
      .map((entry) => [entry.slice(0, entry.indexOf("=")), entry.slice(entry.indexOf("=") + 1)]));
    if (Object.keys(vars).length) {
      found = { pid, argv0: cmdline(pid)[0], vars };
      break;
    }
  }
  if (!found) {
    ctx.observe("flow: agent environment", "no agent process exposed its marker; the Rust contract test covers the mechanism");
    return;
  }
  const refused = await codemuxCli(ctx.env, ["json", "agent_connector_status", "{}"], { signal: ctx.signal, extraEnv: found.vars });
  ctx.observe("flow: agent environment from", { pid: found.pid, argv0: found.argv0, vars: Object.keys(found.vars) });
  ctx.check("flow: an agent process's marker is refused agent_connector_status",
    refused.code === 1 && refused.stdout.includes("admin_refused"), { code: refused.code, stdout: refused.stdout.slice(0, 300) });
}

// ── http: the network door ────────────────────────────────────────────────

async function http(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  const before = await ctx.codemux("agent_connector_status", {});
  const label = `live-${tag}`;
  let stdio = null;
  try {
    const port = await freePort();
    const enabled = await codemuxCli(env, ["connector", "enable", "--port", String(port), "--json"], { signal });
    const status = printed(enabled)?.result;
    ctx.check("http: the connector listens on its own port", enabled.code === 0 && status?.listenerRunning === true && status.port === port,
      { code: enabled.code, stdout: enabled.stdout.slice(0, 300) });
    const url = status.localUrl;

    const issued = printed(await codemuxCli(env, ["connector", "issue", "--label", label, "--access", "supervised", "--json"], { signal }));
    const token = issued?.result?.token;
    ctx.check("http: a supervised personal token is issued", typeof token === "string" && token.startsWith("cmcp_") && issued.result.access === "supervised",
      { label: issued?.result?.label, access: issued?.result?.access });
    const client = new HttpMcpClient(url, token, { signal });
    await client.initialize({ name: "orch-live-http", version: "1" });

    // Parity with a stdio client at the same access.
    stdio = StdioMcpClient.spawn(env, [env.BIN, "mcp", "--client", `parity-${tag}`, "--access", "supervised"], { signal, timeoutSec: 300 });
    await stdio.initialize();
    const [overHttp, overStdio] = [await client.list(), await stdio.list()];
    const differs = overHttp.filter((tool) => {
      const twin = overStdio.find((other) => other.name === tool.name);
      return !twin || JSON.stringify(twin.inputSchema) !== JSON.stringify(tool.inputSchema)
        || JSON.stringify(twin.annotations) !== JSON.stringify(tool.annotations);
    });
    ctx.check("http: every HTTP tool is on stdio with the same schema and hints", overHttp.length > 0 && differs.length === 0,
      differs.map((tool) => tool.name));
    await stdio.close();
    stdio = null;

    const { text, expected } = challenge(tag);
    const launched = await settle(client, await client.call("thread_launch", {
      workspace_id: env.WORKSPACE_ID, provider: "codex", model: CODEX_MODEL, effort: "low", permission_mode: "read-only",
      title: `Outside http ${tag}`, message: text, client_request_id: `oh-launch-${tag}`,
    }), { signal, intervalMs: 1_000 });
    const done = await finishRun(client, { workspaceId: env.WORKSPACE_ID, threadId: launched.thread_id, turnId: turnOf(launched), signal });
    const page = await client.call("thread_read", { workspace_id: env.WORKSPACE_ID, thread_id: launched.thread_id });
    ctx.check("http: the launched thread's reply comes back through the token",
      done.state === "done" && (done.final_text ?? "").includes(expected) && page.messages.some((m) => m.content.includes(expected)),
      { state: done.state, final_text: done.final_text });
    const refused = await client.call("worktree_create", { repo_path: env.REPO, branch: `oh-${tag}`, client_request_id: `oh-wt-${tag}` })
      .then(() => null, (error) => error);
    ctx.check("http: a supervised token cannot create worktrees", refused instanceof McpToolError && refused.code === "access_denied",
      refused?.message);

    const revoked = await codemuxCli(env, ["connector", "revoke", label, "--json"], { signal });
    const after = await client.call("workspace_list", {}).then(() => null, (error) => error);
    ctx.check("http: a revoked token gets 401", revoked.code === 0 && after instanceof HttpStatusError && after.status === 401,
      { revoke: revoked.code, after: after?.message });

    await oauth(ctx, url, tag);
  } finally {
    await stdio?.close();
    await codemuxCli(env, ["connector", "revoke", label, "--json"], {}).catch(() => {});
    // Back to the settings the root had.
    await ctx.codemux("agent_connector_set_config", { enabled: before.enabled, public_origin: before.publicOrigin ?? null, port: before.port })
      .catch((error) => ctx.observe("http: restoring the connector settings", error.message));
  }
}

/** A scripted OAuth sign-in the owner approves with `codemux connector
 *  approve`, then refresh rotation and replay revoking the family. */
async function oauth(ctx, url, tag) {
  const { env, signal } = ctx;
  const issuer = new URL(url).origin;
  const callback = "https://client.example/cb";
  // RFC 7636 appendix B.
  const verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
  const challengeCode = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
  const form = (path, fields, headers = {}) => fetch(`${issuer}${path}`, {
    method: "POST", redirect: "manual", signal, headers, body: new URLSearchParams(fields),
  });

  const registered = await (await fetch(`${issuer}/oauth/mcp/register`, {
    method: "POST", signal, headers: { "content-type": "application/json" },
    body: JSON.stringify({ client_name: `orch-live-${tag}`, redirect_uris: [callback], grant_types: ["authorization_code", "refresh_token"] }),
  })).json();
  const clientId = registered.client_id;
  const authorize = new URL(`${issuer}/oauth/mcp/authorize`);
  for (const [key, value] of Object.entries({ client_id: clientId, redirect_uri: callback, response_type: "code", resource: url,
    code_challenge_method: "S256", code_challenge: challengeCode })) authorize.searchParams.set(key, value);
  const waiting = await fetch(authorize, { redirect: "manual", signal });
  const cookie = (waiting.headers.get("set-cookie") ?? "").split(";")[0];
  const requestId = cookie.split("=")[0].replace(/^cmcp_wait_/, "");
  const nonce = /name="nonce" value="([^"]+)"/.exec(await waiting.text())?.[1];
  const complete = () => form("/oauth/mcp/complete", { request_id: requestId, nonce }, { origin: issuer, cookie });
  const parked = await complete();
  ctx.check("http: an OAuth sign-in waits for the owner", waiting.status === 200 && parked.status === 202 && !!nonce,
    { authorize: waiting.status, complete: parked.status });
  const approved = await codemuxCli(env, ["connector", "approve", requestId, "--access", "supervised", "--json"], { signal });
  const redirect = await complete();
  const code = new URL(redirect.headers.get("location") ?? "about:blank").searchParams.get("code");
  ctx.check("http: `codemux connector approve` lets the sign-in finish", approved.code === 0 && redirect.status === 303 && !!code,
    { approve: approved.code, complete: redirect.status });
  const tokens = await (await form("/oauth/mcp/token", { grant_type: "authorization_code", client_id: clientId, code,
    redirect_uri: callback, resource: url, code_verifier: verifier })).json();
  const refresh = (token) => form("/oauth/mcp/token", { grant_type: "refresh_token", client_id: clientId, refresh_token: token });
  const first = new HttpMcpClient(url, tokens.access_token, { signal });
  await first.initialize();

  // The access token lapses at its TTL (CODEMUX_E2E_CONNECTOR_ACCESS_TTL_SECS
  // in the serve's env shortens it in debug builds).
  if (tokens.expires_in <= 60) {
    await ctx.waitFor(async () => (await first.ping()) === 401 || null,
      { timeout: (tokens.expires_in + 30) * 1_000, interval: 2_000, label: "the access token lapses" });
    ctx.check(`http: the access token lapses after its ${tokens.expires_in} s TTL`, true);
  } else {
    ctx.observe("http: access token TTL", `${tokens.expires_in} s: the serve runs without CODEMUX_E2E_CONNECTOR_ACCESS_TTL_SECS, so expiry is not exercised`);
  }
  const rotated = await refresh(tokens.refresh_token);
  const second = await rotated.json();
  const renewed = new HttpMcpClient(url, second.access_token, { signal });
  ctx.check("http: the refresh token rotates both tokens",
    rotated.status === 200 && second.refresh_token !== tokens.refresh_token && second.access_token !== tokens.access_token
      && (await renewed.ping()) === 200, { status: rotated.status });
  // Past the 30 s reuse grace, a spent refresh token revokes the family: its
  // refresh tokens and its live access tokens. The sleep outlasts a short
  // access-token TTL, so the access token checked is one renewed just before
  // the replay and seen working: its 401 then comes from the revocation, not
  // from expiry.
  await sleep(31_000, undefined, { signal });
  const third = await (await refresh(second.refresh_token)).json();
  const live = new HttpMcpClient(url, third.access_token, { signal });
  const liveBefore = await live.ping();
  const replayed = await refresh(tokens.refresh_token);
  const replayBody = await replayed.json().catch(() => null);
  const liveAfter = await live.ping();
  const refreshAfter = (await refresh(third.refresh_token)).status;
  const familyGone = liveBefore === 200 && liveAfter === 401 && refreshAfter === 400;
  ctx.check("http: reusing a spent refresh token after 31 s revokes the sign-in",
    replayed.status === 400 && replayBody?.error === "invalid_grant" && familyGone,
    { status: replayed.status, body: replayBody, liveBefore, liveAfter, refreshAfter });
}

// ── cli: the CLI verbs ────────────────────────────────────────────────────

async function cli(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  const run = (args) => codemuxCli(env, ["--client", "live-cli", ...args], { signal });
  const folder = join(env.ROOT, `cli-live-${tag}`);
  mkdirSync(folder, { recursive: true });
  const opened = [];
  try {
    const created = printed(await run(["workspace", "create", folder, "--json"]));
    ctx.check("cli: workspace create opens a folder", created?.ok === true && !!created.result?.workspace_id, created);
    opened.push(created?.result?.workspace_id);
    const worktree = printed(await run(["worktree", "create", "--repo", env.REPO, "--branch", `cli-live-${tag}`, "--json"]));
    const workspaceId = worktree?.result?.workspace_id;
    ctx.check("cli: worktree create makes a worktree workspace", worktree?.ok === true && !!workspaceId, worktree);
    opened.push(workspaceId);

    const first = challenge(tag);
    const started = await run(["thread", "start", "--workspace", workspaceId, "--provider", "codex", "--model", CODEX_MODEL, "--effort", "low",
      "--permission-mode", "read-only", "-m", first.text, "--wait", "--json"]);
    const report = printed(started);
    ctx.check("cli: thread start --wait exits 0 with the reply", started.code === 0 && (report?.final_text ?? "").includes(first.expected),
      { code: started.code, state: report?.state, final_text: report?.final_text });
    const threadId = report?.thread_id;
    const second = challenge(tag);
    const sent = await run(["thread", "send", threadId, "--workspace", workspaceId, "-m", second.text, "--wait", "--json"]);
    ctx.check("cli: thread send --wait exits 0 with the next reply",
      sent.code === 0 && (printed(sent)?.final_text ?? "").includes(second.expected), { code: sent.code, final_text: printed(sent)?.final_text });

    // An outside launch takes only a model id that agent_capabilities lists
    // (`unsupported_model` otherwise). Claude's catalog lists aliases such as
    // `haiku`, so pick the cheap one from it, as the guide tells agents to.
    const caps = printed(await run(["call", "agent_capabilities", JSON.stringify({ workspace_id: workspaceId, provider: "claude" })]));
    const claudeModels = caps?.result?.providers?.[0]?.capabilities?.models?.map((model) => model.id) ?? [];
    const claudeModel = claudeModels.find((id) => id === CLAUDE_MODEL) ?? claudeModels.find((id) => /haiku/i.test(id));
    ctx.check("cli: agent_capabilities lists a cheap Claude model", !!claudeModel, { models: claudeModels });
    const asking = await run(["thread", "start", "--workspace", workspaceId, "--provider", "claude", "--model", claudeModel,
      "--permission-mode", "default", "-m", "Create a file named x.txt containing hi in the current directory.", "--wait", "--json"]);
    const pending = printed(asking);
    const approval = pending?.pending?.approvals?.[0];
    ctx.check("cli: a run that needs approval exits 2 with the approval", asking.code === 2 && !!approval?.request_id,
      { code: asking.code, state: pending?.state, pending: pending?.pending });
    if (approval) {
      const denied = await run(["thread", "respond", pending.thread_id, approval.request_id, "deny", "--workspace", workspaceId, "--json"]);
      const waited = await run(["thread", "wait", pending.thread_id, "--workspace", workspaceId, "--json"]);
      ctx.check("cli: deny, then thread wait reports the run's end", denied.code === 0 && [0, 1].includes(waited.code),
        { respond: denied.code, wait: waited.code, state: printed(waited)?.state });
    }

    const third = challenge(tag);
    const branched = await run(["thread", "start", "--workspace", workspaceId, "--provider", "codex", "--model", CODEX_MODEL, "--effort", "low",
      "--permission-mode", "read-only", "-m", third.text, "--worktree-branch", `cli-live-2-${tag}`, "--wait", "--json"]);
    const branchedReport = printed(branched);
    const launchedIn = branchedReport?.workspace_id;
    opened.push(launchedIn);
    ctx.check("cli: thread start --worktree-branch runs in the new worktree workspace",
      branched.code === 0 && !!launchedIn && launchedIn !== workspaceId && (await workspaceIds(ctx)).includes(launchedIn)
        && (branchedReport.final_text ?? "").includes(third.expected), { code: branched.code, workspace: launchedIn });
    const workspace = (await ctx.codemux("get_app_state", {})).workspaces.find((w) => w.workspace_id === launchedIn);
    ctx.check("cli: the new workspace is a worktree on the branch", workspace?.git_branch === `cli-live-2-${tag}` && !!workspace?.worktree_path,
      workspace && { branch: workspace.git_branch, worktree_path: workspace.worktree_path });

    const stopped = await run(["thread", "stop", threadId, "--workspace", workspaceId, "--json"]);
    ctx.check("cli: thread stop", stopped.code === 0, { code: stopped.code });
    for (const id of [launchedIn, workspaceId]) {
      const checkout = (await ctx.codemux("get_app_state", {})).workspaces.find((w) => w.workspace_id === id)?.worktree_path;
      const closed = await run(["workspace", "close", id, "--remove-worktree", "--json"]);
      ctx.check(`cli: workspace close --remove-worktree removes ${id}`,
        closed.code === 0 && !(await workspaceIds(ctx)).includes(id) && !(checkout && existsSync(onHost(ctx, checkout))),
        { code: closed.code, stdout: closed.stdout.slice(0, 300) });
    }
    const folderClosed = await run(["workspace", "close", created.result.workspace_id, "--json"]);
    ctx.check("cli: workspace close keeps a plain folder's files", folderClosed.code === 0 && existsSync(folder));
  } finally {
    for (const id of opened) await ownerClose(ctx, id);
  }
}

// ── agent-shell: an in-app agent's own shell ──────────────────────────────

async function agentShell(ctx) {
  const tag = tagOf();
  const bin = ctx.env.BIN;
  const text = [
    "Run these two shell commands with your Bash tool, exactly as written, one at a time. Do not run anything else.",
    `1. "${bin}" json agent_connector_status '{}'; echo "AGENT-SHELL-${tag}-A=$?"`,
    `2. "${bin}" browser open about:blank; echo "AGENT-SHELL-${tag}-B=$?"`,
    "Then reply with the single word DONE.",
  ].join("\n");
  const chat = await ctx.startChat({ provider: "claude", model: CLAUDE_MODEL, permissionMode: "bypassPermissions", text });
  ctx.observe("agent-shell thread", chat.threadId);
  // `$?` is literal in the commands, so a digit after `=` comes from an output.
  const ran = new RegExp(`AGENT-SHELL-${tag}-B=\\d`);
  const all = await ctx.waitFor(async () => {
    const rows = await ctx.assert.sql(`SELECT payload FROM agent_chat_messages WHERE thread_id=${q(chat.threadId)} ` +
      `AND json_extract(payload,'$.type')='item_completed'`);
    const text = rows.map((row) => row.payload).join("\n");
    return ran.test(text) ? text : null;
  }, { timeout: 300_000, interval: 2_000, label: "both commands ran" });
  ctx.check("agent-shell: an admin command from the agent's shell is refused",
    all.includes(`AGENT-SHELL-${tag}-A=1`) && all.includes("admin_refused"));
  ctx.check("agent-shell: a browser command from the same shell still works", all.includes(`AGENT-SHELL-${tag}-B=0`));
}

// ── claude: a real outside agent ──────────────────────────────────────────

async function claude(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  const outside = join(env.ROOT, "outside");
  mkdirSync(join(env.ROOT, "logs"), { recursive: true });
  mkdirSync(outside, { recursive: true });
  const config = join(outside, "claude-mcp.json");
  // No --client: the principal comes from Claude Code's clientInfo.
  writeFileSync(config, JSON.stringify({ mcpServers: { codemux: { command: env.BIN, args: ["mcp"] } } }));
  const { text: task, expected } = challenge(tag);
  const call = (tool) => `mcp__codemux__${tool}`;
  const prompt = [
    "Use only the codemux MCP tools, exactly as follows, without asking questions.",
    `1. Call ${call("worktree_create")} with {"repo_path":${JSON.stringify(env.REPO)},"branch":"outside-claude-${tag}","client_request_id":"oc-wt-${tag}"}. ` +
      `If its state is not "succeeded", call ${call("operation_status")} with its operation_id until it is. Take workspace_id from the result.`,
    `2. Call ${call("thread_launch")} with {"workspace_id":"<the workspace_id>","provider":"codex","model":"${CODEX_MODEL}","effort":"low",` +
      `"permission_mode":"read-only","title":"Outside Claude ${tag}","message":${JSON.stringify(task)},"client_request_id":"oc-launch-${tag}"}. ` +
      `Then call ${call("operation_status")} with its operation_id until its state is "succeeded", and take thread_id from it.`,
    `3. Call ${call("thread_wait")} with {"workspace_id":"<the workspace_id>","thread_id":"<the thread_id>","timeout_ms":120000}. ` +
      `Call it again while state is "running".`,
    `4. Call ${call("thread_read")} with {"workspace_id":"<the workspace_id>","thread_id":"<the thread_id>"} to read the assistant's reply.`,
    `5. Call ${call("workspace_close")} with {"workspace_id":"<the workspace_id>","client_request_id":"oc-close-${tag}","delete_worktree":true}.`,
    "6. Reply with exactly one line: REPORT: followed by the assistant's reply from step 4.",
  ].join("\n");
  // Flags are version-dependent: use --max-turns only where this claude has it.
  const help = await sandboxRun(env, ["claude", "--help"], { signal, timeoutSec: 60, cwd: outside });
  const tools = ["worktree_create", "operation_status", "thread_launch", "thread_wait", "thread_read", "workspace_close"].map(call);
  const argv = ["/usr/bin/env", "MCP_TOOL_TIMEOUT=600000", "claude", "-p", prompt, "--model", CLAUDE_MODEL,
    "--mcp-config", config, "--strict-mcp-config", "--allowedTools", tools.join(","),
    "--disallowedTools", "Bash,Read,Edit,Write,Glob,Grep,WebFetch,WebSearch,Task",
    "--output-format", "stream-json", "--verbose", ...(help.stdout.includes("--max-turns") ? ["--max-turns", "24"] : [])];
  let workspaceId = null;
  try {
    const result = await sandboxRun(env, argv, { signal, timeoutSec: 480, cwd: outside });
    writeFileSync(join(env.ROOT, "logs", "outside-claude.jsonl"), result.stdout);
    writeFileSync(join(env.ROOT, "logs", "outside-claude.stderr.log"), result.stderr);
    const events = result.stdout.split("\n").filter(Boolean).map((line) => { try { return JSON.parse(line); } catch { return null; } }).filter(Boolean);
    const final = events.findIndex((event) => event.type === "result");
    const used = events.filter((event) => event.type === "assistant").flatMap((event) => event.message?.content ?? [])
      .filter((block) => block.type === "tool_use").map((block) => block.name.replace(/^mcp__codemux__/, ""));
    const order = ["worktree_create", "thread_launch", "thread_wait", "thread_read"].map((tool) => used.indexOf(tool));
    ctx.observe("claude: tools used", used);
    ctx.check("claude: exits 0", result.code === 0, { code: result.code, stderr: result.stderr.slice(-500) });
    ctx.check("claude: called worktree_create, thread_launch, thread_wait and thread_read in order",
      order.every((index) => index >= 0) && order.every((index, i) => i === 0 || index > order[i - 1]), used);
    const relayed = events.slice(0, final < 0 ? events.length : final)
      .some((event) => event.type === "user" && JSON.stringify(event.message?.content ?? []).includes(expected));
    ctx.check("claude: the token reached Claude in a tool result before its answer", relayed);
    ctx.check("claude: the final answer reports the token", (events[final]?.result ?? "").includes(`REPORT: ${expected}`), events[final]?.result);

    const receipts = await receiptsFor(ctx, tag);
    ctx.check("claude: its receipts belong to local:claude-code",
      receipts.length >= 3 && receipts.every((r) => r.principal === "local:claude-code"), receipts);
    workspaceId = receipts.find((r) => r.tool === "worktree_create")?.workspace_id ?? null;
    const launch = receipts.find((r) => r.tool === "thread_launch");
    const [session] = launch ? await ctx.assert.sql(`SELECT workspace_id FROM agent_chat_sessions WHERE thread_id=${q(launch.thread_id)}`) : [];
    const [reply] = launch ? await ctx.assert.sql(`SELECT content FROM agent_chat_search WHERE thread_id=${q(launch.thread_id)} ` +
      `AND role='assistant' AND content LIKE ${q(`%${expected}%`)}`) : [];
    ctx.check("claude: the Codex thread ran in the new worktree and stored the token", !!workspaceId && session?.workspace_id === workspaceId && !!reply,
      { workspaceId, session });
    ctx.check("claude: the worktree is gone", !(await workspaceIds(ctx)).includes(workspaceId));
    workspaceId = null;
  } finally {
    await ownerClose(ctx, workspaceId);
  }
}

// ── user-scope: a user-level codemux entry does not widen in-app chats ───

async function userScope(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  const codexConfig = join(env.ROOT, "home", ".codex", "config.toml");
  const savedCodex = existsSync(codexConfig) ? readFileSync(codexConfig, "utf8") : null;
  const added = [];
  try {
    // The guide's user-scope setup, written only to the sandbox's private HOME.
    for (const [cli, argv] of [
      ["claude", ["claude", "mcp", "add", "--scope", "user", "codemux", "--", env.BIN, "mcp", "--client", "claude-code"]],
      ["codex", ["codex", "mcp", "add", "codemux", "--", env.BIN, "mcp", "--client", "codex"]],
    ]) {
      const result = await sandboxRun(env, argv, { signal, timeoutSec: 120 });
      ctx.check(`user-scope: \`${cli} mcp add\` writes the private HOME's config`, result.code === 0, { code: result.code, stderr: result.stderr.slice(-300) });
      added.push(cli);
    }
    const receiver = await openChat(ctx, { provider: "claude", model: CLAUDE_MODEL, permissionMode: "bypassPermissions" });
    const ask = (key) => `Call the codemux thread_send tool once with {"thread_id":${JSON.stringify(receiver.threadId)},` +
      `"client_request_id":"${key}","delivery":"queue","message":"US-${tag}"}. Do not call any other tool. Then reply SENT.`;
    const chats = [
      ["claude", await ctx.startChat({ provider: "claude", model: CLAUDE_MODEL, permissionMode: "bypassPermissions", text: ask(`us-claude-${tag}`) })],
      ["codex", await ctx.startChat({ provider: "codex", model: CODEX_MODEL, effort: "low", permissionMode: "danger-full-access", text: ask(`us-codex-${tag}`) })],
    ];
    for (const [provider, chat] of chats) {
      const row = await ctx.waitFor(async () => (await ctx.assert.sql(`SELECT principal FROM agent_message_deliveries ` +
        `WHERE thread_id=${q(receiver.threadId)} AND principal=${q(`thread:${chat.threadId}`)}`))[0] ?? null,
      { timeout: 300_000, interval: 2_000, label: `${provider} chat's delivery` });
      ctx.check(`user-scope: an in-app ${provider} chat sends as itself, not as the user-scope client`, row.principal === `thread:${chat.threadId}`, row);
    }
    const outsiders = await ctx.assert.sql(`SELECT principal FROM agent_message_deliveries WHERE thread_id=${q(receiver.threadId)} ` +
      `AND principal LIKE 'local:%'`);
    ctx.check("user-scope: nothing reached the receiver as a local client", outsiders.length === 0, outsiders);
    // In-app Codex turns its user-level codemux entry off (D7).
    const appServer = below(env.INIT_PID).map(cmdline).find((argv) => argv.some((arg) => arg === "app-server")
      && argv.some((arg) => arg === "mcp_servers.codemux.enabled=false"));
    ctx.check("user-scope: in-app Codex starts with the user's codemux entry disabled", !!appServer);
  } finally {
    if (added.includes("claude")) await sandboxRun(env, ["claude", "mcp", "remove", "--scope", "user", "codemux"], { timeoutSec: 120 }).catch(() => {});
    if (added.includes("codex")) await sandboxRun(env, ["codex", "mcp", "remove", "codemux"], { timeoutSec: 120 }).catch(() => {});
    // Private run-local file: put it back as it was, in place.
    if (savedCodex !== null) writeFileSync(codexConfig, savedCodex);
  }
}

// ── hermes: Hermes as an outside agent (opt-in) ───────────────────────────

async function hermes(ctx) {
  const { env, signal } = ctx;
  const tag = tagOf();
  // A file bind into the sandbox: edit it in place, never by rename.
  const config = join(env.ROOT, "private", "hermes", "config.yaml");
  const saved = readFileSync(config, "utf8");
  const outside = join(env.ROOT, "outside");
  mkdirSync(outside, { recursive: true });
  const usage = join(env.ROOT, "logs", "outside-hermes-usage.json");
  let workspaceId = null;
  try {
    writeFileSync(config, `${saved.trimEnd()}\nmcp_servers:\n  codemux:\n    command: ${JSON.stringify(env.BIN)}\n` +
      `    args: [mcp, --client, hermes]\n    timeout: 660\n    tools: { include: [worktree_create, operation_status, thread_launch, thread_wait, thread_read, workspace_close] }\n`);
    const { text: task, expected } = challenge(tag);
    const call = (tool) => `mcp__codemux__${tool}`;
    const prompt = [
      "If the codemux tools are not loaded, use tool_search for 'codemux' first. Then use only the codemux tools, exactly as follows.",
      `1. Call ${call("worktree_create")} with {"repo_path":${JSON.stringify(env.REPO)},"branch":"outside-hermes-${tag}","client_request_id":"oh-wt-${tag}"}; ` +
        `poll ${call("operation_status")} until "succeeded" and take workspace_id.`,
      `2. Call ${call("thread_launch")} with {"workspace_id":"<the workspace_id>","provider":"codex","model":"${CODEX_MODEL}","effort":"low",` +
        `"permission_mode":"read-only","title":"Outside Hermes ${tag}","message":${JSON.stringify(task)},"client_request_id":"oh-launch-${tag}"}; ` +
        `poll ${call("operation_status")} until "succeeded" and take thread_id.`,
      `3. Call ${call("thread_wait")} with {"workspace_id":"<the workspace_id>","thread_id":"<the thread_id>","timeout_ms":120000} until state is not "running".`,
      `4. Call ${call("thread_read")} to read the assistant's reply.`,
      `5. Call ${call("workspace_close")} with {"workspace_id":"<the workspace_id>","client_request_id":"oh-close-${tag}","delete_worktree":true}.`,
      "6. Reply with exactly one line: REPORT: followed by the assistant's reply from step 4.",
    ].join("\n");
    const result = await sandboxRun(env, ["hermes", "-z", prompt, "--reasoning", "low", "--usage-file", usage], { signal, timeoutSec: 600, cwd: outside });
    writeFileSync(join(env.ROOT, "logs", "outside-hermes.log"), `${result.stdout}\n--- stderr\n${result.stderr}`);
    ctx.check("hermes: reports the token", result.code === 0 && result.stdout.includes(`REPORT: ${expected}`), { code: result.code, stdout: result.stdout.slice(-500) });
    const receipts = await receiptsFor(ctx, tag);
    ctx.check("hermes: its receipts belong to local:hermes", receipts.length >= 2 && receipts.every((r) => r.principal === "local:hermes"), receipts);
    workspaceId = receipts.find((r) => r.tool === "worktree_create")?.workspace_id ?? null;
    const launch = receipts.find((r) => r.tool === "thread_launch");
    const [session] = launch ? await ctx.assert.sql(`SELECT workspace_id FROM agent_chat_sessions WHERE thread_id=${q(launch.thread_id)}`) : [];
    const [reply] = launch ? await ctx.assert.sql(`SELECT content FROM agent_chat_search WHERE thread_id=${q(launch.thread_id)} ` +
      `AND role='assistant' AND content LIKE ${q(`%${expected}%`)}`) : [];
    ctx.check("hermes: the Codex thread ran in the new worktree and stored the token", session?.workspace_id === workspaceId && !!reply, { workspaceId, session });
    const spent = existsSync(usage) ? JSON.parse(readFileSync(usage, "utf8")) : null;
    ctx.check("hermes: the usage file reports model calls", (spent?.api_calls ?? 0) >= 1, spent);
  } finally {
    writeFileSync(config, saved);
    await ownerClose(ctx, workspaceId);
  }
}

const RUNS = { flow, http, cli, "agent-shell": agentShell, claude, "user-scope": userScope, hermes };

export async function run(ctx) {
  const parts = selectedParts(ctx.env);
  ctx.observe("parts", parts);
  for (const part of parts) {
    const started = Date.now();
    try {
      await RUNS[part](ctx);
    } catch (error) {
      if (error instanceof McpRpcError || error instanceof McpToolError || error instanceof HttpStatusError) {
        throw new Error(`${part}: ${error.message}`);
      }
      throw error;
    } finally {
      ctx.observe(`${part} seconds`, Math.round((Date.now() - started) / 1000));
    }
  }
}
