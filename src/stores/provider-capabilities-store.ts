import { useEffect } from "react";
import { create } from "zustand";

import { useProviderHealth } from "@/stores/provider-health-store";
import { listChatProviderCapabilities } from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  ProviderChatCapabilities,
  ProviderHealthReport,
} from "@/tauri/types";

/** Server-side TTL on the Rust Cursor harvest cache, mirrored here so the
 *  poll below can be stated relative to it. Keep in lockstep with
 *  `CAPABILITY_CACHE_TTL` in `agent_provider/cursor/capabilities.rs`. */
export const CURSOR_CAPABILITY_TTL_MS = 5 * 60 * 1000;

/** How often the app re-harvests Cursor's catalog. Deliberately LONGER
 *  than the Rust TTL: polling exactly at the TTL means each tick has a
 *  coin-flip chance of landing inside the still-valid cache window, so
 *  every other poll is a no-op and the effective refresh period silently
 *  doubles. A margin past the TTL makes every tick a real refresh. */
export const CURSOR_CAPABILITY_REFRESH_MS =
  CURSOR_CAPABILITY_TTL_MS + 60 * 1000;

/** Whether a scheduled Cursor re-harvest is worth issuing.
 *
 *  A machine without the CLI has no catalog to refresh, and the probe
 *  would try to spawn a missing binary once per tick forever. Only a
 *  definite "not installed" suppresses the poll — an unknown health slot
 *  (never probed) still refreshes, because the harvest is how the picker
 *  learns Cursor exists at all. */
export function shouldRefreshCursorCapabilities(
  health: ProviderHealthReport | null,
): boolean {
  return health?.installed !== false;
}

interface ProviderCapabilitiesStore {
  claude: ProviderChatCapabilities | null;
  codex: ProviderChatCapabilities | null;
  cursor: ProviderChatCapabilities | null;
  /** Step 12 Stage 3 — slot for OpenCode's live model harvest. Stays
   *  `null` until `refresh("opencode")` resolves. Failure surfaces in
   *  `opencodeError`; the slot itself stays `null` so the picker can
   *  render an empty state rather than a stale list. */
  opencode: ProviderChatCapabilities | null;
  /** Hermes' per-profile model catalogue, flattened across every
   *  configured profile — each row carries its own `profile`. Stays
   *  `null` until `refresh("hermes")` resolves. */
  hermes: ProviderChatCapabilities | null;
  claudeError: string | null;
  codexError: string | null;
  cursorError: string | null;
  opencodeError: string | null;
  hermesError: string | null;
  loaded: boolean;
  refresh: (provider: AgentChatProviderKind) => Promise<void>;
  refreshAll: () => Promise<void>;
}

export const useProviderCapabilities = create<ProviderCapabilitiesStore>(
  (set) => ({
    claude: null,
    codex: null,
    cursor: null,
    opencode: null,
    hermes: null,
    claudeError: null,
    codexError: null,
    cursorError: null,
    opencodeError: null,
    hermesError: null,
    loaded: false,
    refresh: async (provider) => {
      try {
        const caps = await listChatProviderCapabilities(provider);
        set((state) => storeOk(state, provider, caps));
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        console.error(
          `[provider-capabilities] refresh(${provider}) failed:`,
          err,
        );
        set((state) => storeErr(state, provider, message));
      }
    },
    refreshAll: async () => {
      const store = useProviderCapabilities.getState();
      // Promise.all is fine here — each `refresh` awaits its own
      // try/catch so a single provider's failure (commonly OpenCode
      // not installed) doesn't reject the whole barrier.
      await Promise.all([
        store.refresh("claude"),
        store.refresh("codex"),
        store.refresh("cursor"),
        store.refresh("opencode"),
        store.refresh("hermes"),
      ]);
      set({ loaded: true });
    },
  }),
);

/**
 * Fetch chat-side provider capabilities once on mount. Mount exactly
 * once (e.g. in `App.tsx`). Subsequent refreshes happen implicitly when
 * the Rust side emits `provider_capabilities_updated` (live harvest
 * path, deferred — MVP ships fallback-only for Claude/Codex; OpenCode
 * already does a live harvest at refresh time, see Stage 3).
 *
 * Originally gated on `enableAgentChat` so the harvest only ran for
 * Beta users. That gate was lifted because the same picker is now
 * reused by the merge-resolver settings panel — a non-Beta consumer
 * that legitimately needs the model list. Per-provider failures
 * (`codex_not_installed`, `opencode_not_installed`) still surface
 * correctly via the per-provider error slot, so users who don't have
 * those CLIs installed see "Not installed" tooltips instead of broken
 * states. The capability call is read-only from the user's perspective:
 * Claude returns its static fallback, Codex/OpenCode harvest only if
 * their binaries are present on PATH.
 *
 * `enableAgentChat` is kept as a parameter for any future caller that
 * wants to opt out, but the default behavior is unconditional refresh.
 */
export function useProviderCapabilitiesInit(): void {
  const refreshAll = useProviderCapabilities((s) => s.refreshAll);
  const refresh = useProviderCapabilities((s) => s.refresh);
  useEffect(() => {
    let timer: number | null = null;
    let cancelled = false;
    void refreshAll().finally(() => {
      if (cancelled) return;
      timer = window.setInterval(() => {
        const health = useProviderHealth.getState().slots.cursor.report;
        if (!shouldRefreshCursorCapabilities(health)) return;
        void refresh("cursor");
      }, CURSOR_CAPABILITY_REFRESH_MS);
    });
    // Cursor's installed CLI is the catalog authority. Refresh on a bounded
    // cadence so newly released/retired models appear without a Codemux
    // release or app restart. Start the clock after the initial harvest so
    // the matching Rust TTL has definitely elapsed on the first tick.
    return () => {
      cancelled = true;
      if (timer !== null) window.clearInterval(timer);
    };
  }, [refreshAll, refresh]);
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
    case "opencode":
      return state.opencode;
    case "hermes":
      return state.hermes;
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
    case "opencode":
      return state.opencodeError;
    case "hermes":
      return state.hermesError;
  }
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
    case "opencode":
      return { opencode: caps, opencodeError: null };
    case "hermes":
      return { hermes: caps, hermesError: null };
  }
  // Exhaustive switch above; the void return is unreachable but keeps
  // TS happy when AgentChatProviderKind grows. It also DEFEATS the
  // exhaustiveness check: a provider with no arm here compiles clean
  // and silently writes the whole state back, leaving that rail empty
  // forever with no error. Add the arm by hand — `npm run check` will
  // not catch it.
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
    case "opencode":
      return { opencodeError: message };
    case "hermes":
      return { hermesError: message };
  }
  return state;
}
