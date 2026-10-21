// OpenCode chats share one server with per-chat tools; Hermes keeps one
// toolset and its credential across Stop; slash commands reach providers
// unchanged. Opt in with ORCH_LIVE_PROVIDERS including opencode and hermes.
import { mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';

export const name = 'shared-tools';
export const description = 'OpenCode footprint and per-chat identity on one server; Hermes Stop, /model and /tools.';

const q = (s) => `'${s.replaceAll("'", "''")}'`;
const OPENCODE_MODEL = process.env.ORCH_LIVE_OPENCODE_MODEL ?? 'opencode/beam-free';

/** OpenCode processes below the sandbox init, read from the host's /proc. */
function footprint(initPid) {
  const children = new Map();
  for (const pid of readdirSync('/proc').filter((p) => /^\d+$/.test(p))) {
    try {
      const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
      const ppid = Number(stat.slice(stat.lastIndexOf(')') + 2).split(' ')[1]);
      if (!children.has(ppid)) children.set(ppid, []);
      children.get(ppid).push(Number(pid));
    } catch { /* exited */ }
  }
  const below = [];
  const stack = [Number(initPid)];
  while (stack.length) {
    for (const child of children.get(stack.pop()) ?? []) { below.push(child); stack.push(child); }
  }
  let servers = 0, processes = 0, rssKb = 0;
  for (const pid of below) {
    try {
      const argv = readFileSync(`/proc/${pid}/cmdline`, 'utf8').split('\0');
      if (!argv.some((a) => a.includes('opencode'))) continue;
      processes += 1;
      if (argv.includes('serve')) servers += 1;
      rssKb += Number(/VmRSS:\s+(\d+)/.exec(readFileSync(`/proc/${pid}/status`, 'utf8'))?.[1] ?? 0);
    } catch { /* exited */ }
  }
  return { servers, processes, rss_mb: Math.round(rssKb / 1024) };
}

async function startIn(ctx, workspaceId, cwd, { provider, model = null, text, permissionMode = null, extra = null }) {
  const threadId = randomUUID();
  const paneId = await ctx.driver.invoke('agent_chat_create_pane', { workspaceId, provider, cwd, launchMode: 'new_tab', threadId });
  await ctx.driver.invoke('agent_chat_start_session', {
    paneId, provider, expectedThread: null,
    input: { thread_id: threadId, cwd, model, resume_cursor: null, permission_mode: permissionMode, effort: null,
      context_window: null, service_tier: 'default', additional_directories: [], env: null, extra },
  });
  if (text) await send(ctx, provider, threadId, text);
  return { threadId, paneId };
}

async function send(ctx, provider, threadId, text) {
  await ctx.driver.invoke('agent_chat_send_turn', {
    provider,
    input: { thread_id: threadId, text, display_text: text, skill_ids: [], skill_text: null, include_plugins: true,
      images: [], model_override: null, effort_override: null, permission_mode_override: null },
  });
}

async function settled(ctx, threadId, label) {
  await ctx.waitFor(async () => {
    const state = await ctx.codemux('thread_status', { workspace_id: ctx.env.WORKSPACE_ID, thread_id: threadId }).catch(() => null);
    return state?.settled;
  }, { timeout: 240_000, interval: 1000, label });
}

/** Content of the first assistant message matching `re`, or null. */
async function replyMatching(ctx, threadId, re) {
  const page = await ctx.assert.conversation(threadId, 200).catch(() => null);
  const messages = (page?.data ?? page)?.messages ?? [];
  return messages.find((m) => m.role === 'assistant' && re.test(m.content ?? ''))?.content ?? null;
}

async function received(ctx, receiver, marker) {
  const rows = await ctx.assert.sql(`SELECT sender_thread_id,created_by,json_extract(payload,'$.text') AS text FROM agent_chat_messages WHERE thread_id=${q(receiver)} AND json_extract(payload,'$.type')='user_message'`);
  return rows.find((row) => row.created_by === 'agent' && row.text?.includes(marker));
}

export async function run(ctx) {
  const providers = (ctx.env.ORCH_LIVE_PROVIDERS ?? '').split(',');
  const measures = {};

  if (providers.includes('opencode')) {
    measures.before = footprint(ctx.env.INIT_PID);
    // A receiver for identity proofs; it never needs to run a turn.
    const receiver = await startIn(ctx, ctx.env.WORKSPACE_ID, ctx.env.REPO, { provider: 'opencode', model: OPENCODE_MODEL });
    const same = [];
    for (let i = 0; i < 3; i += 1) {
      same.push(await startIn(ctx, ctx.env.WORKSPACE_ID, ctx.env.REPO, { provider: 'opencode', model: OPENCODE_MODEL, text: 'Do not use tools. Reply only OK.' }));
    }
    for (const chat of same) await settled(ctx, chat.threadId, 'same-directory OpenCode chat settled');
    measures.same_directory = footprint(ctx.env.INIT_PID);
    ctx.check('three chats in one directory share one OpenCode server', measures.same_directory.servers === 1, measures);

    // Each chat on the shared server acts only as itself.
    const tags = same.map(() => randomUUID().slice(0, 8));
    for (const [i, chat] of same.slice(0, 2).entries()) {
      await send(ctx, 'opencode', chat.threadId, `Call the injected Codemux thread_send tool once with thread_id ${JSON.stringify(receiver.threadId)}, client_request_id "id-${tags[i]}", delivery "queue", message "FROM-${tags[i]}". Then reply SENT. Do not call any other tool.`);
    }
    for (const [i, chat] of same.slice(0, 2).entries()) {
      const row = await ctx.waitFor(() => received(ctx, receiver.threadId, `FROM-${tags[i]}`), { timeout: 240_000, interval: 1000, label: `identity proof ${i}` });
      ctx.check(`shared-server chat ${i} sends as itself`, row.sender_thread_id === chat.threadId, { row, chat: chat.threadId });
    }

    const others = [];
    for (const suffix of ['b', 'c', 'd']) {
      const dir = join(ctx.env.ROOT, `repo-${suffix}`);
      mkdirSync(dir, { recursive: true });
      const workspace = await ctx.codemux('create_workspace', { path: dir });
      const workspaceId = workspace?.data?.workspace_id ?? workspace?.workspace_id;
      others.push(await startIn(ctx, workspaceId, dir, { provider: 'opencode', model: OPENCODE_MODEL, text: 'Do not use tools. Reply only OK.' }));
    }
    for (const chat of others) {
      await ctx.waitFor(() => replyMatching(ctx, chat.threadId, /OK/), { timeout: 240_000, interval: 1000, label: 'other-directory OpenCode chat replied' });
    }
    measures.plus_three_directories = footprint(ctx.env.INIT_PID);
    ctx.check('chats in other directories reuse the same server', measures.plus_three_directories.servers === 1, measures);
    for (const chat of [receiver, ...same, ...others]) {
      await ctx.driver.invoke('agent_chat_stop_session', { provider: 'opencode', threadId: chat.threadId });
    }
    measures.all_closed = footprint(ctx.env.INIT_PID);
    ctx.observe('opencode footprint', measures);
  }

  if (providers.includes('hermes')) {
    const profiles = await ctx.driver.invoke('hermes_profiles', {});
    const profile = profiles.find((p) => p.id === 'default');
    const catalog = await ctx.driver.invoke('hermes_catalog', { profile });
    const model = catalog.session.models.currentModelId;
    const hermes = await startIn(ctx, ctx.env.WORKSPACE_ID, ctx.env.REPO, {
      provider: 'hermes', model, permissionMode: 'dont_ask', extra: { hermes_profile: profile }, text: 'Do not use tools. Reply only READY.',
    });
    await ctx.waitFor(() => replyMatching(ctx, hermes.threadId, /READY/), { timeout: 240_000, interval: 1000, label: 'Hermes ready' });

    await send(ctx, 'hermes', hermes.threadId, '/model');
    const modelReply = await ctx.waitFor(() => replyMatching(ctx, hermes.threadId, /Current model:|Model switched|Error/), { timeout: 60_000, interval: 500, label: '/model answered' });
    ctx.check('/model reaches Hermes with no appended argument', modelReply.startsWith('Current model:'), modelReply);

    // Stop a running turn twice through the Stop command.
    await send(ctx, 'hermes', hermes.threadId, "Run this exact shell command and wait for it: python3 -c 'import time; time.sleep(120)'. Then reply DONE.");
    await ctx.waitFor(async () => {
      const state = await ctx.codemux('thread_status', { workspace_id: ctx.env.WORKSPACE_ID, thread_id: hermes.threadId }).catch(() => null);
      return state && !state.settled;
    }, { timeout: 60_000, interval: 500, label: 'Hermes turn running' });
    // Let the shell command start before Stop (a tool row appears).
    await ctx.waitFor(async () => (await ctx.assert.sql(`SELECT COUNT(*) AS n FROM agent_chat_messages WHERE thread_id=${q(hermes.threadId)} AND json_extract(payload,'$.type')='item_started'`))[0]?.n > 0
      || (await ctx.assert.sql(`SELECT COUNT(*) AS n FROM agent_chat_messages WHERE thread_id=${q(hermes.threadId)} AND json_extract(payload,'$.type')='item_completed'`))[0]?.n > 2,
      { timeout: 60_000, interval: 500, label: 'Hermes shell command started' }).catch(() => null);
    for (let i = 0; i < 2; i += 1) {
      await ctx.driver.invoke('agent_chat_interrupt_turn', { provider: 'hermes', threadId: hermes.threadId, turnId: null });
    }
    await settled(ctx, hermes.threadId, 'Hermes stopped');

    await send(ctx, 'hermes', hermes.threadId, '/tools');
    const listing = await ctx.waitFor(() => replyMatching(ctx, hermes.threadId, /Available tools/), { timeout: 60_000, interval: 500, label: '/tools answered' });
    const tools = listing.split('\n').filter((line) => /thread_send:/.test(line));
    ctx.check('one Codemux toolset after two Stops', tools.length === 1, tools);

    const tag = randomUUID().slice(0, 8);
    const receiver = await startIn(ctx, ctx.env.WORKSPACE_ID, ctx.env.REPO, { provider: 'hermes', model, permissionMode: 'dont_ask', extra: { hermes_profile: profile } });
    await send(ctx, 'hermes', hermes.threadId, `Call the injected Codemux thread_send tool once with thread_id ${JSON.stringify(receiver.threadId)}, client_request_id "stop-${tag}", delivery "queue", message "AFTER-STOP-${tag}". Then reply SENT.`);
    const row = await ctx.waitFor(() => received(ctx, receiver.threadId, `AFTER-STOP-${tag}`), { timeout: 240_000, interval: 1000, label: 'Hermes tool call after Stop' });
    ctx.check('Hermes tool call after Stop acts as the chat', row.sender_thread_id === hermes.threadId, row);
    ctx.observe('hermes', { chat: hermes.threadId, receiver: receiver.threadId, tools, modelReply });
    await ctx.openTab(await ctx.tabOf(hermes.paneId));
    await ctx.shot('pr4fix-hermes-after-stop');
  }
}
