import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import {
  providerReadiness,
  useProbedProviderHealth,
  type ProviderReadiness,
} from "@/stores/provider-health-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { ProviderLogo } from "./provider-logo";

/** Hermes is left out: its readiness depends on which profile is chosen,
 *  and the model picker already handles that. */
const STRIP_PROVIDERS: readonly AgentChatProviderKind[] = [
  "claude",
  "codex",
  "cursor",
  "grok",
  "opencode",
];

const PROVIDER_NAME: Record<AgentChatProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  hermes: "Hermes",
  opencode: "OpenCode",
};

const READINESS_COPY: Record<ProviderReadiness, string> = {
  ready: "Ready",
  unverified: "Unverified",
  not_ready: "Needs setup",
  not_installed: "Not installed",
};

const READINESS_DOT: Record<ProviderReadiness, string> = {
  ready: "bg-success",
  unverified: "bg-warning/60",
  not_ready: "bg-warning",
  not_installed: "bg-muted-foreground/40",
};

/**
 * First-run row of the agents Codemux can drive, each with what its local
 * probe found: ready, needs setup (usually a sign-in), or not installed.
 * Picking an installed one switches the composer to it; the provider banner
 * then explains anything that still needs fixing.
 *
 * The row's height is reserved from the first frame and the chips fade in
 * only once every probe has answered, so the composer above never moves
 * and no half-checked state flashes by.
 */
export function AgentAvailabilityStrip({
  selected,
  onSelect,
}: {
  selected: AgentChatProviderKind;
  onSelect: (provider: AgentChatProviderKind) => void;
}) {
  const health = useProbedProviderHealth(STRIP_PROVIDERS, true);
  const settled = STRIP_PROVIDERS.every((p) => health[p]?.pending === false);
  const answered = STRIP_PROVIDERS.flatMap((provider) => {
    const report = health[provider]?.report;
    return report ? [{ provider, report }] : [];
  });

  return (
    <div
      data-testid="agent-availability-strip"
      className="flex min-h-7 w-full items-center justify-center px-4"
    >
      {settled && answered.length > 0 && (
        <div
          role="group"
          aria-label="Agents"
          className="flex flex-wrap items-center justify-center gap-1.5 motion-safe:animate-in motion-safe:fade-in motion-safe:duration-150"
        >
          <Eyebrow className="mr-1">Agents</Eyebrow>
          {answered.map(({ provider, report }) => {
            const readiness = providerReadiness(report);
            const name = PROVIDER_NAME[provider];
            const isSelected = provider === selected;
            return (
              <button
                key={provider}
                type="button"
                data-testid={`agent-chip-${provider}`}
                data-readiness={readiness}
                aria-pressed={isSelected}
                disabled={readiness === "not_installed"}
                title={report.message ?? `${name} is ready`}
                onClick={() => onSelect(provider)}
                className={cn(
                  "flex h-7 items-center gap-1.5 rounded-md border border-hairline px-2 text-label transition-colors duration-100",
                  isSelected
                    ? "bg-surface-3 text-foreground"
                    : "text-muted-foreground hover:bg-surface-2 hover:text-foreground",
                  "disabled:pointer-events-none disabled:opacity-60",
                )}
              >
                <span aria-hidden className="flex">
                  <ProviderLogo provider={provider} className="size-3.5" />
                </span>
                <span className="font-medium">{name}</span>
                <span
                  aria-hidden
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    READINESS_DOT[readiness],
                  )}
                />
                <span className="text-muted-foreground">
                  {READINESS_COPY[readiness]}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
