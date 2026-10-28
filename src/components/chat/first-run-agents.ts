import { useEffect, useMemo, useRef } from "react";
import { create } from "zustand";
import { AUTOMATIC_PROVIDER_ORDER } from "@/lib/agent-chat/new-chat-defaults";
import { useDisabledProviders } from "@/lib/agent-chat/provider-enablement";
import { useAppStore } from "@/stores/app-store";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useHermes } from "@/stores/hermes-store";
import {
  providerReadiness,
  useProbedProviderHealth,
  useProviderHealth,
  type ProviderReadiness,
} from "@/stores/provider-health-store";
import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  ProviderHealthReport,
} from "@/tauri/types";

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

interface FirstRunSession {
  /** Whether the first app state this session saw had no workspaces; null
   *  until one arrives. */
  startedEmpty: boolean | null;
  /** Hermes profiles and custom agents have been listed (or failed to). */
  extrasListed: boolean;
  /** "Continue anyway" on the Connect an agent card, for this session. */
  connectDismissed: boolean;
}

/** First-run facts that hold for the whole window session. */
export const useFirstRunSession = create<FirstRunSession>(() => ({
  startedEmpty: null,
  extrasListed: false,
  connectDismissed: false,
}));

function noteFirstSnapshot(appState: AppStateSnapshot | null) {
  if (!appState || useFirstRunSession.getState().startedEmpty !== null) return;
  useFirstRunSession.setState({ startedEmpty: (appState.workspaces ?? []).length === 0 });
}
noteFirstSnapshot(useAppStore.getState().appState);
useAppStore.subscribe((s) => noteFirstSnapshot(s.appState));

/**
 * Whether this is a first run: nothing has been opened yet. Signing an agent
 * in or installing one opens a terminal in the Home workspace, so when the
 * session started with no workspaces, a lone Home workspace still counts as
 * a first run. A session that started with any workspace is past its first
 * run, even after the user closes them all down to Home.
 */
export function useFirstRun(): boolean {
  const startedEmpty = useFirstRunSession((s) => s.startedEmpty);
  return useAppStore((s) => {
    if (!s.appState) return false;
    const workspaces = s.appState.workspaces ?? [];
    if (workspaces.length === 0) return true;
    return startedEmpty === true && workspaces.every((w) => w.workspace_type === "home");
  });
}

export interface FirstRunAgents {
  /** Agents probed on this machine, in new-chat order; Hermes and custom
   *  ACP agents are left out, their readiness depends on a profile. */
  providers: readonly AgentChatProviderKind[];
  /** Latest answer per agent; absent while unknown. */
  reports: Partial<Record<AgentChatProviderKind, ProviderHealthReport>>;
  /** Every probe has settled since this surface opened: safe to describe.
   *  Stays true through later re-checks, so the description doesn't blink. */
  known: boolean;
  /** Agents ready to run, in new-chat order. */
  ready: AgentChatProviderKind[];
  /** Installed agents whose probe couldn't tell whether they can run (for
   *  example a sign-in it can't check), in new-chat order. Likely fine, so
   *  they count as usable, but are never described as ready. */
  unverified: AgentChatProviderKind[];
  /** Every check answered and nothing can run: no probed agent is ready or
   *  unverified, and no Hermes profile or enabled custom agent exists. The
   *  landing then asks to connect one, unless the user continued past that
   *  this session. Holds through a re-check, so the screen does not flicker. */
  needsAgent: boolean;
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

  // Hermes and custom ACP agents aren't probed, but someone who set one up
  // can run chats, so their lists keep the connect card away.
  const hermesOn = !disabled.has("hermes");
  const acpOn = !disabled.has("acp");
  useEffect(() => {
    if (!active) return;
    const lists: Promise<unknown>[] = [];
    if (hermesOn && !useHermes.getState().profiles) lists.push(useHermes.getState().loadProfiles());
    if (acpOn && !useCustomAcp.getState().agents) lists.push(useCustomAcp.getState().loadAgents());
    void Promise.allSettled(lists).then(() => useFirstRunSession.setState({ extrasListed: true }));
  }, [active, hermesOn, acpOn]);
  const extrasListed = useFirstRunSession((s) => s.extrasListed);
  const connectDismissed = useFirstRunSession((s) => s.connectDismissed);
  const hasHermesProfile = useHermes((s) => hermesOn && (s.profiles?.length ?? 0) > 0);
  const hasCustomAgent = useCustomAcp((s) => acpOn && !!s.agents?.some((a) => a.enabled));
  const hasExtraAgent = hasHermesProfile || hasCustomAgent;

  return useMemo(() => {
    const reports: FirstRunAgents["reports"] = {};
    for (const provider of providers) {
      const report = health[provider]?.report;
      if (report) reports[provider] = report;
    }
    const withReadiness = (readiness: ProviderReadiness) =>
      providers.filter((p) => reports[p] && providerReadiness(reports[p]) === readiness);
    const ready = withReadiness("ready");
    const unverified = withReadiness("unverified");
    // Only conclusive answers gate: not installed, or installed and unable
    // to run. A probe that couldn't tell never hides the composer.
    const needsAgent =
      !connectDismissed &&
      providers.length > 0 &&
      providers.every((p) => reports[p]) &&
      ready.length === 0 &&
      unverified.length === 0 &&
      ((!hermesOn && !acpOn) || extrasListed) &&
      !hasExtraAgent;
    return { providers, reports, known, ready, unverified, needsAgent };
  }, [providers, health, known, connectDismissed, hermesOn, acpOn, extrasListed, hasExtraAgent]);
}

/** "Continue anyway": show the normal landing for the rest of the session,
 *  for when detection is wrong about what can run here. */
export function dismissConnectAgent(): void {
  useFirstRunSession.setState({ connectDismissed: true });
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
