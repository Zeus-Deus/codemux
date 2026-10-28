import { cn } from "@/lib/utils";
import { providerReadiness } from "@/stores/provider-health-store";
import { useUIStore } from "@/stores/ui-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { AGENT_NAME, type FirstRunAgents } from "./first-run-agents";

const SEPARATOR = <span aria-hidden className="text-muted-foreground/50">·</span>;

/**
 * One quiet line under the first-run landing: which agent new chats will
 * use, what still needs setting up, and a way to Settings → Providers.
 *
 * Its height is reserved from the first frame and the text fades in only
 * once every probe has answered, so nothing above it moves and no
 * half-checked state flashes by.
 */
export function AgentStatusLine({
  agents,
  selected,
}: {
  agents: FirstRunAgents;
  selected: AgentChatProviderKind;
}) {
  const { providers, reports, known, ready, unverified } = agents;
  const openSettings = () => useUIStore.getState().setShowSettings(true, "providers");

  // Lead with the agents known to run. Agents whose sign-in the probe can't
  // check are only called installed, and only when nothing is known ready.
  const anyReady = ready.length > 0;
  const named = anyReady ? ready : unverified;
  let leadText: string | null = null;
  if (named.length > 0) {
    const verb = anyReady ? "ready" : "installed";
    const lead = named.includes(selected) ? selected : named[0];
    leadText =
      named.length === 1 ? `${AGENT_NAME[lead]} is ${verb}`
        : named.length === 2
          ? `${AGENT_NAME[lead]} and ${AGENT_NAME[named.find((p) => p !== lead)!]} are ${verb}`
          : `${AGENT_NAME[lead]} and ${named.length - 1} more are ${verb}`;
  }
  // Installed but unable to run: usually signed out. A missing agent is
  // not called out; most people never meant to install every one.
  const needSetup = providers.filter((p) => {
    const report = reports[p];
    return report && providerReadiness(report) === "not_ready" &&
      // Runs on a signed-in extra account despite the CLI's own login.
      !ready.includes(p) && !unverified.includes(p);
  });
  let setupText: string | null = null;
  if (needSetup.length === 1) {
    const [provider] = needSetup;
    setupText = reports[provider]?.login_command
      ? `${AGENT_NAME[provider]} needs sign-in`
      : `${AGENT_NAME[provider]} needs setup`;
  } else if (needSetup.length > 1) {
    setupText = `${needSetup.length} more need setup`;
  }

  return (
    <div
      data-testid="agent-status-line"
      className="flex min-h-7 w-full items-center justify-center px-4"
    >
      {known && leadText && (
        <p
          role="status"
          className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-body-sm text-muted-foreground motion-safe:animate-in motion-safe:fade-in motion-safe:duration-150"
        >
          <span
            aria-hidden
            className={cn(
              "size-1.5 shrink-0 rounded-full",
              anyReady ? "bg-success" : "bg-muted-foreground/60",
            )}
          />
          <span className="text-foreground/80">{leadText}</span>
          {setupText && (
            <>
              {SEPARATOR}
              <span>{setupText}</span>
            </>
          )}
          {SEPARATOR}
          <button
            type="button"
            onClick={openSettings}
            className="rounded-sm underline decoration-muted-foreground/50 underline-offset-4 transition-colors duration-100 hover:text-foreground hover:decoration-foreground"
          >
            Set up
          </button>
        </p>
      )}
    </div>
  );
}
