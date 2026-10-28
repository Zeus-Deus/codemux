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
  const { providers, reports, known, ready } = agents;
  const openSettings = () => useUIStore.getState().setShowSettings(true, "providers");

  let readyText: string | null = null;
  if (ready.length > 0) {
    const lead = ready.includes(selected) ? selected : ready[0];
    readyText =
      ready.length === 1 ? `${AGENT_NAME[lead]} is ready`
        : ready.length === 2
          ? `${AGENT_NAME[lead]} and ${AGENT_NAME[ready.find((p) => p !== lead)!]} are ready`
          : `${AGENT_NAME[lead]} and ${ready.length - 1} more are ready`;
  }
  // Installed but unable to run: usually signed out. A missing agent is
  // not called out; most people never meant to install every one.
  const needSetup = providers.filter((p) => {
    const report = reports[p];
    return report && providerReadiness(report) === "not_ready";
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
      {known && readyText && (
        <p
          role="status"
          className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-body-sm text-muted-foreground motion-safe:animate-in motion-safe:fade-in motion-safe:duration-150"
        >
          <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-success" />
          <span className="text-foreground/80">{readyText}</span>
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
