import { useEffect, useMemo, useRef } from "react";
import { AUTOMATIC_PROVIDER_ORDER } from "@/lib/agent-chat/new-chat-defaults";
import { useDisabledProviders } from "@/lib/agent-chat/provider-enablement";
import { useAppStore } from "@/stores/app-store";
import {
  providerReadiness,
  useProbedProviderHealth,
  useProviderHealth,
} from "@/stores/provider-health-store";
import type { AgentChatProviderKind, ProviderHealthReport } from "@/tauri/types";

export const AGENT_NAME: Record<AgentChatProviderKind, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  hermes: "Hermes",
  acp: "Custom agent",
  opencode: "OpenCode",
};

/** Floor between re-checks the landing starts itself, so alt-tabbing back
 *  and forth does not spawn a CLI probe on every switch. */
const REPROBE_MIN_MS = 10_000;

let sawNoWorkspaces = false;

/**
 * Whether this is a first run: nothing has been opened yet. Signing an agent
 * in or installing one opens a terminal in the Home workspace, so once this
 * session has seen zero workspaces, a lone Home workspace still counts as a
 * first run. An app that starts with only Home is past its first run.
 */
export function useFirstRun(): boolean {
  return useAppStore((s) => {
    if (!s.appState) return false;
    const workspaces = s.appState.workspaces ?? [];
    if (workspaces.length === 0) {
      sawNoWorkspaces = true;
      return true;
    }
    return sawNoWorkspaces && workspaces.every((w) => w.workspace_type === "home");
  });
}

export interface FirstRunAgents {
  /** Agents offered on this machine, in new-chat order; Hermes and custom
   *  ACP agents are left out, their readiness depends on a profile. */
  providers: readonly AgentChatProviderKind[];
  /** Latest answer per agent; absent while unknown. */
  reports: Partial<Record<AgentChatProviderKind, ProviderHealthReport>>;
  /** Every probe has settled since this surface opened: safe to describe.
   *  Stays true through later re-checks, so the description doesn't blink. */
  known: boolean;
  /** Agents ready to run, in new-chat order. */
  ready: AgentChatProviderKind[];
  /** Every agent answered and none can run: the landing asks to connect one.
   *  Holds through a re-check, so the screen does not flicker while it runs. */
  noneReady: boolean;
}

/** Probe the first-run agents while `active` and summarize what is usable. */
export function useFirstRunAgents(active: boolean): FirstRunAgents {
  const disabled = useDisabledProviders();
  const providers = useMemo(
    () => AUTOMATIC_PROVIDER_ORDER.filter((p) => !disabled.has(p)),
    [disabled],
  );
  const health = useProbedProviderHealth(providers, active);
  // An answer cached from before this surface opened may be stale, so wait
  // for this opening's probes once; after that, re-checks keep the old
  // answer on screen until the new one lands.
  const settledOnce = useRef(false);
  if (providers.every((p) => health[p]?.pending === false)) settledOnce.current = true;
  const known = settledOnce.current;
  return useMemo(() => {
    const reports: FirstRunAgents["reports"] = {};
    for (const provider of providers) {
      const report = health[provider]?.report;
      if (report) reports[provider] = report;
    }
    const ready = providers.filter(
      (p) => reports[p] && providerReadiness(reports[p]) === "ready",
    );
    const noneReady =
      providers.length > 0 && providers.every((p) => reports[p]) && ready.length === 0;
    return { providers, reports, known, ready, noneReady };
  }, [providers, health, known]);
}

/**
 * Re-check `providers` when the landing mounts and whenever the window
 * regains focus, while `active`. Agent sign-in and install happen in a
 * terminal tab or a browser, so coming back to Codemux is the moment the
 * answer may have changed.
 */
export function useReprobeOnReturn(
  providers: readonly AgentChatProviderKind[],
  active: boolean,
): void {
  // Active from the first frame means the landing was opened again over
  // answers it already had, typically on the way back from the sign-in or
  // install terminal: check straight away. Becoming active later means the
  // first probes just answered, so the floor applies.
  const activeOnMount = useRef(active);
  useEffect(() => {
    if (!active) return;
    const reprobe = (minAgeMs: number) => {
      const { slots, refresh } = useProviderHealth.getState();
      for (const provider of providers) {
        const slot = slots[provider];
        if (slot.inFlight || slot.queuedForce) continue;
        if (Date.now() - Math.max(slot.fetchedAt, slot.lastAttemptAt) < minAgeMs) continue;
        void refresh(provider, { force: true });
      }
    };
    reprobe(activeOnMount.current ? 0 : REPROBE_MIN_MS);
    activeOnMount.current = false;
    const onFocus = () => reprobe(REPROBE_MIN_MS);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [providers, active]);
}
