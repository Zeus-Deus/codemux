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

/** Grok's model and reasoning catalogue is owned by the installed CLI and
 *  can change independently of Codemux. Re-discover it on the same bounded
 *  cadence as Cursor's live ACP catalogue. */
export const GROK_CAPABILITY_REFRESH_MS = CURSOR_CAPABILITY_REFRESH_MS;

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

/** Same installed-binary guard for Grok's scheduled live discovery. */
export function shouldRefreshGrokCapabilities(
  health: ProviderHealthReport | null,
): boolean {
  return health?.installed !== false;
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
  loaded: boolean;
  refresh: (provider: AgentChatProviderKind) => Promise<void>;
  refreshAll: () => Promise<void>;
}

export const useProviderCapabilities = create<ProviderCapabilitiesStore>(
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
        store.refresh("grok"),
        store.refresh("opencode"),
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
    let cursorTimer: number | null = null;
    let grokTimer: number | null = null;
    let cancelled = false;
    void refreshAll().finally(() => {
      if (cancelled) return;
      cursorTimer = window.setInterval(() => {
        const health = useProviderHealth.getState().slots.cursor.report;
        if (!shouldRefreshCursorCapabilities(health)) return;
        void refresh("cursor");
      }, CURSOR_CAPABILITY_REFRESH_MS);
      grokTimer = window.setInterval(() => {
        const health = useProviderHealth.getState().slots.grok.report;
        if (!shouldRefreshGrokCapabilities(health)) return;
        void refresh("grok");
      }, GROK_CAPABILITY_REFRESH_MS);
    });
    // The installed Cursor and Grok CLIs are their catalog authorities.
    // Refresh on a bounded cadence so newly released/retired models and
    // reasoning options appear without a Codemux release or app restart.
    // Start the clocks after the initial harvest.
    return () => {
      cancelled = true;
      if (cursorTimer !== null) window.clearInterval(cursorTimer);
      if (grokTimer !== null) window.clearInterval(grokTimer);
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
