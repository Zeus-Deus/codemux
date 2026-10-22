// Stop ends an OpenCode chat's in-flight Codemux tool call, and a full-access
// Hermes chat launches a Hermes workspace on its own profile.
// The OpenCode half uses a scripted local model (no sign-in); run it in a
// fresh root, since it writes the sandbox's OpenCode config. The Hermes half
// needs a configured default Hermes profile and spends one cheap turn per chat.
// Opt in with ORCH_LIVE_PROVIDERS including opencode and/or hermes.
import { createServer } from 'node:http';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';

export const name = 'revocation';
export const description = 'Stop cancels an in-flight OpenCode tool call; a Hermes chat launches a Hermes workspace.';

const q = (s) => `'${s.replaceAll("'", "''")}'`;
const SERVER = /tools\["(codemux_[0-9a-f]{8})"\]/g;

/** OpenAI-compatible streaming model. `SLOW:<workspace>:<thread>` calls
 *  thread_wait on the chat's Codemux server; `HOLD` keeps the turn running. */
function startModel(requests) {
  const server = createServer((req, res) => {
    let raw = '';
    req.on('data', (chunk) => { raw += chunk; });
    req.on('end', async () => {
      if (req.method === 'GET') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ data: [{ id: 'm', object: 'model' }] }));
        return;
      }
      const body = JSON.parse(raw || '{}');
      const messages = body.messages ?? [];
      const last = messages.at(-1) ?? {};
      const textOf = (message) => (typeof message.content === 'string' ? message.content
        : (message.content ?? []).map((part) => part.text ?? '').join(' '));
      const text = textOf(last);
      const tools = (body.tools ?? []).map((tool) => tool.function?.name);
      const system = messages.filter((m) => m.role === 'system').map(textOf).join('\n');
      const servers = [...new Set([...system.matchAll(SERVER)].map((match) => match[1]))];
      requests.push({ at: Date.now(), tools, role: last.role, text: text.slice(0, 600), servers });
      const chunk = (delta, finish = null) => `data: ${JSON.stringify({
        id: 'c', object: 'chat.completion.chunk', created: 0, model: 'm',
        choices: [{ index: 0, delta, finish_reason: finish }],
      })}\n\n`;
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      const slow = last.role === 'user' ? /SLOW:(\S+):(\S+)/.exec(text) : null;
      if (slow && tools.includes('execute')) {
        const code = `const started = Date.now();
const settled = await Promise.allSettled(${JSON.stringify(servers)}.map((server) => tools[server].thread_wait({
  workspace_id: ${JSON.stringify(slow[1])}, thread_id: ${JSON.stringify(slow[2])}, timeout_ms: 20000 })));
return JSON.stringify({ elapsed_ms: Date.now() - started, results: settled.map((r) => r.status === "fulfilled" ? r.value : String(r.reason)) });`;
        requests.at(-1).toolCall = true;
        res.write(chunk({ role: 'assistant', tool_calls: [{ index: 0, id: `call_${Date.now()}`, type: 'function', function: { name: 'execute', arguments: JSON.stringify({ code }) } }] }));
        res.write(chunk({}, 'tool_calls'));
      } else {
        if (last.role === 'user' && text.includes('HOLD')) await new Promise((r) => setTimeout(r, 45_000));
        res.write(chunk({ role: 'assistant', content: last.role === 'tool' ? `done: ${text.slice(0, 200)}` : 'OK' }));
        res.write(chunk({}, 'stop'));
      }
      res.write(`data: ${JSON.stringify({ id: 'c', object: 'chat.completion.chunk', created: 0, model: 'm', choices: [], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } })}\n\n`);
      res.end('data: [DONE]\n\n');
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

async function openCodeChat(ctx) {
  const threadId = randomUUID();
  const paneId = await ctx.driver.invoke('agent_chat_create_pane', {
    workspaceId: ctx.env.WORKSPACE_ID, provider: 'opencode', cwd: ctx.env.REPO, launchMode: 'new_tab', threadId,
  });
  await ctx.driver.invoke('agent_chat_start_session', {
    paneId, provider: 'opencode', expectedThread: null,
    input: { thread_id: threadId, cwd: ctx.env.REPO, model: 'fake/m', resume_cursor: null, permission_mode: null, effort: null,
      context_window: null, service_tier: 'default', additional_directories: [], env: null, extra: null },
  });
  return { threadId, paneId };
}

async function send(ctx, provider, threadId, text) {
  await ctx.driver.invoke('agent_chat_send_turn', {
    provider,
    input: { thread_id: threadId, text, display_text: text, skill_ids: [], skill_text: null, include_plugins: true,
      images: [], model_override: null, effort_override: null, permission_mode_override: null },
  });
}

const status = (ctx, threadId) =>
  ctx.codemux('thread_status', { workspace_id: ctx.env.WORKSPACE_ID, thread_id: threadId }).catch(() => null);

async function openCodeStop(ctx) {
  const requests = [];
  const model = await startModel(requests);
  const dir = join(ctx.env.ROOT, 'config', 'opencode');
  const config = join(dir, 'opencode.json');
  const saved = existsSync(config) ? readFileSync(config, 'utf8') : null;
  mkdirSync(dir, { recursive: true });
  writeFileSync(config, JSON.stringify({
    provider: { fake: { npm: '@ai-sdk/openai-compatible', name: 'Fake', options: { baseURL: `http://127.0.0.1:${model.address().port}/v1`, apiKey: 'test-only' }, models: { m: { name: 'm', tool_call: true } } } },
  }));
  const chats = [];
  try {
    const receiver = await openCodeChat(ctx);
    const caller = await openCodeChat(ctx);
    chats.push(receiver, caller);
    ctx.observe('opencode receiver', receiver.threadId);
    ctx.observe('opencode caller', caller.threadId);
    // The receiver's turn keeps running, so thread_wait blocks (up to 20 s).
    await send(ctx, 'opencode', receiver.threadId, 'HOLD');
    await ctx.waitFor(async () => (await status(ctx, receiver.threadId))?.settled === false, { timeout: 60_000, interval: 500, label: 'receiver turn running' });
    await send(ctx, 'opencode', caller.threadId, `SLOW:${ctx.env.WORKSPACE_ID}:${receiver.threadId}`);
    await ctx.waitFor(async () => requests.some((r) => r.toolCall), { timeout: 120_000, interval: 250, label: 'slow tool call issued' });
    const issued = requests.find((r) => r.toolCall).at;
    // Give the gateway call time to start waiting, then press Stop.
    await ctx.waitFor(async () => Date.now() - issued > 4_000, { timeout: 10_000, interval: 250, label: 'call in flight' });
    await ctx.openTab(await ctx.tabOf(caller.paneId));
    // The visible composer's Stop button; hidden tabs keep theirs mounted.
    await ctx.waitFor(`(() => { const b = [...document.querySelectorAll('button[aria-label="Stop"]')].find((e) => e.offsetParent !== null); if (!b) return false; const r = b.getBoundingClientRect(); const top = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); if (!b.contains(top)) return false; b.dataset.liveStop = '1'; return true; })()`, { timeout: 20_000, interval: 250, label: 'Stop button visible and uncovered' });
    const stopped = Date.now();
    await ctx.driver.clickSelector('button[data-live-stop="1"]');
    await ctx.waitFor(async () => (await status(ctx, caller.threadId))?.settled, { timeout: 30_000, interval: 250, label: 'caller settled after Stop' });
    const settleMs = Date.now() - stopped;
    const receiverRunning = (await status(ctx, receiver.threadId))?.settled === false;
    ctx.observe('Stop → caller settled (ms)', settleMs);
    ctx.check('the receiver was still running, so the wait could not have completed', receiverRunning, null);
    ctx.check('the stopped chat settled well before thread_wait\'s 20 s limit', settleMs < 10_000, settleMs);
    // OpenCode aborts its own tool on Stop as well, so its store records the
    // call as aborted either way; what must never happen is a late result.
    // (The gateway's own answer is covered by the gateway unit test.)
    await ctx.waitFor(async () => Date.now() - stopped > 22_000, { timeout: 30_000, interval: 500, label: 'past thread_wait\'s 20 s limit' });
    const late = requests.filter((r) => r.at > stopped && r.role === 'tool');
    ctx.observe('model requests after Stop', requests.filter((r) => r.at > stopped).map((r) => r.role));
    ctx.check('no tool result reached the model after Stop, even past the wait limit', late.length === 0, late);
    await ctx.shot('pr4r3-opencode-stop');
  } finally {
    for (const chat of chats) {
      await ctx.driver.invoke('agent_chat_stop_session', { provider: 'opencode', threadId: chat.threadId }).catch(() => null);
    }
    if (saved === null) rmSync(config, { force: true }); else writeFileSync(config, saved);
    model.close();
    writeFileSync(join(ctx.env.ROOT, 'logs', 'revocation-model.json'), JSON.stringify(requests, null, 2));
  }
}

async function hermesWorkspace(ctx) {
  const profiles = await ctx.driver.invoke('hermes_profiles', {});
  const profile = profiles.find((p) => p.id === 'default');
  if (!profile) throw new Error('no configured default Hermes profile');
  const catalog = await ctx.driver.invoke('hermes_catalog', { profile });
  if (catalog.state !== 'ready') throw new Error(catalog.message ?? 'Hermes catalog is unavailable');
  if (!catalog.session.modes?.availableModes?.some((m) => m.id === 'dont_ask')) throw new Error('Hermes does not advertise dont_ask');
  const model = catalog.session.models.currentModelId;
  const tag = randomUUID().slice(0, 8);
  const branch = `hermes-launch-${tag}`;
  const chat = await ctx.startChat({
    provider: 'hermes', model, permissionMode: 'dont_ask', extra: { hermes_profile: profile },
    text: `The user asks for one new independent workspace. Call the Codemux workspace_launch tool exactly once with ${JSON.stringify({ title: `Hermes launch ${tag}`, branch, message: `Reply only LAUNCHED-${tag}. Do not use tools.` })} and no provider. Then reply STARTED and end your turn.`,
  });
  ctx.observe('hermes caller', chat.threadId);
  const launched = await ctx.waitFor(async () => {
    const [row] = await ctx.assert.sql(`SELECT s.thread_id, s.provider, s.permission_mode, s.workspace_id, s.cwd FROM agent_control_operations o JOIN agent_chat_sessions s ON s.thread_id = o.thread_id WHERE o.tool = 'workspace_launch' AND o.principal = ${q(`thread:${chat.threadId}`)}`);
    return row ?? null;
  }, { timeout: 300_000, interval: 1_000, label: 'Hermes workspace chat created' });
  ctx.observe('launched chat', launched);
  ctx.check('the launched chat runs Hermes in dont_ask', launched.provider === 'hermes' && launched.permission_mode === 'dont_ask', launched);
  ctx.check('it runs in a new workspace on the requested branch', launched.workspace_id !== ctx.env.WORKSPACE_ID && launched.cwd.endsWith(`/${branch}`), launched);
  const binding = await ctx.driver.invoke('hermes_binding', { threadId: launched.thread_id });
  ctx.check('the launched chat keeps the caller\'s Hermes profile', binding?.profile?.id === 'default', { profile: binding?.profile?.id });
  // The launched chat lives in another workspace; read its reply from the store.
  const reply = await ctx.waitFor(async () => {
    const rows = await ctx.assert.sql(`SELECT json_extract(payload,'$.item.text') AS text FROM agent_chat_messages WHERE thread_id = ${q(launched.thread_id)} AND json_extract(payload,'$.type') = 'item_completed'`);
    return rows.find((r) => r.text?.includes(`LAUNCHED-${tag}`)) ?? null;
  }, { timeout: 300_000, interval: 1_000, label: 'launched Hermes chat replied' });
  ctx.check('the launched Hermes chat ran its brief', !!reply, reply);
  await ctx.openTab(chat.tabId);
  await ctx.shot('pr4r3-hermes-workspace-launch');
}

export async function run(ctx) {
  const providers = (ctx.env.ORCH_LIVE_PROVIDERS ?? '').split(',');
  if (providers.includes('opencode')) await openCodeStop(ctx);
  else ctx.observe('opencode skipped', 'ORCH_LIVE_PROVIDERS does not include opencode');
  if (providers.includes('hermes')) await hermesWorkspace(ctx);
  else ctx.observe('hermes skipped', 'ORCH_LIVE_PROVIDERS does not include hermes');
}
