import { create } from "zustand";
import {
  providerAccountAdd,
  providerAccountRemove,
  providerAccountRename,
  providerAccountSetDefault,
  providerAccountsList,
  providerAccountsRefresh,
  providerAccountThread,
  type AgentChatStartSessionInput,
  type ProviderAccount,
  type ProviderAccountsState,
} from "@/tauri/commands";

export type AccountProvider = "claude" | "codex";

/** Id of the CLI's own sign-in. */
export const DEFAULT_ACCOUNT_ID = "default";

/** `extra` key the backend reads for a new chat's account. */
const START_EXTRA_KEY = "provider_account_id";

interface ProviderAccountsStore {
  accounts: ProviderAccount[] | null;
  /** App-wide default per provider; absent means the CLI's own sign-in. */
  defaults: Record<string, string>;
  /** Account picked for a chat that has not started yet, by thread id. */
  picks: Record<string, string>;
  load: (force?: boolean) => Promise<void>;
  /** Also asks Codex who each home is signed in as (spawns the CLI). */
  refresh: () => Promise<void>;
  add: (provider: AccountProvider, label: string) => Promise<ProviderAccount>;
  rename: (accountId: string, label: string) => Promise<void>;
  remove: (accountId: string) => Promise<void>;
  setDefault: (provider: AccountProvider, accountId: string | null, workspaceId?: string | null) => Promise<void>;
  pick: (threadId: string, accountId: string) => void;
}

let loadFlight: Promise<void> | null = null;

function accept(state: ProviderAccountsState): void {
  useProviderAccounts.setState({ accounts: state.accounts, defaults: state.defaults });
}

export const useProviderAccounts = create<ProviderAccountsStore>((set, get) => ({
  accounts: null,
  defaults: {},
  picks: {},
  load: async (force = false) => {
    if (get().accounts && !force) return;
    loadFlight ??= providerAccountsList()
      .then(accept)
      .finally(() => {
        loadFlight = null;
      });
    await loadFlight;
  },
  refresh: async () => accept(await providerAccountsRefresh()),
  add: async (provider, label) => {
    const account = await providerAccountAdd(provider, label);
    await get().load(true);
    return account;
  },
  rename: async (accountId, label) => accept(await providerAccountRename(accountId, label)),
  remove: async (accountId) => {
    accept(await providerAccountRemove(accountId));
    set((s) => ({
      picks: Object.fromEntries(Object.entries(s.picks).filter(([, id]) => id !== accountId)),
    }));
  },
  setDefault: async (provider, accountId, workspaceId = null) =>
    accept(await providerAccountSetDefault(provider, accountId, workspaceId)),
  pick: (threadId, accountId) => set((s) => ({ picks: { ...s.picks, [threadId]: accountId } })),
}));

/** Every account of `provider`, CLI sign-in first. */
export function accountsFor(accounts: ProviderAccount[] | null, provider: string): ProviderAccount[] {
  return (accounts ?? []).filter((a) => a.provider === provider);
}

/** "Work", or the CLI sign-in's email, or "Default". */
export function accountName(account: ProviderAccount | undefined | null): string {
  if (!account) return "Removed account";
  return account.label ?? "Default";
}

/** Attach the account picked for a not-yet-started chat to its start. The
 *  backend ignores it once the thread is pinned to an account. */
export function withAccountPick(input: AgentChatStartSessionInput): AgentChatStartSessionInput {
  const picked = useProviderAccounts.getState().picks[input.thread_id];
  if (!picked) return input;
  const extra =
    input.extra && typeof input.extra === "object" && !Array.isArray(input.extra)
      ? (input.extra as Record<string, unknown>)
      : {};
  return { ...input, extra: { ...extra, [START_EXTRA_KEY]: picked } };
}

/** Whether a draft about to start on `provider` will run on a signed-in
 *  extra account, which makes the CLI's own (signed-out) login irrelevant
 *  to whether it can answer. */
export async function startsOnSignedInAccount(
  provider: string,
  threadId: string,
  workspaceId: string | null,
): Promise<boolean> {
  if (provider !== "claude" && provider !== "codex") return false;
  try {
    const store = useProviderAccounts.getState();
    await store.load(true);
    const resolved = await providerAccountThread(threadId, provider, workspaceId);
    const id = (resolved.bound ? null : useProviderAccounts.getState().picks[threadId]) ?? resolved.account_id;
    const account = accountsFor(useProviderAccounts.getState().accounts, provider).find((a) => a.id === id);
    return !!account && account.id !== DEFAULT_ACCOUNT_ID && account.identity.signed_in !== false;
  } catch {
    return false;
  }
}
