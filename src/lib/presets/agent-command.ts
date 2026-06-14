/**
 * Structured preset command assembly.
 *
 * Turns a {@link PresetAgentConfig} (agent + optional model + optional
 * reasoning + prompt) into the shell command string stored in
 * `TerminalPreset.commands` and launched by the existing `apply_preset`
 * pipeline. The metadata that governs assembly — the binary, autonomy flag,
 * model flag, and reasoning mechanism — comes from the {@link
 * AgentCatalogEntry} served by the Rust `list_agent_catalog` command.
 *
 * This is the single source of truth for assembly (the editor calls it for
 * the live preview and to compute the `commands` it persists), so the Rust
 * side never re-derives the command — it just stores what the editor sends.
 *
 * Quoting is intentionally minimal and biased toward natural-language
 * prompts: the prompt is wrapped in double quotes with `"` and `\` escaped,
 * which leaves apostrophes (common in English) untouched. Agents or prompts
 * that need exotic shell quoting can switch the preset to raw mode.
 */
import type { AgentCatalogEntry, PresetAgentConfig } from "@/tauri/types";

/** The "no reasoning / default" sentinel — the first catalog option. */
export const NO_REASONING = "";

/** Wrap a string as a double-quoted shell argument. */
function quoteArg(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Quote a model value only if it contains whitespace. */
function maybeQuoteModel(value: string): string {
  return /\s/.test(value) ? quoteArg(value) : value;
}

/** Find a catalog entry by agent id. */
export function findAgentEntry(
  catalog: AgentCatalogEntry[],
  agentId: string | null | undefined,
): AgentCatalogEntry | null {
  if (!agentId) return null;
  return catalog.find((e) => e.id === agentId) ?? null;
}

/** A fresh structured config for a newly-selected agent. */
export function defaultAgentConfig(entry: AgentCatalogEntry): PresetAgentConfig {
  return {
    agent_id: entry.id,
    model: null,
    reasoning: null,
    prompt: "",
    // Default autonomy on when the agent has a skip-permissions flag —
    // matches the built-in presets, which all launch in full-auto.
    skip_permissions: entry.autonomy_flag != null,
  };
}

/**
 * Assemble the shell command for a structured preset.
 *
 * Order: `<binary> [autonomy] [--model X] [reasoning-flag] ["<prompt>"]`.
 * The Codemux agent-context injection (`inject_agent_context`) runs on top
 * of this at launch, appending the workspace system-prompt flag after the
 * positional prompt — both coexist cleanly.
 */
export function assembleAgentCommand(
  entry: AgentCatalogEntry,
  config: PresetAgentConfig,
): string {
  const parts: string[] = [entry.binary];

  if (config.skip_permissions && entry.autonomy_flag) {
    parts.push(entry.autonomy_flag);
  }

  const model = config.model?.trim();
  if (entry.accepts_model && model) {
    parts.push(entry.model_flag, maybeQuoteModel(model));
  }

  // Reasoning: either a real CLI flag, or a prompt prefix that's always
  // safe to inject. The empty value is the "Default / none" option.
  let promptPrefix = "";
  if (entry.reasoning && config.reasoning) {
    const option = entry.reasoning.options.find(
      (o) => o.value === config.reasoning,
    );
    if (option && option.value !== NO_REASONING) {
      if (entry.reasoning.flag_template) {
        parts.push(entry.reasoning.flag_template.replace("{value}", option.value));
      } else if (option.prompt_prefix) {
        promptPrefix = option.prompt_prefix;
      }
    }
  }

  const promptText = `${promptPrefix}${config.prompt ?? ""}`.trim();
  if (entry.supports_prompt && promptText) {
    parts.push(quoteArg(promptText));
  }

  return parts.join(" ");
}
