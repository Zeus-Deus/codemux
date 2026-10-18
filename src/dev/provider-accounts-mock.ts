/**
 * Dev-mock backend for several accounts per provider: Claude has the CLI's
 * own sign-in plus a "Work" account; Codex has its own sign-in plus a
 * signed-out "Side project" account. Chats start on the app default and
 * can be switched; usage and limits carry a per-account split.
 */

type Args = Record<string, unknown>;

interface MockAccount {
  id: string;
  provider: "claude" | "codex";
  label: string | null;
  home: string;
  identity: { signed_in: boolean | null; email: string | null; organization: string | null; plan: string | null };
  login_command: string;
}

const ROOT = "/home/dev/.config/codemux/provider-accounts";

let accounts: MockAccount[] = [
  {
    id: "default",
    provider: "claude",
    label: null,
    home: "/home/dev/.claude",
    identity: { signed_in: true, email: "dev@personal.example", organization: null, plan: "Claude Max" },
    login_command: "claude auth login",
  },
  {
    id: "claude-work-3f9a1c",
    provider: "claude",
    label: "Work",
    home: `${ROOT}/claude/claude-work-3f9a1c`,
    identity: { signed_in: true, email: "dev@acme.example", organization: "Acme", plan: "Claude Team" },
    login_command: `CLAUDE_CONFIG_DIR='${ROOT}/claude/claude-work-3f9a1c' claude auth login`,
  },
  {
    id: "default",
    provider: "codex",
    label: null,
    home: "/home/dev/.codex",
    identity: { signed_in: true, email: "dev@personal.example", organization: null, plan: "ChatGPT Pro" },
    login_command: "codex login",
  },
  {
    id: "codex-side-project-8b2e40",
    provider: "codex",
    label: "Side project",
    home: `${ROOT}/codex/codex-side-project-8b2e40`,
    identity: { signed_in: false, email: null, organization: null, plan: null },
    login_command: `CODEX_HOME='${ROOT}/codex/codex-side-project-8b2e40' codex login`,
  },
];
const defaults: Record<string, string> = { claude: "claude-work-3f9a1c" };
const workspaceDefaults = new Map<string, Record<string, string>>();
const bindings = new Map<string, string>();
let seq = 0;

const state = () => ({ accounts, defaults: { ...defaults } });

const find = (id: string) => accounts.find((a) => a.id === id && a.id !== "default");

function slug(label: string) {
  return label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 24);
}

/** Extra limits for the extra accounts, keyed like the backend's
 *  `provider@account`. */
export function mockAccountQuota(now: number): Record<string, unknown> {
  const resetIn = (mins: number) => now + mins * 60_000;
  return {
    "claude@claude-work-3f9a1c": {
      windows: [
        { kind: "five_hour", used_pct: 99.4, resets_at_ms: resetIn(47), label: "five_hour", window_mins: 300 },
        { kind: "seven_day", used_pct: 71, resets_at_ms: resetIn(4_100), label: "seven_day", window_mins: 10_080 },
      ],
      plan_label: "Claude Team",
      auth_mode: "subscription",
      received_at_ms: now,
    },
  };
}

/** Per-account split of the usage fixture's Claude and Codex lanes. */
export function withAccountUsage(summary: unknown): unknown {
  const s = summary as {
    providers: { provider: string; tokens: number; cost_usd: number; session_count: number }[];
  };
  const lane = (p: string) => s.providers.find((x) => x.provider === p);
  const split = (p: string, id: string, label: string | null, share: number) => {
    const l = lane(p);
    return {
      provider: p,
      account_id: id,
      label,
      tokens: Math.round((l?.tokens ?? 0) * share),
      cost_usd: (l?.cost_usd ?? 0) * share,
      session_count: Math.max(0, Math.round((l?.session_count ?? 0) * share)),
    };
  };
  return {
    ...(summary as object),
    accounts: [
      split("claude", "default", null, 0.38),
      split("claude", "claude-work-3f9a1c", "Work", 0.62),
      split("codex", "default", null, 1),
      split("codex", "codex-side-project-8b2e40", "Side project", 0),
    ],
  };
}

export function createProviderAccountsMock() {
  const handlers: Record<string, (a: Args) => unknown> = {
    provider_accounts_list: () => state(),
    provider_accounts_refresh: async () => {
      await new Promise((r) => setTimeout(r, 250));
      return state();
    },
    provider_account_add: (a) => {
      const provider = a.provider === "codex" ? "codex" : "claude";
      const label = String(a.label ?? "").trim();
      if (!label) return Promise.reject("Give the account a name.");
      const id = `${provider}-${slug(label) || "account"}-${(++seq).toString(16).padStart(6, "0")}`;
      const home = `${ROOT}/${provider}/${id}`;
      const env = provider === "claude" ? "CLAUDE_CONFIG_DIR" : "CODEX_HOME";
      const account: MockAccount = {
        id,
        provider,
        label,
        home,
        identity: { signed_in: false, email: null, organization: null, plan: null },
        login_command: `${env}='${home}' ${provider === "claude" ? "claude auth login" : "codex login"}`,
      };
      accounts = [...accounts, account];
      return account;
    },
    provider_account_rename: (a) => {
      accounts = accounts.map((x) => (x.id === a.accountId && x.id !== "default" ? { ...x, label: String(a.label) } : x));
      return state();
    },
    provider_account_remove: (a) => {
      accounts = accounts.filter((x) => x.id !== a.accountId || x.id === "default");
      for (const [p, id] of Object.entries(defaults)) if (id === a.accountId) delete defaults[p];
      return state();
    },
    provider_account_set_default: (a) => {
      const provider = String(a.provider);
      const id = (a.accountId as string | null) ?? null;
      if (a.workspaceId) {
        const scoped = workspaceDefaults.get(String(a.workspaceId)) ?? {};
        if (id) scoped[provider] = id;
        else delete scoped[provider];
        workspaceDefaults.set(String(a.workspaceId), scoped);
      } else if (!id || id === "default") delete defaults[provider];
      else defaults[provider] = id;
      return state();
    },
    provider_account_open_sign_in: (a) => {
      const account = find(String(a.accountId));
      // Pretend the sign-in finished in the terminal.
      if (account) account.identity = { signed_in: true, email: "dev@side.example", organization: null, plan: "ChatGPT Plus" };
      return undefined;
    },
    provider_account_thread: (a) => {
      const thread = String(a.threadId);
      const provider = String(a.provider);
      const bound = bindings.get(`${provider}:${thread}`);
      const workspace = "ws-mock";
      const scoped = workspaceDefaults.get(workspace)?.[provider] ?? null;
      const started = !thread.startsWith("draft");
      return {
        account_id: bound ?? scoped ?? defaults[provider] ?? "default",
        bound: started,
        workspace_id: started ? workspace : null,
        workspace_default: scoped,
      };
    },
    provider_account_switch_thread: async (a) => {
      await new Promise((r) => setTimeout(r, 500));
      const thread = String(a.threadId);
      const id = String(a.accountId);
      const provider = accounts.find((x) => x.id === id)?.provider ?? "claude";
      bindings.set(`${provider}:${thread}`, id);
      return { account_id: id, carried: true };
    },
  };
  return { handlers };
}
