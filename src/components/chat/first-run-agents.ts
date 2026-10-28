import { useEffect, useMemo, useRef } from "react";
import { create } from "zustand";
import { AUTOMATIC_PROVIDER_ORDER } from "@/lib/agent-chat/new-chat-defaults";
import { useDisabledProviders } from "@/lib/agent-chat/provider-enablement";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { dbGetRecentProjects } from "@/tauri/commands";
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

/** Floor between retries of a Hermes or custom agent listing that failed. */
const RELIST_MIN_MS = 30_000;

type ExtraList = "hermes" | "acp";

interface ListFailure {
  at: number;
  /** The listing failed because the agent isn't set up on this machine
   *  (Hermes answers `setup_required:` when its executable or root is
   *  missing), which is a conclusive "nothing to run here". */
  absent: boolean;
}

interface FirstRunSession {
  /** Whether the check for projects opened before this device kept a
   *  first-run flag has answered; first-run onboarding waits for it. */
  pastProjectsChecked: boolean;
  /** When a Hermes profile or custom agent listing last failed, and why.
   *  A listing that succeeded is kept by its own store. */
  listFailed: Partial<Record<ExtraList, ListFailure>>;
  /** "Continue anyway" on the Connect an agent card, for this session. */
  connectDismissed: boolean;
}

/** First-run facts that hold for the whole window session. */
export const useFirstRunSession = create<FirstRunSession>(() => ({
  pastProjectsChecked: false,
  listFailed: {},
  connectDismissed: false,
}));

/** Commands reject with the backend's error string; its code prefix says
 *  why. */
function isSetupRequired(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.startsWith("setup_required:");
}

const isHome = (w: { workspace_type?: string }) => w.workspace_type === "home";

/** Waits before each retry of a recent-projects lookup that failed. While
 *  they run, onboarding stays hidden; once every retry has failed it shows,
 *  since "Continue anyway" covers a returning user it was wrong about. */
export const PAST_PROJECTS_RETRY_MS = [2_000, 4_000, 8_000] as const;

/** A lookup is in flight or waiting to retry. */
let pastProjectsBusy = false;
let pastProjectsFailures = 0;

function settlePastProjects() {
  pastProjectsBusy = false;
  pastProjectsFailures = 0;
  useFirstRunSession.setState({ pastProjectsChecked: true });
}

function checkPastProjects() {
  pastProjectsBusy = true;
  dbGetRecentProjects(10)
    .then((rows) => {
      // Home is a chat location that may be listed, not a project.
      const workspaces = useAppStore.getState().appState?.workspaces ?? [];
      const homePaths = new Set(
        [useAppStore.getState().homeDir, ...workspaces.map((w) => w.cwd)].filter(Boolean),
      );
      if ((rows ?? []).some((row) => !homePaths.has(row.path))) {
        useUIStore.setState({ firstRunDone: true });
      }
      settlePastProjects();
    })
    .catch(() => {
      const delay = PAST_PROJECTS_RETRY_MS[pastProjectsFailures++];
      if (delay === undefined || useUIStore.getState().firstRunDone) settlePastProjects();
      else setTimeout(checkPastProjects, delay);
    });
}

/**
 * Keep `firstRunDone` current. Any project workspace ends the first run.
 * Before the flag existed nothing recorded it, so while it is still unset
 * one look at recent projects tells a returning user from a new one.
 */
function trackFirstRun(appState: AppStateSnapshot | null) {
  if (!appState || useUIStore.getState().firstRunDone) return;
  const workspaces = appState.workspaces ?? [];
  if (workspaces.some((w) => !isHome(w))) {
    useUIStore.setState({ firstRunDone: true });
    return;
  }
  if (pastProjectsBusy || useFirstRunSession.getState().pastProjectsChecked) return;
  checkPastProjects();
}
trackFirstRun(useAppStore.getState().appState);
useAppStore.subscribe((s) => trackFirstRun(s.appState));

/** Whether no project workspace is open: nothing at all, or only Home. */
function useOnlyHomeOpen(): boolean {
  return useAppStore((s) => !!s.appState && (s.appState.workspaces ?? []).every(isHome));
}

/**
 * Whether this is a first run: no project has ever been open on this
 * device. Signing an agent in or installing one opens a terminal in the
 * Home workspace, so a lone Home workspace still counts, including after a
 * restart mid-setup. First-run onboarding (the agent status line and the
 * Connect an agent card) shows only then.
 */
export function useFirstRun(): boolean {
  const onlyHome = useOnlyHomeOpen();
  const done = useUIStore((s) => s.firstRunDone);
  const checked = useFirstRunSession((s) => s.pastProjectsChecked);
  return onlyHome && !done && checked;
}

/** Whether the landing offers the ways to get a project: whenever no
 *  project workspace is open, on a first run or after closing them all. */
export function useShowProjectActions(): boolean {
  return useOnlyHomeOpen();
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
  // can run chats, so their lists keep the connect card away. A list is
  // known once its store holds one, or once Hermes reports it isn't set up
  // here (the usual answer on a fresh machine, where it is enabled but not
  // installed). Any other failed listing leaves it unknown, and the card
  // stays away rather than hide a composer that may work.
  const hermesOn = !disabled.has("hermes");
  const acpOn = !disabled.has("acp");
  useEffect(() => {
    if (!active) return;
    const list = (kind: ExtraList, load: () => Promise<unknown>) => {
      const failed = useFirstRunSession.getState().listFailed[kind];
      if (failed && Date.now() - failed.at < RELIST_MIN_MS) return;
      load().then(
        () =>
          useFirstRunSession.setState((s) => {
            const { [kind]: _cleared, ...listFailed } = s.listFailed;
            return { listFailed };
          }),
        (error: unknown) =>
          useFirstRunSession.setState((s) => ({
            listFailed: {
              ...s.listFailed,
              [kind]: { at: Date.now(), absent: isSetupRequired(error) },
            },
          })),
      );
    };
    if (hermesOn && !useHermes.getState().profiles) list("hermes", () => useHermes.getState().loadProfiles());
    if (acpOn && !useCustomAcp.getState().agents) list("acp", () => useCustomAcp.getState().loadAgents());
  }, [active, hermesOn, acpOn]);
  const connectDismissed = useFirstRunSession((s) => s.connectDismissed);
  const hermesAbsent = useFirstRunSession((s) => !!s.listFailed.hermes?.absent);
  const hermesKnown = useHermes((s) => !hermesOn || s.profiles !== null) || hermesAbsent;
  const acpKnown = useCustomAcp((s) => !acpOn || s.agents !== null);
  const extrasListed = hermesKnown && acpKnown;
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
      extrasListed &&
      !hasExtraAgent;
    return { providers, reports, known, ready, unverified, needsAgent };
  }, [providers, health, known, connectDismissed, extrasListed, hasExtraAgent]);
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
