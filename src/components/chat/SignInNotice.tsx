import { useState } from "react";

import { signInToProvider } from "@/lib/agent-chat/provider-sign-in";
import { useProviderHealth } from "@/stores/provider-health-store";
import type { AgentChatProviderKind } from "@/tauri/types";

const ACTION_CLASS =
  "shrink-0 rounded-sm px-1.5 py-0.5 text-label font-medium transition-colors duration-100 hover:bg-surface-2 disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60";

/**
 * Transcript row for a run that failed because the provider's CLI is
 * signed out. Same red line as a session error, plus the two actions that
 * fix it. The actions hide once a probe says the provider is ready, so an
 * old thread does not keep offering a sign-in that already happened.
 */
export function SignInNotice({
  provider,
  message,
  workspaceId,
}: {
  provider: AgentChatProviderKind;
  message: string;
  workspaceId: string | null;
}) {
  const report = useProviderHealth((s) => s.slots[provider].report);
  const refresh = useProviderHealth((s) => s.refresh);
  const checking = useProviderHealth((s) => !!s.slots[provider].inFlight);
  const [signingIn, setSigningIn] = useState(false);
  const ready = report?.status === "ready";
  return (
    <div
      data-testid="sign-in-notice"
      className="flex items-center gap-2 border-l-2 border-destructive/40 bg-destructive/10 py-1 pl-3 pr-1.5 text-body-sm text-destructive"
    >
      <span className="min-w-0 flex-1 select-text">{message}</span>
      {!ready && (
        <>
          <button
            type="button"
            disabled={signingIn}
            onClick={() => {
              setSigningIn(true);
              void signInToProvider(
                provider,
                report?.login_command,
                workspaceId,
              ).finally(() => setSigningIn(false));
            }}
            className={ACTION_CLASS}
          >
            Sign in
          </button>
          <button
            type="button"
            disabled={checking}
            onClick={() => void refresh(provider, { force: true })}
            className={ACTION_CLASS}
          >
            {checking ? "Checking…" : "Re-check"}
          </button>
        </>
      )}
    </div>
  );
}
