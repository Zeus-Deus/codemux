import { ProviderLogo } from "@/components/chat/provider-logo";
import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import { accountName, useProviderAccounts } from "@/stores/provider-accounts-store";
import type { AccountUsage, ProviderQuota } from "@/tauri/commands";
import { formatCount, formatMoney, formatPercent, formatTokens, isKnownProvider, seriesLabel } from "./usage-format";
import { limitWindowLabel, remainingPercent } from "./usage-limits";
import type { UsageMetric } from "./usage-preferences";

/** The windows worth a glance in a row: the short one and the weekly. */
const GLANCE_KINDS = new Set(["five_hour", "seven_day"]);

const value = (usage: AccountUsage, metric: UsageMetric) =>
  metric === "cost" ? usage.cost_usd : usage.tokens;

const format = (n: number, metric: UsageMetric) => (metric === "cost" ? formatMoney(n) : formatTokens(n));

/** Fill tone by quota left, matching Limits. */
function leftTone(left: number): string {
  if (left <= 15) return "text-status-attention";
  return "text-muted-foreground";
}

/**
 * Settings → Usage: the period split by provider account, for providers
 * the user signed in to more than once. Each row is one account: its share
 * of that provider's work and how much of its plan is left, so it is clear
 * which account to move a chat to.
 */
export function AccountsSection({
  accounts,
  quota,
  metric,
  onOpenLimits,
}: {
  accounts: AccountUsage[];
  quota: Record<string, ProviderQuota>;
  metric: UsageMetric;
  onOpenLimits: () => void;
}) {
  const known = useProviderAccounts((s) => s.accounts);
  const providers = [...new Set(accounts.map((a) => a.provider))];
  if (providers.length === 0) return null;

  return (
    <section className="flex flex-col gap-4">
      <div className="flex min-w-0 flex-col gap-1">
        <Eyebrow>By account</Eyebrow>
        <p className="text-body-sm text-muted-foreground">
          Each account keeps its own history and plan limits. Move a chat to another account from the composer.
        </p>
      </div>
      <div className="divide-y divide-border/50 rounded-lg border border-border/60">
        {providers.map((provider) => {
          const rows = accounts.filter((a) => a.provider === provider);
          const total = rows.reduce((n, a) => n + value(a, metric), 0);
          return (
            <div key={provider} className="flex flex-col gap-1 px-4 py-3">
              <div className="flex items-center gap-2 pb-1">
                {isKnownProvider(provider) && <ProviderLogo provider={provider} className="h-[16px] w-[16px]" />}
                <span className="text-body-sm font-medium">{seriesLabel(provider)}</span>
              </div>
              {rows.map((usage) => {
                const account = known?.find((a) => a.provider === provider && a.id === usage.account_id);
                const key = usage.account_id === "default" ? provider : `${provider}@${usage.account_id}`;
                const windows = (quota[key]?.windows ?? []).filter((w) => GLANCE_KINDS.has(w.kind));
                const share = total > 0 ? value(usage, metric) / total : 0;
                const name = account ? accountName(account) : (usage.label ?? "Default");
                return (
                  <div
                    key={usage.account_id}
                    className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-5 gap-y-1 py-1.5 @container sm:grid-cols-[minmax(9rem,14rem)_minmax(0,1fr)_auto_auto]"
                  >
                    <span className="flex min-w-0 flex-col">
                      <span className="truncate text-body">{name}</span>
                      <span className="truncate text-label text-muted-foreground">
                        {account?.identity.email ?? (usage.account_id === "default" ? "CLI sign-in" : "")}
                      </span>
                    </span>
                    <span className="hidden items-center gap-2 sm:flex" aria-label={`${formatPercent(share)} of ${seriesLabel(provider)}`}>
                      <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted-foreground/15">
                        <span className="block h-full rounded-full bg-foreground/55" style={{ width: `${Math.round(share * 100)}%` }} />
                      </span>
                      <span className="w-10 shrink-0 text-right font-mono text-label tabular-nums text-muted-foreground">
                        {formatPercent(share)}
                      </span>
                    </span>
                    <button
                      type="button"
                      onClick={onOpenLimits}
                      className="hidden min-w-[9rem] justify-end gap-3 rounded-sm text-label tabular-nums outline-none focus-visible:ring-2 focus-visible:ring-ring/60 sm:flex"
                      title="Open Limits"
                    >
                      {windows.length > 0 ? (
                        windows.map((w) => (
                          <span key={w.kind} className={cn("whitespace-nowrap", leftTone(remainingPercent(w)))}>
                            {limitWindowLabel(w)} <span className="font-mono">{remainingPercent(w)}%</span> left
                          </span>
                        ))
                      ) : (
                        <span className="text-muted-foreground/70">No limits read</span>
                      )}
                    </button>
                    <span className="flex flex-col items-end">
                      <span className="font-mono text-body tabular-nums">{format(value(usage, metric), metric)}</span>
                      <span className="text-label text-muted-foreground">
                        {formatCount(usage.session_count)} {usage.session_count === 1 ? "session" : "sessions"}
                      </span>
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </section>
  );
}
