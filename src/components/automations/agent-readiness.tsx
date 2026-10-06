import { Loader2 } from "lucide-react";

import { cn } from "@/lib/utils";
import {
  providerReadiness,
  type ProbedProviderHealth,
} from "@/stores/provider-health-store";
import type { AgentChatProviderKind } from "@/tauri/types";

export type AutomationAgent = "claude" | "codex";

export const AUTOMATION_AGENTS: ReadonlyArray<{
  value: AutomationAgent;
  label: string;
}> = [
  { value: "claude", label: "Claude Code" },
  { value: "codex", label: "Codex" },
];

/** Stable list for `useProbedProviderHealth`. */
export const AUTOMATION_PROVIDERS: readonly AgentChatProviderKind[] =
  AUTOMATION_AGENTS.map((a) => a.value);

export type AutomationAgentHealth = Partial<
  Record<AgentChatProviderKind, ProbedProviderHealth>
>;

/**
 * The agent a new automation should start on: the first one that is ready
 * on this machine, or Claude Code when none is. `null` while a probe is
 * still running, so the form does not switch agents twice.
 */
export function preferredAutomationAgent(
  health: AutomationAgentHealth,
): AutomationAgent | null {
  if (AUTOMATION_AGENTS.some((a) => health[a.value]?.pending !== false)) {
    return null;
  }
  const ready = AUTOMATION_AGENTS.find((a) => {
    const report = health[a.value]?.report;
    return report != null && providerReadiness(report) === "ready";
  });
  return ready?.value ?? "claude";
}

/** True when the local probe says the agent cannot run as-is (missing or
 *  signed out). Unknown and unverified count as usable. */
export function agentBlocked(
  health: AutomationAgentHealth,
  agent: string,
): boolean {
  const report = health[agent as AgentChatProviderKind]?.report;
  if (!report) return false;
  const readiness = providerReadiness(report);
  return readiness === "not_ready" || readiness === "not_installed";
}

/**
 * Whether the chosen agent can run here, under the Agent field. Local
 * agents are probed; a remote host's agents can't be checked from this
 * machine, so the row says so instead of guessing.
 */
export function AgentReadinessRow({
  agent,
  local,
  health,
}: {
  agent: string;
  local: boolean;
  health: AutomationAgentHealth;
}) {
  const label =
    AUTOMATION_AGENTS.find((a) => a.value === agent)?.label ?? agent;

  if (!local) {
    return (
      <p
        data-testid="automation-agent-readiness"
        data-readiness="unknown"
        className="text-body-sm leading-relaxed text-muted-foreground/70"
      >
        {label} sign-in on a remote host is checked when the automation runs.
      </p>
    );
  }

  const probe = health[agent as AgentChatProviderKind];
  if (!probe || probe.pending) {
    return (
      <div
        data-testid="automation-agent-readiness"
        data-readiness="checking"
        className="flex items-center gap-2 text-body-sm leading-relaxed text-muted-foreground/70"
      >
        <Loader2 className="size-3 shrink-0 motion-safe:animate-spin" />
        Checking {label} on this machine…
      </div>
    );
  }
  if (!probe.report) return null;

  const readiness = providerReadiness(probe.report);
  const blocked = readiness === "not_ready" || readiness === "not_installed";
  const message =
    readiness === "ready"
      ? `${label} is installed and signed in on this machine.`
      : (probe.report.message ?? `${label} is not ready on this machine.`);

  return (
    <div
      data-testid="automation-agent-readiness"
      data-readiness={readiness}
      className="flex items-start gap-2 text-body-sm leading-relaxed"
    >
      <span
        aria-hidden
        className={cn(
          "mt-1.5 size-1.5 shrink-0 rounded-full",
          readiness === "ready" ? "bg-success" : "bg-warning",
        )}
      />
      <span
        className={cn(
          "min-w-0 select-text",
          blocked ? "text-warning" : "text-muted-foreground/75",
        )}
      >
        {message}
      </span>
    </div>
  );
}
