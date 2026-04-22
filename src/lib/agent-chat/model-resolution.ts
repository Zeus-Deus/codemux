import type { ChatModelInfo } from "@/tauri/types";

/**
 * Resolvers that mirror T3Code's `shared/src/model.ts` — keep a raw
 * effort / context-window value only when the active model supports it,
 * otherwise fall back to the model default.
 *
 * These are the primitives that enforce the compatibility rule: the UI
 * must never let a user keep an option the active model doesn't
 * support.
 */

/** True iff `value` is one of the model's natively-supported effort levels. */
export function hasEffortLevel(
  model: ChatModelInfo | null | undefined,
  value: string,
): boolean {
  return !!model?.effort_levels.includes(value);
}

/** True iff `value` is a prompt-injected level (e.g. `"ultrathink"`). */
export function isPromptInjectedEffort(
  model: ChatModelInfo | null | undefined,
  value: string,
): boolean {
  return !!model?.prompt_injected_effort_levels.includes(value);
}

/**
 * Resolve a raw effort value against the model.
 *
 * - If the value is natively supported, return it.
 * - If the value is a prompt-injected level (ultrathink), return it
 *   unchanged — the caller is responsible for NOT sending it to the
 *   SDK/RPC effort param (see `effortForApi`).
 * - Otherwise fall back to the model default. `null` if the model has
 *   no effort levels at all (Haiku).
 */
export function resolveEffort(
  model: ChatModelInfo | null | undefined,
  raw: string | null | undefined,
): string | null {
  if (!model) return null;
  const trimmed = typeof raw === "string" ? raw.trim() : "";
  if (trimmed) {
    if (model.effort_levels.includes(trimmed)) return trimmed;
    if (model.prompt_injected_effort_levels.includes(trimmed)) return trimmed;
  }
  return model.default_effort ?? null;
}

/**
 * Strip prompt-injected values before handing the effort to the
 * backend's SDK/RPC effort param. `ultrathink` never reaches the SDK —
 * it rides in the prompt text instead.
 */
export function effortForApi(
  model: ChatModelInfo | null | undefined,
  effort: string | null | undefined,
): string | null {
  if (!model || !effort) return null;
  if (model.prompt_injected_effort_levels.includes(effort)) {
    return model.default_effort ?? null;
  }
  return effort;
}

/** True iff `value` is one of the model's context-window options. */
export function hasContextWindowOption(
  model: ChatModelInfo | null | undefined,
  value: string,
): boolean {
  return !!model?.context_window_options.some((o) => o.value === value);
}

/** Default context-window value for the model, or `null` if none. */
export function getDefaultContextWindow(
  model: ChatModelInfo | null | undefined,
): string | null {
  return model?.context_window_options.find((o) => o.is_default)?.value ?? null;
}

/**
 * Resolve a raw context-window value. If missing or unsupported, fall
 * back to the model default. `null` when the model doesn't expose a
 * context-window picker.
 */
export function resolveContextWindow(
  model: ChatModelInfo | null | undefined,
  raw: string | null | undefined,
): string | null {
  if (!model) return null;
  const dflt = getDefaultContextWindow(model);
  if (!raw) return dflt;
  return hasContextWindowOption(model, raw) ? raw : dflt;
}

/**
 * Apply T3Code's `resolveClaudeApiModelId` trick client-side: when the
 * user picked `"1m"`, Anthropic expects the model id to carry the
 * `[1m]` bracket suffix. Used by the frontend when constructing
 * `StartSessionInput` payloads for Claude — the Rust adapter also
 * applies the same mutation as a defense in depth.
 */
export function resolveClaudeApiModelId(
  modelId: string,
  contextWindow: string | null | undefined,
): string {
  if (contextWindow === "1m") return `${modelId}[1m]`;
  return modelId;
}
