import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { accountName } from "@/stores/provider-accounts-store";
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

/** Above the model list: the account the chat uses on this provider. Its
 *  tooltip says what choosing another account does. */
export function AccountListHeader({ view, providerLabel }: { view: ThreadAccountView; providerLabel: string }) {
  const account = view.current;
  if (!account) return null;
  const moving = view.switching ? view.options.find((a) => a.id === view.switching) : null;
  const explainer = view.bound
    ? "Picking another account moves this chat there and keeps the conversation."
    : "This chat starts on the account you pick.";
  return (
    <TooltipProvider delayDuration={250}>
      <Tooltip>
        <TooltipTrigger asChild>
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
            {/* The tooltip is hover-only; screen readers get it here. */}
            <span className="sr-only">{explainer}</span>
          </div>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} aria-hidden>
          {explainer}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
