import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { defaultModelId } from "./capability-defaults";

/** The provider a new chat uses when no default is chosen. */
export const AUTOMATIC_NEW_CHAT_PROVIDER: AgentChatProviderKind = "claude";

/**
 * Providers offered for the new-chat default. Hermes is left out: its
 * models are scoped to a profile, and Settings → Agent already has a
 * "Default Hermes profile" row for that.
 */
export const NEW_CHAT_DEFAULT_PROVIDERS: ReadonlyArray<AgentChatProviderKind> = [
  "claude",
  "codex",
  "cursor",
  "grok",
  "opencode",
];

export const NEW_CHAT_SETTING_KEYS = {
  provider: "chat.default_provider",
  model: "chat.default_model",
  effort: "chat.default_effort",
} as const;

export interface NewChatModelDefaults {
  provider: AgentChatProviderKind;
  model: string;
  /** null leaves the model's own default effort in place. */
  effort: string | null;
}

/**
 * The provider, model and effort a brand-new chat draft starts with, from
 * Settings → Agent → New chats.
 *
 * Unset ("Automatic") keeps the original behavior: Claude on its first
 * model. A saved model the provider no longer offers falls back to that
 * provider's default model, and a saved effort is only kept while the
 * model still lists it. Before capabilities load, the saved ids are
 * trusted as-is, since they came from a capability list in the first place.
 */
export function resolveNewChatModelDefaults(): NewChatModelDefaults {
  const settings = useSettingsStore.getState().settings;
  const provider = settings[NEW_CHAT_SETTING_KEYS.provider] as
    | AgentChatProviderKind
    | undefined;
  if (!provider || !NEW_CHAT_DEFAULT_PROVIDERS.includes(provider)) {
    return {
      provider: AUTOMATIC_NEW_CHAT_PROVIDER,
      model: defaultModelId(AUTOMATIC_NEW_CHAT_PROVIDER),
      effort: null,
    };
  }

  const savedModel = settings[NEW_CHAT_SETTING_KEYS.model] || null;
  const savedEffort = settings[NEW_CHAT_SETTING_KEYS.effort] || null;
  const caps = selectCapabilities(useProviderCapabilities.getState(), provider);
  if (!caps) {
    return {
      provider,
      model: savedModel ?? defaultModelId(provider),
      effort: savedEffort,
    };
  }

  const model = selectModel(caps, savedModel);
  if (!model) {
    return { provider, model: defaultModelId(provider), effort: null };
  }
  return {
    provider,
    model: model.id,
    effort:
      savedEffort && model.effort_levels.includes(savedEffort)
        ? savedEffort
        : null,
  };
}
