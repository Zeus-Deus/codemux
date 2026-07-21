import {
  selectCapabilities,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Fallback model id per provider when the capabilities store hasn't
 * hydrated yet (called at draft creation on the first tick of App
 * mount, before the async `listChatProviderCapabilities` round-trip
 * resolves). Kept as a tiny table so the rest of the codebase never
 * reaches for a hardcoded model id.
 *
 * Any entry here is superseded the moment `provider-capabilities-store`
 * reports real data; the fallback's only job is to make `defaultModelId`
 * synchronous and non-null.
 */
const FALLBACK_DEFAULT_MODEL_BY_PROVIDER: Record<AgentChatProviderKind, string> = {
  claude: "claude-opus-4-8",
  codex: "gpt-5.4",
  // Step 12 Stage 1 placeholder. OpenCode capabilities are harvested
  // live in Stage 2 and effectively zero out this fallback once the
  // store hydrates; the slug here follows OpenCode's own
  // `${providerId}/${modelId}` shape so any code path that does
  // happen to read the bare fallback during the brief
  // pre-hydration window still produces a recognisable identifier.
  opencode: "anthropic/claude-sonnet-4-6",
};

/**
 * Provider-native permission defaults used before the asynchronous
 * capabilities harvest completes. Permission-mode values are provider
 * protocol identifiers, not shared UI enums: Claude's Full access value is
 * `bypassPermissions`, while Codex expects `danger-full-access`.
 *
 * OpenCode intentionally stays null because it has no chat-side permission
 * picker. Once capabilities are available their advertised
 * `default_permission_mode` is authoritative over this bootstrap table.
 */
const FALLBACK_DEFAULT_PERMISSION_MODE_BY_PROVIDER: Record<
  AgentChatProviderKind,
  string | null
> = {
  claude: "bypassPermissions",
  codex: "danger-full-access",
  opencode: null,
};

/** Synchronous accessor for the provider's default model id.
 *
 * Reads `provider-capabilities-store.getState()` (safe outside React —
 * the store is a Zustand singleton). Returns the first model in the
 * capability payload, or the hardcoded fallback if capabilities aren't
 * hydrated yet. */
export function defaultModelId(provider: AgentChatProviderKind): string {
  const caps = selectCapabilities(
    useProviderCapabilities.getState(),
    provider,
  );
  return caps?.models[0]?.id ?? FALLBACK_DEFAULT_MODEL_BY_PROVIDER[provider];
}

/** Resolve the active provider's native default permission-mode value.
 *
 * This is the single frontend source for launch defaults. Callers must not
 * substitute a provider-agnostic value: an unknown mode is omitted by the
 * Codex adapter, which makes the app-server fall back to prompting even when
 * the picker visually says Full access.
 */
export function defaultPermissionModeForProvider(
  provider: AgentChatProviderKind,
): string | null {
  const caps = selectCapabilities(
    useProviderCapabilities.getState(),
    provider,
  );
  return (
    caps?.default_permission_mode ??
    FALLBACK_DEFAULT_PERMISSION_MODE_BY_PROVIDER[provider]
  );
}

/** List the provider's models as `{ id, label, description }` for
 *  picker rendering. `description` carries the backend's resolved
 *  version + blurb (e.g. Claude's
 *  "Opus 4.8 with 1M context · Best for everyday, complex tasks"),
 *  `null` when none was supplied. Returns `[]` when capabilities
 *  aren't hydrated — the picker renders its CommandEmpty state in
 *  that case. */
export function modelsForProvider(
  provider: AgentChatProviderKind,
): Array<{ id: string; label: string; description: string | null }> {
  const caps = selectCapabilities(
    useProviderCapabilities.getState(),
    provider,
  );
  return (
    caps?.models.map((m) => ({
      id: m.id,
      label: m.label,
      description: m.description,
    })) ?? []
  );
}

/** Look up a model's display label by id. Falls back to the raw id
 *  when the model is not in the capabilities payload (or caps haven't
 *  hydrated). */
export function modelLabel(
  provider: AgentChatProviderKind,
  modelId: string,
): string {
  const caps = selectCapabilities(
    useProviderCapabilities.getState(),
    provider,
  );
  return caps?.models.find((m) => m.id === modelId)?.label ?? modelId;
}

export interface CapabilityDefaults {
  model: string;
  effort: string | null;
  contextWindow: string | null;
  permissionMode: string | null;
}

/** Derive the full set of default session-config values for a given
 *  provider + model. Used by `makeDraft` in `chat-draft-store` so
 *  drafts start fully-configured — no null `effort` / `contextWindow`
 *  to trip up the pickers that gate on those fields.
 *
 *  Capability-driven: `effort` picks the model's `default_effort`,
 *  `contextWindow` picks the option flagged `is_default` (or the first
 *  option when none is flagged). `permissionMode` defaults to the
 *  provider's own native default (`bypassPermissions` for Claude,
 *  `danger-full-access` for Codex, and null for OpenCode).
 *
 *  Returns safe null-fallbacks when capabilities aren't hydrated — the
 *  slice setters accept null, so the draft still writes a valid shape.
 */
export function capabilityDefaults(
  provider: AgentChatProviderKind,
  modelId: string,
): CapabilityDefaults {
  const caps = selectCapabilities(
    useProviderCapabilities.getState(),
    provider,
  );
  const model = caps?.models.find((m) => m.id === modelId);
  const defaultContextWindow =
    model?.context_window_options.find((o) => o.is_default)?.value ??
    model?.context_window_options[0]?.value ??
    null;
  return {
    model: modelId,
    effort: model?.default_effort ?? null,
    contextWindow: defaultContextWindow,
    permissionMode: defaultPermissionModeForProvider(provider),
  };
}
