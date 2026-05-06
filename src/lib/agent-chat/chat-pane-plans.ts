/**
 * Pure planning functions extracted from `AgentChatPane`.
 *
 * Each `plan*` function takes a snapshot of the state that drives a
 * decision and returns a data-only description of what should happen.
 * The component interprets the plan by calling store actions / Tauri
 * commands. Separating the decision from the effect makes the logic
 * unit-testable without mounting the component or mocking a stack of
 * hooks.
 */

import type {
  AgentChatProviderKind,
  ChatModelInfo,
  EffortGranularity,
  ProviderChatCapabilities,
} from "@/tauri/types";

import {
  resolveContextWindow,
  resolveEffort,
} from "./model-resolution";
import {
  applyClaudePromptEffortPrefix,
  ULTRATHINK_PROMPT_PREFIX,
} from "./ultrathink";

// ────────────────────────────────────────────────────────────────────
// planEffortChange — user clicked a new effort in the picker
// ────────────────────────────────────────────────────────────────────

/**
 * Recipe for what should happen when the user picks a new effort.
 *
 * - `updateDraft`: the composer draft needs to change. `prepend`
 *   re-prepends the Ultrathink prefix (idempotent); `strip` removes
 *   a leading prefix; `null` leaves the draft alone.
 * - `setEffort`: the new effort value to write to the thread slice.
 *   `null` means "do not write effort" — used for the ultrathink
 *   branch where the prompt itself carries the signal.
 * - `restart`: true when the provider is per-session (Claude) and the
 *   effort change therefore requires a silent session restart. Always
 *   false for per-turn providers (Codex).
 */
export interface EffortChangePlan {
  updateDraft:
    | { kind: "prepend"; nextDraft: string }
    | { kind: "strip"; nextDraft: string }
    | null;
  setEffort: string | null;
  restart: boolean;
}

export interface PlanEffortChangeInput {
  nextEffort: string;
  model: ChatModelInfo | null;
  currentDraft: string;
  provider: AgentChatProviderKind;
}

/** Case-insensitive check for the canonical ultrathink prefix. */
function hasLeadingUltrathinkPrefix(text: string): boolean {
  return /^Ultrathink:\s*/i.test(text);
}

export function planEffortChange(
  input: PlanEffortChangeInput,
): EffortChangePlan | null {
  const { nextEffort, model, currentDraft, provider } = input;
  if (!model) return null;

  // Prompt-injected branch — write the prefix into the draft and do
  // NOT store the effort. The prompt is the signal.
  if (model.prompt_injected_effort_levels.includes(nextEffort)) {
    const prepended = applyClaudePromptEffortPrefix(
      currentDraft,
      "ultrathink",
    );
    // `applyClaudePromptEffortPrefix` returns empty for an empty
    // input. For an empty composer we want to seed the prefix so the
    // user sees immediate feedback.
    const nextDraft = prepended.length > 0 ? prepended : ULTRATHINK_PROMPT_PREFIX;
    return {
      updateDraft: { kind: "prepend", nextDraft },
      setEffort: null,
      restart: false,
    };
  }

  // Switching OUT of ultrathink while the draft still carries the
  // prefix — strip it so state and prompt stay consistent.
  const stripIfPresent = hasLeadingUltrathinkPrefix(currentDraft)
    ? currentDraft.replace(/^Ultrathink:\s*/i, "")
    : null;

  return {
    updateDraft:
      stripIfPresent !== null
        ? { kind: "strip", nextDraft: stripIfPresent }
        : null,
    setEffort: nextEffort,
    restart: provider === "claude",
  };
}

// ────────────────────────────────────────────────────────────────────
// planModelChange — user picked a new model in the ModelPicker
// ────────────────────────────────────────────────────────────────────

/**
 * Compatibility-reset plan when the user switches models. `undefined`
 * means "do not touch the field"; `null` means "clear it"; a string
 * means "set it to this value". A field is only present when the
 * current value is invalid for the new model.
 */
export interface ModelChangePlan {
  resetEffort: string | null | undefined;
  resetContextWindow: string | null | undefined;
}

export interface PlanModelChangeInput {
  newModel: ChatModelInfo | null;
  currentEffort: string | null;
  currentContextWindow: string | null;
}

export function planModelChange(
  input: PlanModelChangeInput,
): ModelChangePlan {
  const { newModel, currentEffort, currentContextWindow } = input;
  if (!newModel) {
    return { resetEffort: undefined, resetContextWindow: undefined };
  }
  const resolvedEffort = resolveEffort(newModel, currentEffort);
  const resolvedCtx = resolveContextWindow(newModel, currentContextWindow);
  return {
    resetEffort:
      resolvedEffort !== currentEffort ? resolvedEffort : undefined,
    resetContextWindow:
      resolvedCtx !== currentContextWindow ? resolvedCtx : undefined,
  };
}

// ────────────────────────────────────────────────────────────────────
// planSubmit — user pressed Send
// ────────────────────────────────────────────────────────────────────

/**
 * Transformed payload for a send-turn call.
 *
 * - `text`: the user's draft, possibly with the ultrathink prefix
 *   re-applied as a belt-and-braces defense for Claude.
 * - `effortOverride`: the per-turn effort to pass to Codex's RPC.
 *   `null` for Claude (which applies effort at session init).
 */
export interface SubmitPlan {
  text: string;
  effortOverride: string | null;
}

export interface PlanSubmitInput {
  rawText: string;
  provider: AgentChatProviderKind;
  effort: string | null;
}

export function planSubmit(input: PlanSubmitInput): SubmitPlan {
  const { rawText, provider, effort } = input;
  const text =
    provider === "claude" && effort === "ultrathink"
      ? applyClaudePromptEffortPrefix(rawText, "ultrathink")
      : rawText;
  return {
    text,
    effortOverride: provider === "codex" ? effort : null,
  };
}

// ────────────────────────────────────────────────────────────────────
// planPermissionModeChange — user picked a new permission mode
// ────────────────────────────────────────────────────────────────────

/**
 * Recipe for a permission-mode change.
 *
 * - `setPermissionMode`: the new value to write to the thread slice.
 *   Never null — a picker selection always represents an intent.
 * - `restart`: true when the provider requires a session restart to
 *   apply the mode (PerSession granularity). false means the mode
 *   rides on the next send via `permission_mode_override`.
 */
export interface PermissionModeChangePlan {
  setPermissionMode: string;
  restart: boolean;
}

export interface PlanPermissionModeChangeInput {
  nextMode: string;
  capabilities: ProviderChatCapabilities | null;
}

export function planPermissionModeChange(
  input: PlanPermissionModeChangeInput,
): PermissionModeChangePlan | null {
  const { nextMode, capabilities } = input;
  if (!capabilities) return null;
  const supported = capabilities.permission_modes.some(
    (m) => m.value === nextMode,
  );
  if (!supported) return null;
  return {
    setPermissionMode: nextMode,
    restart: capabilities.permission_granularity === "per_session",
  };
}

// ────────────────────────────────────────────────────────────────────
// planProviderOrCapabilityChange — reset orphaned permission mode
// ────────────────────────────────────────────────────────────────────

/**
 * Compatibility-reset plan when the provider changes (or its
 * capabilities snapshot arrives late). If the thread's current
 * `permissionMode` isn't in the new provider's `permission_modes`,
 * reset to the provider's default; otherwise leave alone.
 *
 * `undefined` means "do not touch"; a string means "write this".
 */
export interface CapabilityCompatResetPlan {
  resetPermissionMode: string | null | undefined;
}

export function planCapabilityCompatReset(input: {
  capabilities: ProviderChatCapabilities | null;
  currentPermissionMode: string | null;
}): CapabilityCompatResetPlan {
  const { capabilities, currentPermissionMode } = input;
  if (!capabilities) return { resetPermissionMode: undefined };
  const current = currentPermissionMode ?? null;
  if (!current) {
    // Seed with the provider default if we have one, else leave
    // unset.
    if (capabilities.default_permission_mode) {
      return { resetPermissionMode: capabilities.default_permission_mode };
    }
    return { resetPermissionMode: undefined };
  }
  const stillValid = capabilities.permission_modes.some(
    (m) => m.value === current,
  );
  if (stillValid) return { resetPermissionMode: undefined };
  return {
    resetPermissionMode: capabilities.default_permission_mode ?? null,
  };
}

// Re-export types used by callers from the same module surface.
export type { EffortGranularity };
