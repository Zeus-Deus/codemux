import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Launch-time model-selection helpers for the New Workspace dialog.
 *
 * The dialog launches a CLI agent preset. This module decides which
 * agent family a preset belongs to and where its model list comes from,
 * so the model pill lights up automatically for any preset that runs an
 * already-modeled CLI. Mirrors the Rust `agent_capability` module —
 * detection here only drives the UI; the Rust side owns flag injection.
 */

/** CLI agent families the launch dialog can offer model selection for.
 *  Mirrors the Rust `agent_capability::AgentFamily`. */
export type LaunchFamily = "claude" | "codex" | "opencode" | "gemini";

/**
 * Detect the agent family a preset command launches, or `null` when the
 * binary is not one Codemux models. Mirrors Rust `detect_family`: skips
 * leading `VAR=val` env assignments and strips any directory path, so
 * `/usr/bin/claude`, `FOO=bar claude`, and `claude` all resolve alike.
 */
export function detectLaunchFamily(
  command: string | undefined | null,
): LaunchFamily | null {
  if (!command) return null;
  for (const tok of command.trim().split(/\s+/)) {
    if (!tok) continue;
    // Leading env assignment (`KEY=val`) — keep scanning.
    if (!tok.startsWith("-") && tok.includes("=")) continue;
    const base = tok.split(/[/\\]/).pop() ?? tok;
    switch (base) {
      case "claude":
        return "claude";
      case "codex":
        return "codex";
      case "opencode":
        return "opencode";
      case "gemini":
        return "gemini";
      default:
        return null;
    }
  }
  return null;
}

/**
 * Map a launch family to its chat-capability provider kind. Gemini has
 * no chat driver, so it returns `null` and the dialog falls back to the
 * maintained [`GEMINI_MODELS`] list instead of the capability store.
 */
export function familyToProviderKind(
  family: LaunchFamily,
): AgentChatProviderKind | null {
  switch (family) {
    case "claude":
      return "claude";
    case "codex":
      return "codex";
    case "opencode":
      return "opencode";
    case "gemini":
      return null;
  }
}

export interface ReasoningOption {
  value: string;
  label: string;
}

/**
 * Families whose CLI accepts a reasoning flag — `claude --effort` and
 * `codex -c model_reasoning_effort`. OpenCode's TUI and the Gemini CLI
 * expose none, so the picker hides the reasoning row for them. The
 * reasoning *values* are not listed here — they come dynamically from
 * each model's `effort_levels` in the live capability bundle. This set
 * is only the structural fact of which CLI has the flag at all (it
 * mirrors the family arms of the Rust `apply_model_selection`).
 */
export const REASONING_FLAG_FAMILIES: ReadonlySet<LaunchFamily> = new Set([
  "claude",
  "codex",
]);

export interface LaunchModel {
  id: string;
  label: string;
  /** Upstream provider id for federated families (OpenCode). */
  subProvider?: string | null;
}

/**
 * Maintained model list for Gemini — the one family with no chat
 * capability harvest. Small and hand-updated; Gemini ships a handful of
 * models that change rarely. Tracked as the Gemini arm of the launch
 * model picker (see `docs/plans/model-selection-before-launch.md`).
 */
export const GEMINI_MODELS: LaunchModel[] = [
  { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
  { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash" },
];

/**
 * Extract a model id baked into a preset command (`--model X`,
 * `--model=X`, or `-m X`). Lets the picker pre-select a preset that
 * already hard-codes a model — and the Rust side won't double-inject.
 */
export function parseBakedModel(
  command: string | undefined | null,
): string | null {
  if (!command) return null;
  const tokens = command.trim().split(/\s+/);
  for (let i = 0; i < tokens.length; i++) {
    const tok = tokens[i];
    if (tok === "--model" || tok === "-m") {
      return tokens[i + 1] ?? null;
    }
    if (tok.startsWith("--model=")) {
      return tok.slice("--model=".length) || null;
    }
  }
  return null;
}

/** Number of models above which the picker shows a search input. Short
 *  lists (Claude, Codex, Gemini) stay glanceable; OpenCode's federated
 *  list crosses this immediately. */
export const MODEL_SEARCH_THRESHOLD = 10;
