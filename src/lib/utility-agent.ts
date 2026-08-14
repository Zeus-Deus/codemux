import {
  selectCapabilities,
  selectError,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import type { UtilityModelSelection } from "@/tauri/commands";

export const UTILITY_SETTING_KEYS = {
  mode: "ai.utility.mode",
  provider: "ai.utility.provider",
  model: "ai.utility.model",
  effort: "ai.utility.effort",
} as const;

export type UtilityAgentMode = "auto" | "custom";

function preferredModel(
  provider: AgentChatProviderKind,
): UtilityModelSelection | null {
  const state = useProviderCapabilities.getState();
  const caps = selectCapabilities(state, provider);
  if (!caps || selectError(state, provider)) return null;

  if (provider === "codex") {
    const model =
      caps.models.find((entry) => entry.id === "gpt-5.6-luna") ??
      caps.models.find((entry) => /luna/i.test(entry.id));
    return model
      ? {
          provider,
          model: model.id,
          effort: model.effort_levels.includes("low")
            ? "low"
            : model.default_effort,
        }
      : null;
  }
  if (provider === "claude") {
    const model =
      caps.models.find((entry) => entry.id === "claude-haiku-4-5") ??
      caps.models.find((entry) => /haiku/i.test(entry.id));
    return model ? { provider, model: model.id, effort: null } : null;
  }

  // Federated OpenCode catalogues can be enormous and pricing varies by the
  // user's upstream account. Auto only chooses a model explicitly advertised
  // as free; otherwise Settings asks the user for a deliberate selection.
  const model = caps.models.find((entry) => entry.is_free);
  return model
    ? { provider, model: model.id, effort: model.default_effort }
    : null;
}

export function resolveAutoUtilitySelection(): UtilityModelSelection | null {
  return (
    preferredModel("codex") ??
    preferredModel("claude") ??
    preferredModel("opencode")
  );
}

export function utilitySelectionFromStores(): UtilityModelSelection | null {
  const settings = useSettingsStore.getState();
  const mode = settings.get(UTILITY_SETTING_KEYS.mode) as UtilityAgentMode;
  if (mode !== "custom") return resolveAutoUtilitySelection();
  const provider = settings.get(
    UTILITY_SETTING_KEYS.provider,
  ) as AgentChatProviderKind;
  const model = settings.get(UTILITY_SETTING_KEYS.model).trim();
  if (!model || !["claude", "codex", "opencode"].includes(provider))
    return null;
  const effort = settings.get(UTILITY_SETTING_KEYS.effort).trim() || null;
  return { provider, model, effort };
}
