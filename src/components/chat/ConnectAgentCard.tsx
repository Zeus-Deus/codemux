import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { installProvider, signInToProvider } from "@/lib/agent-chat/provider-sign-in";
import { toast } from "@/lib/toast";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { providerReadiness } from "@/stores/provider-health-store";
import { useUIStore } from "@/stores/ui-store";
import type { AgentChatProviderKind, ProviderHealthReport } from "@/tauri/types";
import { AGENT_NAME, dismissConnectAgent, type FirstRunAgents } from "./first-run-agents";
import { ProviderLogo } from "./provider-logo";

/** The agents a fresh machine is offered, each with a one-click way in.
 *  The rest live behind "More agents". */
const CARD_PROVIDERS: readonly AgentChatProviderKind[] = ["claude", "codex", "opencode"];

const openProvidersSettings = () => useUIStore.getState().setShowSettings(true, "providers");

/**
 * First-run landing when no agent on this machine can run yet: one row per
 * agent with the step that fixes it (sign in, or install), plus the way to
 * the rest of them. The landing re-checks as the user comes back, and
 * switches to the normal composer as soon as one agent can run, or when the
 * user continues anyway.
 */
export function ConnectAgentCard({ agents }: { agents: FirstRunAgents }) {
  const rows = CARD_PROVIDERS.filter((p) => agents.providers.includes(p));
  return (
    <div className="flex w-full max-w-[560px] flex-col items-center gap-6 px-4" data-testid="connect-agent">
      <div className="flex flex-col items-center gap-2 text-center" data-backdrop-calm>
        <h1 className="text-3xl font-medium tracking-tight text-foreground">
          Connect an agent to start
        </h1>
        <p className="text-body text-muted-foreground">
          Codemux runs the coding agents you already use. Set up one, and add more any time.
        </p>
      </div>
      <div className="flex w-full flex-col gap-3">
        <ul
          aria-label="Agents"
          className="w-full divide-y divide-border overflow-hidden rounded-xl border border-border bg-popover"
        >
          {rows.map((provider) => (
            <AgentRow key={provider} provider={provider} report={agents.reports[provider]} />
          ))}
          <Row
            logo={
              <span className="flex items-center -space-x-1.5" aria-hidden>
                {(["cursor", "grok"] as const).map((p) => (
                  <span key={p} className="flex size-6 items-center justify-center rounded-md bg-surface-2 ring-2 ring-popover">
                    <ProviderLogo provider={p} className="size-3.5" />
                  </span>
                ))}
              </span>
            }
            name="More agents"
            detail="Cursor, Grok, Hermes, or any ACP agent"
          >
            <Button variant="outline" size="sm" onClick={openProvidersSettings}>
              Browse agents
            </Button>
          </Row>
        </ul>
        <p className="text-center text-body-sm text-muted-foreground">
          Sign in and Install open a terminal. Install only types the command; nothing runs until
          you press Enter.
        </p>
      </div>
      {/* Detection can be wrong (an agent on a PATH the probe can't see, a
          sign-in it can't read), so the card is never a dead end. */}
      <button
        type="button"
        onClick={dismissConnectAgent}
        className="rounded-sm text-body-sm text-muted-foreground underline decoration-muted-foreground/50 underline-offset-4 transition-colors duration-100 hover:text-foreground hover:decoration-foreground"
      >
        Continue anyway
      </button>
    </div>
  );
}

function AgentRow({
  provider,
  report,
}: {
  provider: AgentChatProviderKind;
  report: ProviderHealthReport | undefined;
}) {
  const name = AGENT_NAME[provider];
  const chatGpt = useChatGptStore((s) => s.status?.phase);
  const [opening, setOpening] = useState(false);
  if (!report) return null;
  const readiness = providerReadiness(report);

  const run = (action: () => Promise<void>) => {
    setOpening(true);
    void action().finally(() => setOpening(false));
  };

  let detail: string;
  let action: ReactNode = null;
  if (readiness === "ready") {
    detail = "Ready";
  } else if (readiness === "not_installed") {
    detail = provider === "opencode" ? "Not installed · has free models" : "Not installed";
    action = (
      <Button variant="outline" size="sm" disabled={opening} onClick={() => run(() => installProvider(provider, name))}>
        Install
      </Button>
    );
  } else if (report.login_command) {
    detail = "Installed · not signed in";
    action = (
      <Button
        variant="outline"
        size="sm"
        disabled={opening}
        onClick={() => run(() => signInToProvider(provider, report.login_command, null))}
      >
        Sign in
      </Button>
    );
  } else {
    detail = "Installed · needs setup";
    action = (
      <Button variant="outline" size="sm" onClick={openProvidersSettings}>
        Set up
      </Button>
    );
  }

  // A ChatGPT plan signs Codex in without a separate account; the CLI is
  // still what runs, so Install stays next to it.
  let planAction: ReactNode = null;
  if (provider === "codex" && readiness !== "ready") {
    if (chatGpt === "connected") {
      detail = `${detail} · ChatGPT plan connected`;
    } else if (chatGpt === "pending") {
      detail = "Finish signing in in your browser";
      planAction = (
        <Button variant="ghost" size="sm" onClick={() => void useChatGptStore.getState().cancel()}>
          Cancel
        </Button>
      );
    } else {
      planAction = (
        <Button variant="ghost" size="sm" onClick={() => void startChatGptPlan()}>
          Use ChatGPT plan
        </Button>
      );
    }
  }

  return (
    <Row
      logo={
        <span className="flex size-8 items-center justify-center rounded-lg bg-surface-2" aria-hidden>
          <ProviderLogo provider={provider} className="size-4" />
        </span>
      }
      name={name}
      detail={detail}
      title={report.message ?? undefined}
      testId={`connect-agent-${provider}`}
    >
      {planAction}
      {action}
    </Row>
  );
}

async function startChatGptPlan(): Promise<void> {
  await useChatGptStore.getState().start(null);
  const error = useChatGptStore.getState().error;
  if (error) toast.failure("Couldn't start ChatGPT sign-in", error);
}

function Row({
  logo,
  name,
  detail,
  title,
  testId,
  children,
}: {
  logo: ReactNode;
  name: string;
  detail: string;
  title?: string;
  testId?: string;
  children: ReactNode;
}) {
  return (
    <li className="flex items-center gap-3 px-4 py-3" data-testid={testId}>
      <span className="flex w-11 shrink-0 justify-center">{logo}</span>
      <div className="min-w-0 flex-1" title={title}>
        <div className="text-body font-medium text-foreground">{name}</div>
        <div className="truncate text-body-sm text-muted-foreground">{detail}</div>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">{children}</div>
    </li>
  );
}
