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
  },
  {
    provider: "codex",
    model: process.env.ORCH_LIVE_CODEX_MODEL ?? "gpt-6-luna",
    effort: "low",
  },
];

/** ORCH_LIVE_SMOKE_PROVIDERS=claude,codex (default: all). Unknown names and
 *  an empty selection are errors, so a typo can't turn into a vacuous pass. */
function selectedChats() {
  const raw = process.env.ORCH_LIVE_SMOKE_PROVIDERS;
  if (raw === undefined) return CHATS;
  const wanted = raw.split(",").map((p) => p.trim()).filter(Boolean);
  const known = CHATS.map((c) => c.provider);
  const unknown = wanted.filter((p) => !known.includes(p));
  if (unknown.length) throw new Error(`ORCH_LIVE_SMOKE_PROVIDERS: unknown provider(s) ${unknown.join(", ")} (known: ${known.join(", ")})`);
  if (!wanted.length) throw new Error("ORCH_LIVE_SMOKE_PROVIDERS selects no provider");
  return CHATS.filter((c) => wanted.includes(c.provider));
}

/** A prompt whose expected reply never appears in the prompt itself, so
 *  finding it proves the model answered (the user message can't match). */
function challenge() {
  const a = 11 + Math.floor(Math.random() * 40);
  const b = 11 + Math.floor(Math.random() * 40);
  const expected = `SMOKE-${a + b}-OK`;
  const text = `Compute ${a} plus ${b}. Reply with exactly one token and nothing else: the word SMOKE, ` +
    "a hyphen, the sum as digits, a hyphen, the letters OK.";
  if (text.includes(expected)) throw new Error("smoke prompt contains its own answer");
  return { text, expected };
}

export async function run(ctx) {
  const chats = selectedChats();
  for (const chat of chats) {
    const { text, expected } = challenge();
    const { threadId, tabId } = await ctx.startChat({ ...chat, text });
    ctx.observe(`${chat.provider} thread`, threadId);
    ctx.observe(`${chat.provider} model`, chat.model);
    ctx.observe(`${chat.provider} expected reply`, expected);

    const reply = await ctx.waitFor(
      () => ctx.assert.reply(threadId, expected).catch(() => null),
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
    // The chat's transcript host lists its thread id; it must be on screen
    // and one of its assistant messages (not the user's prompt) must show
    // the reply.
    const visible = `(() => { const host = document.querySelector('[data-transcript-cache-host*="${threadId}"]');
      return !!host && host.offsetParent !== null && [...host.querySelectorAll('[data-testid="assistant-message"]')]
        .some((m) => m.innerText.includes(${JSON.stringify(expected)})); })()`;
    await ctx.waitFor(visible, { timeout: 60_000, label: `${chat.provider} reply rendered` });
    ctx.check(`${chat.provider}: reply rendered in an assistant message in the paired browser`, true);
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
  ctx.check(`ran ${chats.length} provider(s): ${chats.map((c) => c.provider).join(", ")}`, chats.length >= 1);
}
