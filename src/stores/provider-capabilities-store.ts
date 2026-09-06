import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

import { useProviderHealth } from "@/stores/provider-health-store";
import { listChatProviderCapabilities } from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  ProviderChatCapabilities,
  ProviderHealthReport,
} from "@/tauri/types";

/** Server-side TTL on the Rust Cursor harvest cache, mirrored here so the
 *  intent freshness window below can be stated relative to it. Keep in lockstep with
 *  `CAPABILITY_CACHE_TTL` in `agent_provider/cursor/capabilities.rs`. */
export const CURSOR_CAPABILITY_TTL_MS = 5 * 60 * 1000;

/** How often the app re-harvests Cursor's catalog. Deliberately LONGER
 *  than the Rust TTL: polling exactly at the TTL means each tick has a
 *  coin-flip chance of landing inside the still-valid cache window, so
 *  every other poll is a no-op and the effective refresh period silently
 *  doubles. A margin past the TTL makes every tick a real refresh. */
export const CURSOR_CAPABILITY_REFRESH_MS =
  CURSOR_CAPABILITY_TTL_MS + 60 * 1000;

/** Grok's model and reasoning catalogue is owned by the installed CLI and
 *  can change independently of Codemux. Re-discover it on the same bounded
 *  cadence as Cursor's live ACP catalogue. */
export const GROK_CAPABILITY_REFRESH_MS = CURSOR_CAPABILITY_REFRESH_MS;

/** Whether a repeat Cursor re-harvest is worth issuing.
 *
 *  A machine without the CLI has no catalog to refresh, and the probe
 *  would try to spawn a missing binary on every attempt forever. Only a
 *  definite "not installed" suppresses it — an unknown health slot (never
 *  probed) still refreshes, because the harvest is how the picker learns
 *  Cursor exists at all. */
export function shouldRefreshCursorCapabilities(
  health: ProviderHealthReport | null,
): boolean {
  return health?.installed !== false;
}

/** Same installed-binary guard for Grok's live discovery. */
export function shouldRefreshGrokCapabilities(
  health: ProviderHealthReport | null,
): boolean {
  return health?.installed !== false;
}

/** Applies the installed-binary guard for the CLI-owned catalogs when a
 *  picker reopens past its re-harvest window. Providers whose catalogs are
 *  release-bundled have nothing to spawn, so they are always allowed. */
function shouldReharvestCapabilities(
  provider: AgentChatProviderKind,
): boolean {
  const health = useProviderHealth.getState().slots[provider]?.report ?? null;
  if (provider === "cursor") return shouldRefreshCursorCapabilities(health);
  if (provider === "grok") return shouldRefreshGrokCapabilities(health);
  return true;
}

interface ProviderCapabilitiesStore {
  claude: ProviderChatCapabilities | null;
  codex: ProviderChatCapabilities | null;
  cursor: ProviderChatCapabilities | null;
  grok: ProviderChatCapabilities | null;
  /** Step 12 Stage 3 — slot for OpenCode's live model harvest. Stays
   *  `null` until `refresh("opencode")` resolves. Failure surfaces in
   *  `opencodeError`; the slot itself stays `null` so the picker can
   *  render an empty state rather than a stale list. */
  opencode: ProviderChatCapabilities | null;
  claudeError: string | null;
  codexError: string | null;
  cursorError: string | null;
  grokError: string | null;
  opencodeError: string | null;
  /** Per-provider request lifecycle. `true` means the latest refresh settled,
   *  whether it returned models, an empty catalog, or an error. */
  loadedProviders: Partial<Record<AgentChatProviderKind, boolean>>;
  refresh: (provider: AgentChatProviderKind) => Promise<void>;
}

// Settings can render more than one model picker, and a provider harvest may
// launch a CLI. Dedupe at the store boundary so concurrent explicit intents
// never fan out into duplicate child processes.
const providerRefreshInFlight = new Map<
  AgentChatProviderKind,
  Promise<void>
>();
const providerIntentRefreshedAt = new Map<AgentChatProviderKind, number>();
// Sign-out generation counter. A harvest can spend seconds in a CLI while the
// user signs out; `resetProviderCapabilities` bumps this, and every refresh
// compares the epoch it captured at flight start before writing results. A
// stale flight that loses the race is discarded instead of resurrecting the
// just-cleared catalog in memory (and, via the persist middleware, back into
// localStorage after `clearStorage()`).
let providerResetEpoch = 0;
// Providers whose catalogue lives in an installed CLI and can change without
// a Codemux release. A later intent past this window re-harvests; every other
// provider stays a once-per-renderer intent because its catalogue ships with
// the release or has no matching server-side cache TTL.
const INTENT_REHARVEST_INTERVAL_MS: Partial<
  Record<AgentChatProviderKind, number>
> = {
  cursor: CURSOR_CAPABILITY_REFRESH_MS,
  grok: GROK_CAPABILITY_REFRESH_MS,
};
export const PROVIDER_CAPABILITIES_STORAGE_KEY =
  "codemux:provider-capabilities:v1";

export const useProviderCapabilities = create<ProviderCapabilitiesStore>()(
  persist(
    (set) => ({
      claude: null,
      codex: null,
      cursor: null,
      grok: null,
      opencode: null,
      claudeError: null,
      codexError: null,
      cursorError: null,
      grokError: null,
      opencodeError: null,
      loadedProviders: {},
      refresh: (provider) => {
        const existing = providerRefreshInFlight.get(provider);
        if (existing) return existing;

        set((state) => ({
          loadedProviders: {
            ...state.loadedProviders,
            [provider]: false,
          },
        }));

        const startedEpoch = providerResetEpoch;
        const request = (async () => {
          try {
            const caps = await listChatProviderCapabilities(provider);
            if (providerResetEpoch !== startedEpoch) return;
            set((state) => storeOk(state, provider, caps));
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            console.error(
              `[provider-capabilities] refresh(${provider}) failed:`,
              err,
            );
            if (providerResetEpoch !== startedEpoch) return;
            set((state) => storeErr(state, provider, message));
          }
        })().finally(() => {
          // A flight that straddled a reset must not touch state: writing
          // `loadedProviders` here would mark a wiped slot as settled (or
          // clobber the lifecycle of a fresher post-reset flight).
          if (providerResetEpoch === startedEpoch) {
            set((state) => ({
              loadedProviders: {
                ...state.loadedProviders,
                [provider]: true,
              },
            }));
          }
          // Only clear our own dedupe entry. After a reset the map is
          // cleared and may already hold a newer flight for this provider;
          // deleting that one would let concurrent intents fan out again.
          if (providerRefreshInFlight.get(provider) === request) {
            providerRefreshInFlight.delete(provider);
          }
        });
        providerRefreshInFlight.set(provider, request);
        return request;
      },
    }),
    {
      name: PROVIDER_CAPABILITIES_STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Capability catalogs contain public model metadata only. Errors and
      // lifecycle flags are deliberately session-local, so a missing CLI or
      // transient failure is never replayed as current state on next launch.
      partialize: (state) => ({
        claude: state.claude,
        codex: state.codex,
        cursor: state.cursor,
        grok: state.grok,
        opencode: state.opencode,
      }),
    },
  ),
);

/**
 * Opening a model picker is the intent boundary for provider discovery.
 * Mounting the app shell never calls this. A persisted catalog paints the
 * picker immediately, then the first intent in this renderer refreshes it in
 * the background. Re-renders and multiple picker surfaces share that attempt;
 * a later Cursor intent refreshes again once its CLI catalog can be stale.
 */
export function refreshProviderCapabilitiesForIntent(
  provider: AgentChatProviderKind,
): Promise<void> {
  // Health probes can launch provider runtimes too, so they share the exact
  // same explicit-intent boundary. The health store's TTL handles later
  // picker openings without another process spawn.
  const healthRefresh = useProviderHealth.getState().refresh(provider);
  const lastRefresh = providerIntentRefreshedAt.get(provider);
  // The Cursor and Grok catalogs are owned by their installed CLIs and used
  // to refresh on an app-level timer. Capability startup is now intent-driven,
  // so reopening a picker after that cadence becomes the bounded reharvest
  // trigger. Other providers retain their original once-per-renderer intent
  // behavior; their catalogs are release-bundled or harvested without the
  // matching server-side cache TTL.
  const reharvestAfterMs = INTENT_REHARVEST_INTERVAL_MS[provider];
  const refreshStillFresh =
    lastRefresh !== undefined &&
    (reharvestAfterMs === undefined ||
      Date.now() - lastRefresh < reharvestAfterMs);
  if (refreshStillFresh) return healthRefresh;
  // A re-harvest (as opposed to the first intent) always follows an earlier
  // probe, so the health slot is already populated and can gate the spawn
  // synchronously — no extra await on the picker's critical path. A definite
  // "not installed" has no CLI-owned catalog to harvest; an unknown slot
  // still harvests, since that is how the picker learns the provider exists.
  if (lastRefresh !== undefined && !shouldReharvestCapabilities(provider)) {
    return healthRefresh;
  }
  return Promise.all([
    healthRefresh,
    useProviderCapabilities.getState().refresh(provider),
  ]).then(() => {
    // `refresh` deliberately absorbs IPC/provider failures so picker
    // surfaces can render their per-provider error state. Only remember a
    // completed intent after that state proves the live harvest succeeded;
    // otherwise reopening the picker is the user's natural retry action.
    const state = useProviderCapabilities.getState();
    if (selectCapabilities(state, provider) && !selectError(state, provider)) {
      providerIntentRefreshedAt.set(provider, Date.now());
    } else {
      providerIntentRefreshedAt.delete(provider);
    }
  });
}

/** Reset process-wide intent bookkeeping between isolated store tests. */
export function _resetProviderCapabilityIntentForTests(): void {
  providerIntentRefreshedAt.clear();
}

/** Sign-out hygiene, called by the auth store when the user transitions to
 *  null: drop the persisted catalog and reset in-memory capability state so
 *  a signed-out shell doesn't keep replaying the previous session's picker
 *  cache. Catalogs are public model metadata, so this is tidiness rather
 *  than secrecy — deliberately kept simple. */
export function resetProviderCapabilities(): void {
  // Invalidate every in-flight harvest and forget its dedupe entry: a
  // doomed flight must neither write its result when it settles nor absorb
  // the next post-sign-in refresh into itself.
  providerResetEpoch += 1;
  providerRefreshInFlight.clear();
  providerIntentRefreshedAt.clear();
  useProviderCapabilities.setState({
    claude: null,
    codex: null,
    cursor: null,
    grok: null,
    opencode: null,
    claudeError: null,
    codexError: null,
    cursorError: null,
    grokError: null,
    opencodeError: null,
    loadedProviders: {},
  });
  useProviderCapabilities.persist.clearStorage();
}

/** Convenience selector: capabilities for the given provider, or null.
 *  Exhaustive switch over [`AgentChatProviderKind`] — adding a fourth
 *  provider downstream now produces a TypeScript error here rather
 *  than a silent `null` fall-through. */
export function selectCapabilities(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): ProviderChatCapabilities | null {
  switch (provider) {
    case "claude":
      return state.claude;
    case "codex":
      return state.codex;
    case "cursor":
      return state.cursor;
    case "grok":
      return state.grok;
    case "opencode":
      return state.opencode;
  }
}

/** Convenience selector: error string for a given provider, or null. */
export function selectError(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): string | null {
  switch (provider) {
    case "claude":
      return state.claudeError;
    case "codex":
      return state.codexError;
    case "cursor":
      return state.cursorError;
    case "grok":
      return state.grokError;
    case "opencode":
      return state.opencodeError;
  }
}

/** Whether this provider's latest refresh attempt has settled. A failed or
 *  empty harvest is still loaded: consumers should show their empty/error
 *  state instead of an indefinite loading indicator. */
export function selectProviderCapabilitiesLoaded(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): boolean {
  return state.loadedProviders[provider] === true;
}

/** Convenience selector: find a model by id within a provider's list.
 *
 *  One deliberate exception to the strict-match rule: the literal id
 *  `"default"`. The roster used to lead with a `"default"` alias row,
 *  so persisted drafts and thread records commonly store that id. The
 *  backend now folds the alias out of the roster whenever a concrete
 *  twin exists (`dedupe_default_alias`), which would leave those
 *  persisted ids dangling. When `"default"` is absent, `models[0]` is
 *  guaranteed to be the concrete model the alias resolved to, so we
 *  fall back to it. Any other unknown id still returns `null` — only
 *  the historical alias gets this treatment. */
export function selectModel(
  caps: ProviderChatCapabilities | null,
  modelId: string | null | undefined,
) {
  if (!caps || !modelId) return null;
  const found = caps.models.find((m) => m.id === modelId) ?? null;
  if (!found && modelId === "default") {
    return caps.models[0] ?? null;
  }
  return found;
}

// ── Internal: per-provider state-update helpers ─────────────────────
//
// Pulled out of the inline `set()` callbacks so adding a fourth
// provider doesn't require ternary surgery in two places.

function storeOk(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
  caps: ProviderChatCapabilities,
): Partial<ProviderCapabilitiesStore> {
  switch (provider) {
    case "claude":
      return { claude: caps, claudeError: null };
    case "codex":
      return { codex: caps, codexError: null };
    case "cursor":
      return { cursor: caps, cursorError: null };
    case "grok":
      return { grok: caps, grokError: null };
    case "opencode":
      return { opencode: caps, opencodeError: null };
  }
  // Exhaustive switch above; the void return is unreachable but keeps
  // TS happy when AgentChatProviderKind grows.
  return state;
}

function storeErr(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
  message: string,
): Partial<ProviderCapabilitiesStore> {
  switch (provider) {
    case "claude":
      return { claudeError: message };
    case "codex":
      return { codexError: message };
    case "cursor":
      return { cursorError: message };
    case "grok":
      return { grokError: message };
    case "opencode":
      return { opencodeError: message };
  }
  return state;
}
