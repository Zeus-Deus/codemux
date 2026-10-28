import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useChatGptStore } from "@/stores/chatgpt-store";
import {
  providerReadiness,
  useProviderHealth,
  type ProviderReadiness,
} from "@/stores/provider-health-store";
import { defaultAccountSignedIn, useProviderAccounts } from "@/stores/provider-accounts-store";
import type { ProviderAccount } from "@/tauri/commands";
import type { AgentChatProviderKind, ProviderHealthReport } from "@/tauri/types";
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

/** Order an automatic new chat tries providers in, taking the first whose
 *  last health probe on this machine said it is ready. */
export const AUTOMATIC_PROVIDER_ORDER: ReadonlyArray<AgentChatProviderKind> = [
  "claude",
  "codex",
  "opencode",
  "cursor",
  "grok",
];

/** Whether a probe found the CLI installed but signed out of its own
 *  login: the one answer a signed-in extra account overrides. */
export function cliSignedOut(report: ProviderHealthReport): boolean {
  return providerReadiness(report) === "not_ready" && !!report.login_command;
}

/** `providerReadiness`, counting the account new chats on `provider` start
 *  on: a CLI signed out of its own login still runs a chat on a signed-in
 *  extra account set as the default. Until the account list loads
 *  (`accounts` null) the probe's answer stands. */
export function newChatReadiness(
  provider: AgentChatProviderKind,
  report: ProviderHealthReport,
  accounts: ProviderAccount[] | null,
  defaults: Record<string, string>,
): ProviderReadiness {
  if (!accounts || !cliSignedOut(report)) return providerReadiness(report);
  const signedIn = defaultAccountSignedIn(accounts, defaults, provider);
  return signedIn === true ? "ready" : signedIn === null ? "unverified" : "not_ready";
}

/** The first provider in `AUTOMATIC_PROVIDER_ORDER` that is switched on and
 *  whose cached health report is ready (or that starts on a signed-in extra
 *  account, `newChatReadiness`); failing that, the first installed one
 *  whose probe couldn't tell (readiness "unverified", e.g. Claude when its
 *  sign-in can't be checked). Null when none is known to be usable. Reads
 *  the caches only: choosing a default is not reason to probe. */
export function firstUsableProvider(
  off: ReadonlySet<AgentChatProviderKind> = disabledProviders(),
): AgentChatProviderKind | null {
  const slots = useProviderHealth.getState().slots;
  const { accounts, defaults } = useProviderAccounts.getState();
  const readiness = (provider: AgentChatProviderKind) => {
    const slot = slots[provider];
    if (off.has(provider) || slot.lastProbeFailed || !slot.report) return null;
    return newChatReadiness(provider, slot.report, accounts, defaults);
  };
  return AUTOMATIC_PROVIDER_ORDER.find((p) => readiness(p) === "ready") ??
    AUTOMATIC_PROVIDER_ORDER.find((p) => readiness(p) === "unverified") ??
    null;
}

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
 * otherwise the first agent this machine's health probes found ready
 * (`firstUsableProvider`), otherwise Claude, on that provider's catalog
 * default (`automaticModelId`). A saved model the provider no longer
 * offers falls back to that provider's default model, and a saved effort is only kept while the
 * model still lists it. Before capabilities load, the saved ids are
 * trusted as-is, since they came from a capability list in the first place.
 */
export function resolveNewChatModelDefaults(): NewChatModelDefaults {
  const settings = useSettingsStore.getState().settings;
  const provider = savedNewChatProvider();
  const off = disabledProviders();
  if (!provider) {
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
    const fallback = firstUsableProvider(off) ?? (off.has(AUTOMATIC_NEW_CHAT_PROVIDER)
      ? NEW_CHAT_DEFAULT_PROVIDERS.find((p) => !off.has(p)) ?? AUTOMATIC_NEW_CHAT_PROVIDER
      : AUTOMATIC_NEW_CHAT_PROVIDER);
    return {
      provider: fallback,
      model: automaticModelId(fallback),
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

/** The model an automatically picked provider starts on: the head of its
 *  harvested catalog, else the provider's own "default" alias where it has
 *  one, else null, which starts the session on the agent's own default.
 *  Never a hardcoded concrete id: that model may need credentials the user
 *  doesn't have (OpenCode routes `anthropic/…` through Anthropic). The
 *  draft re-resolves to the catalog once it is harvested
 *  (`reseedBootstrapDrafts`). */
function automaticModelId(provider: AgentChatProviderKind): string | null {
  const harvested = selectCapabilities(useProviderCapabilities.getState(), provider)?.models[0]?.id;
  if (harvested) return harvested;
  const fallback = defaultModelId(provider);
  return fallback === "default" ? fallback : null;
}

/** The provider saved in Settings → Agent → New chats, or null when new
 *  chats pick their own ("Automatic"). A provider turned off in Settings →
 *  Providers is never a new chat's default, so a saved one that is off, or
 *  unknown, counts as unset. */
function savedNewChatProvider(): AgentChatProviderKind | null {
  const provider = useSettingsStore.getState().settings[NEW_CHAT_SETTING_KEYS.provider] as
    | AgentChatProviderKind
    | undefined;
  if (!provider || !NEW_CHAT_DEFAULT_PROVIDERS.includes(provider)) return null;
  return disabledProviders().has(provider) ? null : provider;
}

/** Whether new chats pick their provider themselves ("Automatic"). */
export function isAutomaticNewChatProvider(): boolean {
  return savedNewChatProvider() === null;
}
