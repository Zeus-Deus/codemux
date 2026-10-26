import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

async function fixture(t, code) {
  const directory = await mkdtemp(path.join(tmpdir(), "codemux-plugin-worker-"));
  await writeFile(path.join(directory, "plugin.mjs"), code);
  const child = spawn(process.env.CODEMUX_PLUGIN_HOST_TEST_BINARY || process.execPath,
    process.env.CODEMUX_PLUGIN_HOST_TEST_BINARY ? [] : [fileURLToPath(new URL("./main.mjs", import.meta.url))],
    { stdio: ["pipe", "pipe", "pipe"], env: { PATH: process.env.PATH } });
  const queue = [], waiters = [];
  let input = "", failure;
  child.stdout.on("data", chunk => {
    input += chunk;
    let newline;
    while ((newline = input.indexOf("\n")) >= 0) {
      const value = JSON.parse(input.slice(0, newline)); input = input.slice(newline + 1);
      const waiter = waiters.shift(); if (waiter) waiter.resolve(value); else queue.push(value);
    }
  });
  child.once("exit", code => { failure = new Error(`Worker exited: ${code}`); for (const waiter of waiters.splice(0)) waiter.reject(failure); });
  const next = () => queue.length ? Promise.resolve(queue.shift()) : failure ? Promise.reject(failure) : new Promise((resolve, reject) => waiters.push({ resolve, reject }));
  const send = message => child.stdin.write(JSON.stringify(message) + "\n");
  t.after(async () => { child.kill("SIGKILL"); await rm(directory, { recursive: true, force: true }); });
  assert.equal((await next()).type, "booted");
  send({ type: "init", protocol: 1, directory, entry: "plugin.mjs", dataDirectory: directory, capabilities: ["storage", "settings", "ui.native-trees"] });
  assert.equal((await next()).type, "ready");
  const invoke = (handler, input, id = "one") => send({ type: "invoke", handler, input, invocationId: id, operationId: "operation", caller: { kind: "agent", threadId: "source" }, context: { projectId: "project", workspaceId: "workspace", threadId: "target" } });
  return { next, send, invoke, child };
}

test("activation and escaped async calls have no host authority; caller/context stay immutable", async t => {
  const f = await fixture(t, `let release, late; export async function activate(ctx) {
    let denied=false; try {await ctx.storage.get('activation');} catch {denied=true;}
    ctx.handle('action:start', (_, invocation) => {try {invocation.caller.threadId='forged';}catch{}
      late = new Promise(r=>release=r).then(()=>ctx.storage.set('late',true)).then(()=>false,()=>true);
      return {data:{denied,caller:invocation.caller,context:invocation.context,operationId:invocation.operationId}};});
    ctx.handle('action:release', async()=>{release();return {data:await late};});}`);
  f.invoke("action:start"); const start = await f.next();
  assert.equal(start.type, "result"); assert.equal(start.result.data.denied, true);
  assert.equal(start.result.data.caller.threadId, "source"); assert.equal(start.result.data.context.workspaceId, "workspace");
  assert.equal(start.result.data.operationId, "operation");
  f.invoke("action:release", null, "two"); assert.equal((await f.next()).result.data, true);
});
test("host errors retain structured recovery details", async t => {
  const f = await fixture(t, `export function activate(ctx){ctx.handle('action:call',async()=>{try{await ctx.threads.send({threadId:'target'});}catch(e){return {data:{reason:e.reason,priorEffect:e.priorEffect}};}});}`);
  f.invoke("action:call"); const call = await f.next(); assert.equal(call.method, "threads.send");
  f.send({ type: "host-result", invocationId: call.invocationId, callId: call.callId, ok: false, error: { reason: "already_applied", message: "Recover previous effect", priorEffect: { threadId: "prior" } } });
  assert.deepEqual((await f.next()).result.data, { reason: "already_applied", priorEffect: { threadId: "prior" } });
});
test("cancel aborts signal and revokes pending host calls without affecting next invocation", async t => {
  const f = await fixture(t, `export function activate(ctx){ctx.handle('action:hold',async(_,i)=>{await ctx.storage.get('entered');await new Promise(r=>i.signal.addEventListener('abort',r));return {data:'forbidden'};});ctx.handle('action:ok',()=>({data:'ok'}));}`);
  f.invoke("action:hold"); const call = await f.next();
  f.send({ type: "host-result", invocationId: call.invocationId, callId: call.callId, ok: true, value: 1 });
  f.send({ type: "cancel", invocationId: "one" }); assert.equal((await f.next()).type, "cancelled");
  f.invoke("action:ok", null, "two"); assert.equal((await f.next()).result.data, "ok");
});
test("results filter unknown intents and provenance, reject invalid output, bound logs", async t => {
  const f = await fixture(t, `export function activate(ctx){ctx.handle('action:output',()=>({intents:[{type:'future.intent'},{type:'view.open',viewId:'view'}],card:{viewId:'view',summary:'Summary',dataOwner:'forged'}}));ctx.handle('action:invalid',()=>({message:false}));ctx.handle('action:logs',()=>{for(let i=0;i<1000;i++)ctx.log('info','x'.repeat(3000));return {};});}`);
  f.invoke("action:output"); const output = (await f.next()).result;
  assert.equal(output.unsupportedIntentCount, 1); assert.equal(output.card.dataOwner, undefined);
  assert.deepEqual(output.intents, [{ type: "view.open", viewId: "view" }]);
  f.invoke("action:invalid", null, "two"); assert.equal((await f.next()).type, "failure");
  f.invoke("action:logs", null, "three"); let logs = 0, message;
  while ((message = await f.next()).type === "log") { logs++; assert.equal(message.message.length, 2000); }
  assert.equal(message.type, "result"); assert.equal(logs, 100);
});
test("native tree registration uses the same invocation host-call scope", async t => {
  const f = await fixture(t, `export function activate(ctx){ctx.trees.register('projects',async()=>({nodes:await ctx.projects.list()}));}`);
  f.invoke("tree:projects"); const call = await f.next(); assert.equal(call.method, "projects.list");
  f.send({ type: "host-result", invocationId: call.invocationId, callId: call.callId, ok: true, value: [] });
  assert.deepEqual((await f.next()).result, { data: { nodes: [] } });
});

test("secret and effect/operation receipt queries preserve the active invocation identity", async t => {
  const f = await fixture(t, `export function activate(ctx){ctx.handle('action:receipts',async()=>({data:await Promise.all([ctx.secrets.get('token'),ctx.workflow.query({query:'effect',idempotencyKey:'stable'}),ctx.workflow.query({query:'operation',operationId:'native-operation'})])}));}`);
  f.invoke("action:receipts");
  const calls = [await f.next(), await f.next(), await f.next()];
  assert.deepEqual(calls.map(call => [call.method, call.input]), [
    ["secrets.get", { key: "token" }],
    ["workflow.query", { query: "effect", idempotencyKey: "stable" }],
    ["workflow.query", { query: "operation", operationId: "native-operation" }],
  ]);
  for (const [index, call] of calls.entries()) {
    assert.equal(call.invocationId, "one");
    f.send({ type: "host-result", invocationId: call.invocationId, callId: call.callId, ok: true, value: index });
  }
  assert.deepEqual((await f.next()).result.data, [0, 1, 2]);
});
