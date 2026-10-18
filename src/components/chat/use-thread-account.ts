import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  accountName,
  accountsFor,
  DEFAULT_ACCOUNT_ID,
  pickKey,
  useProviderAccounts,
  type AccountProvider,
} from "@/stores/provider-accounts-store";
import {
  providerAccountSwitchThread,
  providerAccountThread,
  type ProviderAccount,
  type ThreadAccount,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

export const isAccountProvider = (provider: AgentChatProviderKind | string): provider is AccountProvider =>
  provider === "claude" || provider === "codex";

/** One or two letters for an account's badge: "W" for Work, "SP" for Side project. */
export function accountInitials(account: ProviderAccount): string {
  const words = accountName(account).trim().split(/\s+/).filter(Boolean);
  const letters = words.length > 1 ? words[0][0] + words[1][0] : (words[0]?.[0] ?? "?");
  return letters.toUpperCase();
}

/** Name and plan, for tooltips and headers. Never the email: that stays in
 *  Settings → Providers. */
export function accountSummary(account: ProviderAccount): string {
  if (account.identity.signed_in === false) return `${accountName(account)} · Not signed in`;
  return [accountName(account), account.identity.plan].filter(Boolean).join(" · ");
}

export interface ThreadAccountView {
  /** This provider's accounts, CLI sign-in first. */
  options: ProviderAccount[];
  /** Two or more accounts: only then is there anything to show. */
  multi: boolean;
  /** The account the chat runs on, or will start on. */
  current: ProviderAccount | null;
  /** The chat has started on this provider: choosing moves it. */
  bound: boolean;
  thread: ThreadAccount | null;
  /** Account a move is under way to. */
  switching: string | null;
  /** A turn is running: moving would cut it off. */
  busy: boolean;
  /** Pick the account a draft starts on, or move a started chat. */
  choose: (account: ProviderAccount) => Promise<void>;
  toggleWorkspaceDefault: () => Promise<void>;
  /** Re-read sign-ins; asks the CLIs when an account isn't known signed in. */
  refreshIdentities: () => void;
}

/**
 * Which account a chat uses on `provider`, and the means to change it.
 * Before the chat starts on that provider a choice is a pick it starts
 * with; after, choosing another account moves the chat there, conversation
 * included. Shared by the model picker and the usage-limit notice.
 */
export function useThreadAccount(
  provider: AgentChatProviderKind | string,
  threadId: string | null | undefined,
  enabled = true,
): ThreadAccountView {
  const supported = isAccountProvider(provider);
  const accounts = useProviderAccounts((s) => s.accounts);
  const picked = useProviderAccounts((s) =>
    threadId && supported ? s.picks[pickKey(provider, threadId)] : undefined,
  );
  const streaming = useAgentChatStore((s) => (threadId ? !!s.threads[threadId]?.streaming : false));
  const revision = useProviderAccounts((s) =>
    threadId && supported ? (s.bindingRevisions[pickKey(provider, threadId)] ?? 0) : 0,
  );
  // A draft aimed at an existing workspace resolves that workspace's default.
  const draftWorkspace = useChatDraftStore((s) => {
    const target = Object.values(s.draftsById).find((d) => d.threadId === threadId)?.target;
    return target?.kind === "existing_workspace" ? target.workspaceId : null;
  });
  const [thread, setThread] = useState<ThreadAccount | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);
  const autoPick = useRef<string | null>(null);
  const options = useMemo(() => (supported ? accountsFor(accounts, provider) : []), [accounts, provider, supported]);
  const multi = enabled && supported && !!threadId && options.length > 1;

  useEffect(() => {
    if (enabled && supported) void useProviderAccounts.getState().load().catch(() => {});
  }, [enabled, supported]);

  const reload = useCallback(async () => {
    if (!threadId || !isAccountProvider(provider)) return;
    try {
      const resolved = await providerAccountThread(threadId, provider, draftWorkspace);
      setThread(resolved);
      // A draft starts where it is shown, even if it materializes into a
      // new workspace whose own default would differ: record what is
      // displayed as the draft's pick. An explicit pick stays; an automatic
      // one follows the draft's target.
      const store = useProviderAccounts.getState();
      const existing = store.picks[pickKey(provider, threadId)];
      if (!resolved.bound && (!existing || existing === autoPick.current)) {
        autoPick.current = resolved.account_id;
        store.pick(provider, threadId, resolved.account_id);
      }
    } catch {
      setThread(null);
    }
  }, [threadId, provider, draftWorkspace]);

  // The binding is made when the chat starts; re-read once a run settles
  // so a just-started chat shows itself as pinned.
  useEffect(() => {
    if (multi) void reload();
  }, [reload, multi, streaming, revision]);

  const bound = thread?.bound ?? false;
  const currentId = bound ? thread!.account_id : (picked ?? thread?.account_id ?? DEFAULT_ACCOUNT_ID);
  const current = options.find((a) => a.id === currentId) ?? options[0] ?? null;

  const choose = async (account: ProviderAccount) => {
    if (!threadId || !isAccountProvider(provider)) return;
    // A draft's pick is explicit even when it matches what is shown: it
    // must not change if a default changes before the first send.
    if (!bound) {
      autoPick.current = null;
      useProviderAccounts.getState().pick(provider, threadId, account.id);
      return;
    }
    if (account.id === current?.id || streaming || switching) return;
    setSwitching(account.id);
    try {
      const outcome = await providerAccountSwitchThread(threadId, account.id);
      // Every view of this chat (model chip, picker, limit notice) re-reads
      // where it now runs, this one included.
      useProviderAccounts.getState().bindingChanged(provider, threadId);
      // The limit belonged to the account the chat just left. Its record
      // stays in the transcript; the standing notice does not.
      useAgentChatStore.setState((s) => {
        const slice = s.threads[threadId];
        return slice?.usageLimit ? { threads: { ...s.threads, [threadId]: { ...slice, usageLimit: null } } } : s;
      });
      toast.success(`Switched to ${accountName(account)}`, {
        description: outcome.carried
          ? "The conversation continues on this account."
          : "This account starts with fresh context; the transcript stays visible here.",
      });
    } catch (error) {
      toast.failure(`Couldn't switch to ${accountName(account)}`, error);
    } finally {
      setSwitching(null);
    }
  };

  const toggleWorkspaceDefault = async () => {
    if (!thread?.workspace_id || !current || !isAccountProvider(provider)) return;
    try {
      await useProviderAccounts
        .getState()
        .setDefault(provider, thread.workspace_default === current.id ? null : current.id, thread.workspace_id);
      await reload();
    } catch (error) {
      toast.failure("Couldn't set the workspace default", error);
    }
  };

  const refreshIdentities = () => {
    // A sign-in may have finished in a terminal since the last read. Ask
    // the CLIs again when an account is not known to be signed in (keyring
    // sign-ins leave no file to see); otherwise a list is enough.
    const store = useProviderAccounts.getState();
    const unsure = options.some((a) => a.identity.signed_in !== true);
    void (unsure ? store.refresh() : store.load(true)).catch(() => {});
    if (multi) void reload();
  };

  return {
    options,
    multi,
    current,
    bound,
    thread,
    switching,
    busy: streaming,
    choose,
    toggleWorkspaceDefault,
    refreshIdentities,
  };
}
