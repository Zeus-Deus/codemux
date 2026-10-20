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
  /** Account picked for a chat that has not started yet, by
   *  `pickKey(provider, threadId)`: a draft that changes provider starts
   *  over from that provider's default. */
  picks: Record<string, string>;
  /** Picks the app made to match what a draft shows, by `pickKey`, rather
   *  than ones the user chose: only these follow the app-wide default.
   *  Shared so every view of a draft agrees. */
  autoPicks: Record<string, boolean>;
  load: (force?: boolean) => Promise<void>;
  /** Also asks Codex who each home is signed in as (spawns the CLI). */
  refresh: () => Promise<void>;
  add: (provider: AccountProvider, label: string) => Promise<ProviderAccount>;
  rename: (accountId: string, label: string) => Promise<void>;
  remove: (accountId: string) => Promise<void>;
  setDefault: (provider: AccountProvider, accountId: string | null) => Promise<void>;
  pick: (provider: AccountProvider, threadId: string, accountId: string, auto?: boolean) => void;
  /** Ask the composer's model picker to open on `provider`'s accounts
   *  (the usage-limit notice's "Choose account…"). */
  pickerRequest: { provider: AccountProvider; threadId: string; seq: number } | null;
  /** Bumped by `pickKey(provider, threadId)` when a chat moves account, so
   *  every view of that chat re-reads where it runs. */
  bindingRevisions: Record<string, number>;
  bindingChanged: (provider: AccountProvider, threadId: string) => void;
  requestPicker: (provider: AccountProvider, threadId: string) => void;
}

export const pickKey = (provider: string, threadId: string) => `${provider}:${threadId}`;

let loadFlight: Promise<void> | null = null;

function accept(state: ProviderAccountsState): void {
  useProviderAccounts.setState({ accounts: state.accounts, defaults: state.defaults });
}

export const useProviderAccounts = create<ProviderAccountsStore>((set, get) => ({
  accounts: null,
  defaults: {},
  picks: {},
  autoPicks: {},
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
  setDefault: async (provider, accountId) => accept(await providerAccountSetDefault(provider, accountId)),
  pick: (provider, threadId, accountId, auto = false) =>
    set((s) => {
      const key = pickKey(provider, threadId);
      return { picks: { ...s.picks, [key]: accountId }, autoPicks: { ...s.autoPicks, [key]: auto } };
    }),
  pickerRequest: null,
  bindingRevisions: {},
  bindingChanged: (provider, threadId) =>
    set((s) => {
      const key = pickKey(provider, threadId);
      return { bindingRevisions: { ...s.bindingRevisions, [key]: (s.bindingRevisions[key] ?? 0) + 1 } };
    }),
  requestPicker: (provider, threadId) =>
    set((s) => ({ pickerRequest: { provider, threadId, seq: (s.pickerRequest?.seq ?? 0) + 1 } })),
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
export function withAccountPick(
  provider: AccountProvider,
  input: AgentChatStartSessionInput,
): AgentChatStartSessionInput {
  const picked = useProviderAccounts.getState().picks[pickKey(provider, input.thread_id)];
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
): Promise<boolean> {
  if (provider !== "claude" && provider !== "codex") return false;
  try {
    const store = useProviderAccounts.getState();
    await store.load(true);
    const resolved = await providerAccountThread(threadId, provider);
    const id = (resolved.bound ? null : useProviderAccounts.getState().picks[pickKey(provider, threadId)]) ?? resolved.account_id;
    if (id === DEFAULT_ACCOUNT_ID) return false;
    const find = () => accountsFor(useProviderAccounts.getState().accounts, provider).find((a) => a.id === id);
    // Not known signed in: ask before refusing. A keyring sign-in leaves no
    // file to see, so neither "not probed yet" nor an answer from before a
    // terminal sign-in is final. This only runs when the CLI's own login
    // would otherwise block the draft.
    if (find()?.identity.signed_in !== true) await store.refresh();
    return find()?.identity.signed_in === true;
  } catch {
    return false;
  }
}
