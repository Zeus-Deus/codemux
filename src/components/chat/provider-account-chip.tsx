import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, Settings2, TriangleAlert, UserRound } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  accountName,
  accountsFor,
  DEFAULT_ACCOUNT_ID,
  useProviderAccounts,
  type AccountProvider,
} from "@/stores/provider-accounts-store";
import { useUIStore } from "@/stores/ui-store";
import {
  providerAccountSwitchThread,
  providerAccountThread,
  type ProviderAccount,
  type ThreadAccount,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

const isAccountProvider = (provider: AgentChatProviderKind): provider is AccountProvider =>
  provider === "claude" || provider === "codex";

function accountDetail(account: ProviderAccount): string {
  if (account.identity.signed_in === false) return "Not signed in";
  return [account.identity.email, account.identity.plan].filter(Boolean).join(" · ") || "Signed in";
}

/**
 * Which account this chat uses, under the composer. Shown only when the
 * provider has more than one account. Before the chat starts, a pick
 * decides the account it starts on; after, picking another account moves
 * the chat there, conversation included. When the chat has hit its plan
 * limit, the row says so and offers the switch.
 */
export function ProviderAccountChip({
  provider,
  threadId,
  disabled = false,
}: {
  provider: AgentChatProviderKind;
  threadId: string | null | undefined;
  /** A turn is running; switching would cut it off. */
  disabled?: boolean;
}) {
  const accounts = useProviderAccounts((s) => s.accounts);
  const picked = useProviderAccounts((s) => (threadId ? s.picks[threadId] : undefined));
  const usageLimit = useAgentChatStore((s) => (threadId ? (s.threads[threadId]?.usageLimit ?? null) : null));
  // A draft aimed at an existing workspace resolves that workspace's default.
  const draftWorkspace = useChatDraftStore((s) => {
    const target = Object.values(s.draftsById).find((d) => d.threadId === threadId)?.target;
    return target?.kind === "existing_workspace" ? target.workspaceId : null;
  });
  const [thread, setThread] = useState<ThreadAccount | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);
  const autoPick = useRef<string | null>(null);
  const supported = isAccountProvider(provider);
  const options = useMemo(() => (supported ? accountsFor(accounts, provider) : []), [accounts, provider, supported]);

  useEffect(() => {
    if (supported) void useProviderAccounts.getState().load().catch(() => {});
  }, [supported]);

  const reload = useCallback(async () => {
    if (!threadId || !isAccountProvider(provider)) return;
    try {
      const resolved = await providerAccountThread(threadId, provider, draftWorkspace);
      setThread(resolved);
      // A draft starts where it is shown, even if it materializes into a
      // new workspace whose own default would differ: record what the row
      // displays as the draft's pick.
      // An explicit pick stays; an automatic one follows the draft's target.
      const store = useProviderAccounts.getState();
      const existing = store.picks[threadId];
      if (!resolved.bound && (!existing || existing === autoPick.current)) {
        autoPick.current = resolved.account_id;
        store.pick(threadId, resolved.account_id);
      }
    } catch {
      setThread(null);
    }
  }, [threadId, provider, draftWorkspace]);

  // The binding is made when the chat starts; re-read once a run settles
  // so a just-started chat shows itself as pinned.
  useEffect(() => {
    if (options.length > 1) void reload();
  }, [reload, options.length, disabled]);

  if (!supported || !threadId || options.length < 2) return null;

  const bound = thread?.bound ?? false;
  const currentId = bound ? thread!.account_id : (picked ?? thread?.account_id ?? DEFAULT_ACCOUNT_ID);
  const current = options.find((a) => a.id === currentId) ?? options[0];
  const limited = bound && usageLimit !== null && usageLimit.provider === provider;
  const workspaceDefault = thread?.workspace_default ?? null;

  const choose = async (account: ProviderAccount) => {
    // A draft's pick is explicit even when it matches what resolution shows:
    // it must not change if a default changes before the first send.
    if (!bound) {
      autoPick.current = null;
      useProviderAccounts.getState().pick(threadId, account.id);
      return;
    }
    if (account.id === current.id) return;
    setSwitching(account.id);
    try {
      const outcome = await providerAccountSwitchThread(threadId, account.id);
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
      await reload();
    } catch (error) {
      toast.failure(`Couldn't switch to ${accountName(account)}`, error);
    } finally {
      setSwitching(null);
    }
  };

  const toggleWorkspaceDefault = async () => {
    if (!thread?.workspace_id) return;
    try {
      await useProviderAccounts
        .getState()
        .setDefault(provider, workspaceDefault === current.id ? null : current.id, thread.workspace_id);
      await reload();
    } catch (error) {
      toast.failure("Couldn't set the workspace default", error);
    }
  };

  return (
    <div className="flex items-center gap-2 px-3 pb-2 text-label text-muted-foreground">
      <DropdownMenu
        onOpenChange={(open) => {
          // A sign-in finished in a terminal since the last read.
          if (open) void useProviderAccounts.getState().load(true).catch(() => {});
        }}
      >
        <DropdownMenuTrigger
          disabled={disabled || switching !== null}
          aria-label={`Account: ${accountName(current)}. Change account`}
          className={cn(
            "inline-flex h-6 min-w-0 items-center gap-1.5 rounded-md px-1.5 transition-colors duration-150 hover:bg-surface-2 hover:text-foreground disabled:opacity-50",
            limited && "text-status-attention hover:text-status-attention",
          )}
        >
          {switching ? (
            <Spinner className="size-3.5" aria-hidden />
          ) : limited ? (
            <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
          ) : (
            <UserRound className="size-3.5 shrink-0" aria-hidden />
          )}
          <span className="truncate">
            {limited ? `Limit reached on ${accountName(current)}` : accountName(current)}
          </span>
          {!limited && current.identity.email && (
            <span className="truncate text-muted-foreground/70">{current.identity.email}</span>
          )}
          {limited && <span className="font-medium">· Switch account</span>}
          <ChevronDown className="size-3 shrink-0 opacity-70" aria-hidden />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          <DropdownMenuLabel className="text-caption text-muted-foreground/80">
            {bound ? "Move this chat to" : "Start this chat on"}
          </DropdownMenuLabel>
          {options.map((account) => (
            <DropdownMenuItem
              key={account.id}
              onSelect={() => void choose(account)}
              // A chat cannot start on an account with no sign-in.
              disabled={account.identity.signed_in === false && account.id !== current.id}
              className="items-start gap-2 py-1.5"
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-label font-medium text-foreground">{accountName(account)}</span>
                <span
                  className={cn(
                    "block truncate text-label",
                    account.identity.signed_in === false ? "text-status-attention" : "text-muted-foreground/80",
                  )}
                >
                  {accountDetail(account)}
                </span>
              </span>
              {account.id === current.id && <Check className="mt-0.5 size-3.5 text-foreground/70" aria-hidden />}
            </DropdownMenuItem>
          ))}
          {bound && (
            <p className="px-2 pt-1 pb-1.5 text-caption text-muted-foreground/70">
              Moving restarts the session on the other account and brings the conversation along.
            </p>
          )}
          <DropdownMenuSeparator />
          {thread?.workspace_id && (
            <DropdownMenuCheckboxItem
              checked={workspaceDefault === current.id}
              onCheckedChange={() => void toggleWorkspaceDefault()}
              onSelect={(e) => e.preventDefault()}
              className="text-label"
            >
              Use {accountName(current)} for new chats in this workspace
            </DropdownMenuCheckboxItem>
          )}
          <DropdownMenuItem className="text-label" onSelect={() => useUIStore.getState().setShowSettings(true, "providers")}>
            <Settings2 className="size-3.5" aria-hidden />
            Manage accounts…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
