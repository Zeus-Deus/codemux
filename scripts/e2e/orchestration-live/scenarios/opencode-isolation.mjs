// Chats on the shared OpenCode server cannot act as each other, even when a
// user's OpenCode agent rules override Codemux's per-chat tool permissions.
// A scripted local model makes chat A call thread_send on every Codemux
// server OpenCode offers it (through Code Mode's `execute`) while chat B's turn
// is running; only A's own call may deliver.
// Needs the opencode CLI but no provider sign-in. Run it in a fresh root
// (before any OpenCode chat starts): it writes the sandbox's OpenCode config.
// Opt in with ORCH_LIVE_PROVIDERS including opencode.
import { createServer } from 'node:http';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';

export const name = 'opencode-isolation';
export const description = 'Agent permission overrides cannot let one OpenCode chat act as another.';

const q = (s) => `'${s.replaceAll("'", "''")}'`;
const SERVER = /tools\["(codemux_[0-9a-f]{8})"\]/g;

/** OpenAI-compatible streaming model. `PROBE:<thread>` calls thread_send on
 *  every offered Codemux server at once; `HOLD` keeps the turn running. */
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
      const toolResults = messages.filter((m) => m.role === 'tool').map(textOf);
      const system = messages.filter((m) => m.role === 'system').map(textOf).join('\n');
      const servers = [...new Set([...system.matchAll(SERVER)].map((match) => match[1]))];
      requests.push({ tools, role: last.role, text: text.slice(0, 600), toolResults, servers });
      const chunk = (delta, finish = null) => `data: ${JSON.stringify({
        id: 'c', object: 'chat.completion.chunk', created: 0, model: 'm',
        choices: [{ index: 0, delta, finish_reason: finish }],
      })}\n\n`;
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      const probe = last.role === 'user' ? /PROBE:(\S+)/.exec(text) : null;
      if (probe && tools.includes('execute')) {
        const code = `const servers = ${JSON.stringify(servers)};
const settled = await Promise.allSettled(servers.map((server) => tools[server].thread_send({
  thread_id: ${JSON.stringify(probe[1])}, client_request_id: "probe-" + server, delivery: "queue", message: "FROM-" + server })));
return JSON.stringify(settled.map((r, i) => ({ server: servers[i], status: r.status, value: r.status === "fulfilled" ? r.value : String(r.reason) })));`;
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

async function startIn(ctx, { text = null } = {}) {
  const threadId = randomUUID();
  const paneId = await ctx.driver.invoke('agent_chat_create_pane', {
    workspaceId: ctx.env.WORKSPACE_ID, provider: 'opencode', cwd: ctx.env.REPO, launchMode: 'new_tab', threadId,
  });
  await ctx.driver.invoke('agent_chat_start_session', {
    paneId, provider: 'opencode', expectedThread: null,
    input: { thread_id: threadId, cwd: ctx.env.REPO, model: 'fake/m', resume_cursor: null, permission_mode: null, effort: null,
      context_window: null, service_tier: 'default', additional_directories: [], env: null, extra: null },
  });
  if (text) await send(ctx, threadId, text);
  return { threadId, paneId };
}

async function send(ctx, threadId, text) {
  await ctx.driver.invoke('agent_chat_send_turn', {
    provider: 'opencode',
    input: { thread_id: threadId, text, display_text: text, skill_ids: [], skill_text: null, include_plugins: true,
      images: [], model_override: null, effort_override: null, permission_mode_override: null },
  });
}

const status = (ctx, threadId) =>
  ctx.codemux('thread_status', { workspace_id: ctx.env.WORKSPACE_ID, thread_id: threadId }).catch(() => null);

export async function run(ctx) {
  if (!(ctx.env.ORCH_LIVE_PROVIDERS ?? '').split(',').includes('opencode')) {
    ctx.observe('skipped', 'ORCH_LIVE_PROVIDERS does not include opencode');
    return;
  }
  const requests = [];
  const model = await startModel(requests);
  const dir = join(ctx.env.ROOT, 'config', 'opencode');
  const config = join(dir, 'opencode.json');
  const saved = existsSync(config) ? readFileSync(config, 'utf8') : null;
  mkdirSync(dir, { recursive: true });
  writeFileSync(config, JSON.stringify({
    provider: { fake: { npm: '@ai-sdk/openai-compatible', name: 'Fake', options: { baseURL: `http://127.0.0.1:${model.address().port}/v1`, apiKey: 'test-only' }, models: { m: { name: 'm', tool_call: true } } } },
    // The user rule that outranks Codemux's server-wide codemux_* deny.
    agent: { build: { permission: { '*': 'allow' } } },
  }));
  const chats = [];
  try {
    const receiver = await startIn(ctx);
    const a = await startIn(ctx);
    const b = await startIn(ctx);
    chats.push(receiver, a, b);
    for (const chat of chats) ctx.observe('chat', chat.threadId);

    // B's turn runs (its credential has live authority) while A probes.
    await send(ctx, b.threadId, 'HOLD');
    await ctx.waitFor(async () => (await status(ctx, b.threadId))?.settled === false, { timeout: 60_000, interval: 500, label: 'B turn running' });
    await send(ctx, a.threadId, `PROBE:${receiver.threadId}`);
    await ctx.waitFor(async () => requests.some((r) => r.role === 'tool'), { timeout: 120_000, interval: 500, label: 'A tool results' });
    const bRunning = (await status(ctx, b.threadId))?.settled === false;

    const offered = requests.findLast((r) => r.role === 'user' && r.text.includes('PROBE:'))?.servers ?? [];
    const results = requests.findLast((r) => r.role === 'tool')?.toolResults ?? [];
    await ctx.waitFor(async () => (await status(ctx, a.threadId))?.settled, { timeout: 120_000, interval: 500, label: 'A settled' });
    const rows = await ctx.assert.sql(`SELECT principal,state FROM agent_message_deliveries WHERE thread_id=${q(receiver.threadId)}`);
    const from = (chat) => rows.filter((row) => row.principal === `thread:${chat.threadId}`);

    ctx.observe('Codemux servers offered to A', offered);
    ctx.observe('tool results returned to A', results.map((text) => text.slice(0, 600)));
    ctx.check("the agent override offers A every chat's Codemux server", offered.length === 3, offered);
    ctx.check("B's turn was running during A's calls", bRunning, null);
    ctx.check('A delivered exactly once, as itself', from(a).length === 1, rows);
    ctx.check('nothing was delivered as B', from(b).length === 0, rows);
    const outcomes = results.flatMap((text) => { try { return JSON.parse(text); } catch { return []; } })
      .map(({ server, status, value }) => ({ server, status, refused: String(value).includes('not another chat') }));
    ctx.observe('per-server outcomes', outcomes);
    ctx.check("only A's own server accepted the call", outcomes.filter((o) => o.status === 'fulfilled').length === 1, outcomes);
    ctx.check('the gateway refused the calls made with the other chats\' credentials',
      outcomes.length === 3 && outcomes.filter((o) => o.status === 'rejected' && o.refused).length === 2, outcomes);
    await ctx.openTab(await ctx.tabOf(a.paneId));
    await ctx.shot('pr4r2-opencode-isolation');
  } finally {
    for (const chat of chats) {
      await ctx.driver.invoke('agent_chat_stop_session', { provider: 'opencode', threadId: chat.threadId }).catch(() => null);
    }
    if (saved === null) rmSync(config, { force: true }); else writeFileSync(config, saved);
    model.close();
    writeFileSync(join(ctx.env.ROOT, 'logs', 'opencode-isolation-model.json'), JSON.stringify(requests, null, 2));
  }
}
