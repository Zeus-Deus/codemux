// Smoke: one Claude chat and one Codex chat in the seeded repo workspace each
// answer a single prompt on a cheap model; the replies are persisted and
// rendered in the paired browser.
export const name = "smoke";
export const description =
  "Start a Claude chat and a Codex chat in the seeded repo workspace, send one prompt each, " +
  "assert the assistant replies are persisted (DB + conversation_read) and rendered.";

const CHATS = [
  {
    provider: "claude",
    model: process.env.ORCH_LIVE_CLAUDE_MODEL ?? "claude-haiku-4-5",
    effort: null,
    token: "SMOKE-CLAUDE-OK",
  },
  {
    provider: "codex",
    model: process.env.ORCH_LIVE_CODEX_MODEL ?? "gpt-6-luna",
    effort: "low",
    token: "SMOKE-CODEX-OK",
  },
];

export async function run(ctx) {
  const only = process.env.ORCH_LIVE_SMOKE_PROVIDERS?.split(",");
  for (const chat of CHATS.filter((c) => !only || only.includes(c.provider))) {
    const text = `Reply with exactly this and nothing else: ${chat.token}`;
    const { threadId, tabId } = await ctx.startChat({ ...chat, text });
    ctx.observe(`${chat.provider} thread`, threadId);
    ctx.observe(`${chat.provider} model`, chat.model);

    const reply = await ctx.waitFor(
      () => ctx.assert.reply(threadId, chat.token).catch(() => null),
      { timeout: 300_000, interval: 2_000, label: `${chat.provider} reply persisted` },
    );
    ctx.check(`${chat.provider}: assistant reply persisted (conversation_read)`, true, reply.content);

    const [session] = await ctx.assert.sql(
      `SELECT thread_id, provider, workspace_id FROM agent_chat_sessions WHERE thread_id = '${threadId}'`,
    );
    ctx.check(
      `${chat.provider}: session row bound to the workspace`,
      session?.provider === chat.provider && session?.workspace_id === ctx.env.WORKSPACE_ID,
      session,
    );
    const [user] = await ctx.assert.sql(
      `SELECT COUNT(*) AS n FROM agent_chat_search WHERE thread_id = '${threadId}' AND role = 'user'`,
    );
    ctx.check(`${chat.provider}: user message persisted`, user?.n >= 1, user);

    await ctx.openTab(tabId);
    // The chat's transcript host lists its thread id; it must be on
    // screen and show the reply.
    const visible = `(() => { const host = document.querySelector('[data-transcript-cache-host*="${threadId}"]');
      return !!host && host.offsetParent !== null && host.innerText.includes(${JSON.stringify(chat.token)}); })()`;
    await ctx.waitFor(visible, { timeout: 60_000, label: `${chat.provider} reply rendered` });
    ctx.check(`${chat.provider}: reply rendered in the paired browser`, true);
    await ctx.shot(`smoke-${chat.provider}`, {
      // Phone shell: open the workspace from its list (once it has been
      // entered it stays open); it shows the chat picked above.
      phone: async (driver) => {
        if (await driver.eval(visible)) return;
        await driver.clickSelector(".mobile-workspace-open");
        await driver.waitFor(visible, { timeout: 20_000, label: "chat open in the phone shell" });
      },
    });
  }
}
