import { Check, Settings2 } from "lucide-react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { accountName } from "@/stores/provider-accounts-store";
import { useUIStore } from "@/stores/ui-store";
import type { ProviderAccount } from "@/tauri/commands";
import { accountInitials, type ThreadAccountView } from "../use-thread-account";

/**
 * Initials on the bottom-right of a provider logo, telling accounts of one
 * provider apart. `current` marks the account the chat uses.
 */
export function AccountBadge({
  account,
  current = false,
  size = "sm",
}: {
  account: ProviderAccount;
  current?: boolean;
  size?: "sm" | "md";
}) {
  return (
    <span
      aria-hidden
      data-testid="provider-account-badge"
      className={cn(
        "pointer-events-none absolute flex items-center justify-center rounded-full border border-background font-bold leading-none tabular-nums",
        size === "sm"
          ? "-right-1.5 -bottom-1 h-[11px] min-w-[11px] px-[2px] text-[7px]"
          : "-right-1.5 -bottom-1.5 h-[13px] min-w-[13px] px-[2px] text-[8px]",
        current ? "bg-primary text-primary-foreground" : "bg-surface-2 text-muted-foreground",
      )}
    >
      {accountInitials(account)}
    </span>
  );
}

/** Above the model list: the account the chat uses on this provider. */
export function AccountListHeader({ view, providerLabel }: { view: ThreadAccountView; providerLabel: string }) {
  const account = view.current;
  if (!account) return null;
  const moving = view.switching ? view.options.find((a) => a.id === view.switching) : null;
  return (
    <div
      className="flex items-center gap-2 border-b border-border/60 bg-muted/20 px-3 py-1.5 text-label"
      data-testid="provider-account-header"
    >
      {moving ? (
        <span className="flex min-w-0 flex-1 items-center gap-1.5 text-muted-foreground">
          <Spinner className="size-3" aria-hidden />
          <span className="truncate">Moving to {accountName(moving)}…</span>
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate">
          <span className="font-medium text-foreground">{accountName(account)}</span>
          {account.identity.signed_in === false ? (
            <span className="ml-1.5 text-status-attention">Not signed in</span>
          ) : account.identity.plan ? (
            <span className="ml-1.5 text-muted-foreground">{account.identity.plan}</span>
          ) : null}
        </span>
      )}
      <span className="shrink-0 rounded-sm border border-border/60 px-1.5 text-caption text-muted-foreground">
        {providerLabel}
      </span>
    </div>
  );
}

/** Below the picker: what choosing an account does, the workspace default,
 *  and the way to Settings → Providers. */
export function AccountFooter({ view, onManage }: { view: ThreadAccountView; onManage: () => void }) {
  const account = view.current;
  if (!account) return null;
  const workspaceDefault = view.thread?.workspace_default ?? null;
  const item =
    "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-label text-foreground/90 outline-none transition-colors duration-150 hover:bg-muted focus-visible:bg-muted";
  return (
    <div className="flex flex-col gap-px border-t border-border/60 p-1" data-testid="provider-account-footer">
      <p className="px-2 py-1 text-caption text-muted-foreground">
        {view.bound
          ? view.busy
            ? "Accounts can be switched once this turn finishes."
            : "Picking another account moves this chat there and keeps the conversation."
          : "This chat starts on the account you pick."}
      </p>
      {/* Defaults are set on the desktop; paired clients only switch. */}
      {view.thread?.workspace_id && !isRemoteClient() && (
        <button
          type="button"
          role="menuitemcheckbox"
          aria-checked={workspaceDefault === account.id}
          className={item}
          onClick={() => void view.toggleWorkspaceDefault()}
        >
          <span
            aria-hidden
            className={cn(
              "flex size-3.5 items-center justify-center rounded-[3px] border border-border",
              workspaceDefault === account.id && "border-primary bg-primary text-primary-foreground",
            )}
          >
            {workspaceDefault === account.id && <Check className="size-2.5" />}
          </span>
          Use {accountName(account)} for new chats in this workspace
        </button>
      )}
      <button
        type="button"
        className={item}
        onClick={() => {
          onManage();
          useUIStore.getState().setShowSettings(true, "providers");
        }}
      >
        <Settings2 className="size-3.5 text-muted-foreground" aria-hidden />
        Manage accounts…
      </button>
    </div>
  );
}
