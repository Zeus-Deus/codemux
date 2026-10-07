import { useEffect } from "react";
import { create } from "zustand";

import { agentChatProviderHealth } from "@/tauri/commands";
import type { AgentChatProviderKind, ProviderHealthReport } from "@/tauri/types";

/**
 * Probe-backed provider runtime health, cached per provider with a TTL.
 *
 * Why this exists: a provider whose CLI is missing, broken, or
 * unauthenticated used to fail silently — the chat pane just spun on
 * "Working…" until the stall watchdog fired many minutes later. The
 * chat surfaces render a status banner from this store so the user
 * learns the provider can't run BEFORE burning time on a dead session.
 *
 * The probe is backend-side (`agent_chat_provider_health`) and can cost
 * seconds (Claude spawns its sidecar), hence:
 *  - TTL cache (probes re-run at most every `HEALTH_TTL_MS` per provider);
 *  - single-flight (concurrent refreshes coalesce onto one invoke);
 *  - `force` bypasses the TTL for event-driven re-checks (a session
 *    start just failed → re-probe now so the banner explains why).
 *
 * Recovery is symmetric with failure: a banner that only ever appears is
 * worse than none, so a successful start/turn clears a stale failure
 * (`noteProviderSuccess`) and a mounted surface polls an already-unhealthy
 * provider on the TTL cadence (`reprobeUnhealthy`) — the user who fixed
 * things out-of-band (`claude login` in a terminal) gets the banner back
 * without touching the app. Mounting a surface never starts an initial
 * probe; picker/provider intent and start/send outcomes own that work.
 */

/** Re-probe interval. Matches the "every few minutes is plenty" cadence
 *  for a local-binary check without hammering CLI spawns. Doubles as the
 *  recovery-poll period for an unhealthy provider. */
const HEALTH_TTL_MS = 5 * 60 * 1000;

/** How often a mounted chat surface re-checks an UNHEALTHY provider.
 *  Same cadence as the TTL — one probe per TTL window is the most a
 *  poll can usefully do. */
export const HEALTH_REPROBE_MS = HEALTH_TTL_MS;

/** Floor between focus-triggered re-checks, so alt-tabbing through the
 *  app does not spawn a CLI probe on every switch. */
const FOCUS_REPROBE_MIN_MS = 10_000;

/** A report that means a session cannot start at all: the CLI is missing,
 *  or it is installed and signed out. Inconclusive and partial failures do
 *  not qualify; the session gets a chance to explain itself. */
export function healthBlocksSend(report: ProviderHealthReport | null): boolean {
  if (!report || report.status !== "error") return false;
  return !report.installed || !!report.login_command;
}

interface ProviderHealthSlot {
  report: ProviderHealthReport | null;
  fetchedAt: number;
  /** In-flight probe, for single-flight coalescing. */
  inFlight: Promise<void> | null;
  /** A forced probe that arrived while `inFlight` was still running and
   *  is waiting to start after it settles. Held so several forced calls
   *  in one burst share ONE follow-up instead of stacking spawns. */
  queuedForce: Promise<void> | null;
  /** Dismissal identity (`status\0message`) the user closed. A CHANGED
   *  failure re-surfaces the banner; the same one stays dismissed until
   *  the provider recovers (recovery clears it). */
  dismissedKey: string | null;
}

type SlotMap = Record<AgentChatProviderKind, ProviderHealthSlot>;

interface ProviderHealthStore {
  slots: SlotMap;
  /** Probe `provider`, respecting TTL unless `force`. Resolves when the
   *  slot is up to date (or the probe failed — failure keeps the last
   *  report rather than fabricating one). */
  refresh: (
    provider: AgentChatProviderKind,
    opts?: { force?: boolean },
  ) => Promise<void>;
  /** A start/turn just SUCCEEDED — direct evidence the provider runs.
   *  Drops a stale failure banner and re-probes for ground truth. */
  noteProviderSuccess: (provider: AgentChatProviderKind) => Promise<void>;
  /** Scheduled recovery poll: re-probe only an already-unhealthy
   *  provider. No-op while healthy. */
  reprobeUnhealthy: (provider: AgentChatProviderKind) => Promise<void>;
  /** Pre-send gate: resolves to the report that rules out a session, or
   *  `null` when sending may proceed. A cached failure is re-probed so a
   *  sign-in done elsewhere is never blocked by a stale answer. A
   *  blocking report un-dismisses the banner, which carries its fix. */
  ensureReady: (
    provider: AgentChatProviderKind,
  ) => Promise<ProviderHealthReport | null>;
  dismiss: (provider: AgentChatProviderKind) => void;
}

/** A pristine slot. Exported so tests can reset the store without
 *  re-deriving the shape every time a field is added. */
export function emptyHealthSlot(): ProviderHealthSlot {
  return {
    report: null,
    fetchedAt: 0,
    inFlight: null,
    queuedForce: null,
    dismissedKey: null,
  };
}

/** Dismissal identity. `\0` separates the two fields BECAUSE it cannot
 *  occur in a probe message — write it as an escape, never as a literal
 *  byte, or git classifies this whole file as binary. */
export function healthBannerKey(report: ProviderHealthReport): string {
  return `${report.status}\0${report.message ?? ""}`;
}

export const useProviderHealth = create<ProviderHealthStore>((set, get) => ({
  slots: {
    claude: emptyHealthSlot(),
    codex: emptyHealthSlot(),
    cursor: emptyHealthSlot(),
    grok: emptyHealthSlot(),
    hermes: emptyHealthSlot(),
    opencode: emptyHealthSlot(),
  },
  refresh: async (provider, opts) => {
    const slot = get().slots[provider];
    if (slot.inFlight) {
      // An unforced caller just wants a fresh-enough answer, and the
      // probe already running is exactly that.
      if (!opts?.force) return slot.inFlight;
      // A forced caller is reacting to an event (a send just failed)
      // that the in-flight probe STARTED BEFORE, so coalescing onto it
      // would answer with pre-event facts and the banner would never
      // explain the failure. Queue one follow-up behind it instead, and
      // hand every forced caller in the burst that same follow-up.
      if (slot.queuedForce) return slot.queuedForce;
      const queued = slot.inFlight
        // The probe chain already absorbs its own rejections; the catch
        // keeps the follow-up unconditional if that ever changes.
        .catch(() => {})
        .then(() => {
          set((state) => ({
            slots: {
              ...state.slots,
              [provider]: { ...state.slots[provider], queuedForce: null },
            },
          }));
          return get().refresh(provider, { force: true });
        });
      set((state) => ({
        slots: {
          ...state.slots,
          [provider]: { ...state.slots[provider], queuedForce: queued },
        },
      }));
      return queued;
    }
    const fresh = Date.now() - slot.fetchedAt < HEALTH_TTL_MS;
    if (fresh && !opts?.force) return;
    const probe = agentChatProviderHealth(provider)
      .then((report) => {
        set((state) => {
          const prev = state.slots[provider];
          // Recovery clears the dismissal so the NEXT failure banners
          // again; an unchanged failure keeps the user's dismissal.
          const dismissedKey =
            report.status === "ready" ? null : prev.dismissedKey;
          return {
            slots: {
              ...state.slots,
              [provider]: {
                ...prev,
                report,
                fetchedAt: Date.now(),
                inFlight: null,
                dismissedKey,
              },
            },
          };
        });
      })
      .catch((err) => {
        // Command-layer failure (mock without handler, IPC hiccup):
        // keep the previous report, just stop marking it in-flight.
        // Deliberately NOT synthesized into an error banner — the
        // banner's contract is "probe-backed fact", not "IPC weather".
        console.warn(`[provider-health] probe(${provider}) failed:`, err);
        set((state) => ({
          slots: {
            ...state.slots,
            [provider]: {
              ...state.slots[provider],
              fetchedAt: Date.now(),
              inFlight: null,
            },
          },
        }));
      });
    set((state) => ({
      slots: {
        ...state.slots,
        [provider]: { ...state.slots[provider], inFlight: probe },
      },
    }));
    return probe;
  },
  noteProviderSuccess: async (provider) => {
    const slot = get().slots[provider];
    // The overwhelmingly common case: nothing is bannered, so a
    // successful turn costs one lookup and no probe.
    if (!slot.report || slot.report.status === "ready") return;
    // A start or turn just went through, which is stronger evidence
    // than the stale failing report — hide it NOW rather than making
    // the user wait out a probe. We only ever CLEAR here; fabricating a
    // `ready` report would break the banner's probe-backed contract, so
    // the forced re-probe below re-establishes ground truth (and puts
    // the banner back if the provider really is still degraded).
    //
    // The user's dismissal is deliberately KEPT. Successful turns are
    // the common case, so wiping it here would resurrect a dismissed
    // banner on every send whenever the re-probe returns the same
    // inconclusive answer (a probe that can't classify the CLI's
    // output, say). `refresh` already clears the dismissal on a
    // genuine recovery, and a DIFFERENT failure has a different key.
    set((state) => ({
      slots: {
        ...state.slots,
        [provider]: {
          ...state.slots[provider],
          report: null,
          fetchedAt: 0,
        },
      },
    }));
    await get().refresh(provider, { force: true });
  },
  reprobeUnhealthy: async (provider) => {
    const slot = get().slots[provider];
    // Only an already-failing provider is worth polling: a healthy one
    // has nothing to recover from, and its failures arrive via the
    // start/send error paths.
    if (!slot.report || slot.report.status === "ready") return;
    // Unlike the post-failure forced probes, this is a poll with no
    // triggering event, so an in-flight probe is a perfectly good
    // answer — coalesce instead of queueing another CLI spawn.
    if (slot.inFlight) return slot.inFlight;
    await get().refresh(provider, { force: true });
  },
  ensureReady: async (provider) => {
    const cached = get().slots[provider].report;
    await get().refresh(provider, { force: healthBlocksSend(cached) });
    const report = get().slots[provider].report;
    if (!report || !healthBlocksSend(report)) return null;
    set((state) => ({
      slots: {
        ...state.slots,
        [provider]: { ...state.slots[provider], dismissedKey: null },
      },
    }));
    return report;
  },
  dismiss: (provider) => {
    set((state) => {
      const slot = state.slots[provider];
      if (!slot.report || slot.report.status === "ready") return state;
      return {
        slots: {
          ...state.slots,
          [provider]: { ...slot, dismissedKey: healthBannerKey(slot.report) },
        },
      };
    });
  },
}));

/** The report a chat surface should banner for `provider`, or `null`
 *  when healthy / not yet probed / dismissed. */
export function selectVisibleHealthReport(
  state: ProviderHealthStore,
  provider: AgentChatProviderKind,
): ProviderHealthReport | null {
  const slot = state.slots[provider];
  if (!slot.report || slot.report.status === "ready") return null;
  if (slot.dismissedKey === healthBannerKey(slot.report)) return null;
  return slot.report;
}

/** Poll an already-known unhealthy provider for recovery on the TTL
 *  cadence, and again on mount and whenever the window regains focus.
 *
 *  The poll is what closes the "fixed it, banner still up" gap: nothing
 *  in the app observes a `claude login` run in some other terminal, so
 *  without it a banner could outlive its cause until the next failed
 *  send. Focus covers the common sign-in path, which finishes in a
 *  browser and then returns to Codemux; mount covers coming back to the
 *  chat from the sign-in terminal tab. A provider with no known failure
 *  is never probed here: merely restoring a persisted chat pane is not
 *  provider intent and must never spawn a CLI. */
export function useProviderHealthRecoveryPoll(
  provider: AgentChatProviderKind | null | undefined,
): void {
  const reprobeUnhealthy = useProviderHealth((s) => s.reprobeUnhealthy);
  useEffect(() => {
    if (!provider) return;
    const timer = setInterval(() => {
      void reprobeUnhealthy(provider);
    }, HEALTH_REPROBE_MS);
    const onFocus = () => {
      const { fetchedAt } = useProviderHealth.getState().slots[provider];
      if (Date.now() - fetchedAt < FOCUS_REPROBE_MIN_MS) return;
      void reprobeUnhealthy(provider);
    };
    onFocus();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [provider, reprobeUnhealthy]);
}
