import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useChatGptStore } from "@/stores/chatgpt-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { defaultModelId } from "./capability-defaults";
import { DEFAULT_CAPABLE_PROVIDERS, disabledProviders } from "./provider-enablement";

/** The provider a new chat uses when no default is chosen. */
export const AUTOMATIC_NEW_CHAT_PROVIDER: AgentChatProviderKind = "claude";

/**
 * Providers offered for the new-chat default. Hermes is left out: its
 * models are scoped to a profile, and Settings → Agent already has a
 * "Default Hermes profile" row for that.
 */
export const NEW_CHAT_DEFAULT_PROVIDERS: ReadonlyArray<AgentChatProviderKind> =
  DEFAULT_CAPABLE_PROVIDERS;

export const NEW_CHAT_SETTING_KEYS = {
  provider: "chat.default_provider",
  model: "chat.default_model",
  effort: "chat.default_effort",
} as const;

export interface NewChatModelDefaults {
  provider: AgentChatProviderKind;
  model: string | null;
  /** null leaves the model's own default effort in place. */
  effort: string | null;
}

/**
 * The provider, model and effort a brand-new chat draft starts with, from
 * Settings → Agent → New chats.
 *
 * Unset ("Automatic") uses a connected ChatGPT plan's live Codex catalog,
 * otherwise Claude on its first model. A saved model the provider no longer
 * offers falls back to that provider's default model, and a saved effort is only kept while the
 * model still lists it. Before capabilities load, the saved ids are
 * trusted as-is, since they came from a capability list in the first place.
 */
export function resolveNewChatModelDefaults(): NewChatModelDefaults {
  const settings = useSettingsStore.getState().settings;
  const provider = settings[NEW_CHAT_SETTING_KEYS.provider] as
    | AgentChatProviderKind
    | undefined;
  // A provider turned off in Settings → Providers is never a new chat's
  // default, whether chosen explicitly or picked automatically.
  const off = disabledProviders();
  if (!provider || !NEW_CHAT_DEFAULT_PROVIDERS.includes(provider) || off.has(provider)) {
    if (useChatGptStore.getState().status?.phase === "connected" && !off.has("codex")) {
      return {
        provider: "codex",
        // Account changes clear this catalog; do not seed a CLI-account or
        // hardcoded model while the connected account's models are loading.
        model: useProviderCapabilities.getState().codex?.models[0]?.id ?? null,
        effort: null,
      };
    }
    // Settings keeps at least one of these on (`setProviderEnabled`), so the
    // final fallback is reachable only through a hand-edited setting.
    const fallback = off.has(AUTOMATIC_NEW_CHAT_PROVIDER)
      ? NEW_CHAT_DEFAULT_PROVIDERS.find((p) => !off.has(p)) ?? AUTOMATIC_NEW_CHAT_PROVIDER
      : AUTOMATIC_NEW_CHAT_PROVIDER;
    return {
      provider: fallback,
      model: defaultModelId(fallback),
      effort: null,
    };
  }

  const savedModel = settings[NEW_CHAT_SETTING_KEYS.model] || null;
  const savedEffort = settings[NEW_CHAT_SETTING_KEYS.effort] || null;
  const caps = selectCapabilities(useProviderCapabilities.getState(), provider);
  const connectedPlan = provider === "codex" &&
    useChatGptStore.getState().status?.phase === "connected";
  if (!caps) {
    return {
      provider,
      model: savedModel ?? (connectedPlan ? null : defaultModelId(provider)),
      effort: savedEffort,
    };
  }

  const model = selectModel(caps, savedModel);
  if (!model) {
    return { provider, model: connectedPlan ? null : defaultModelId(provider), effort: null };
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
