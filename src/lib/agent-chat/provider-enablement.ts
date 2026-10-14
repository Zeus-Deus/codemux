import { useMemo } from "react";
import { useSettingsStore } from "@/stores/settings-store";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Providers switched off in Settings → Providers, as a comma list. Stored
 * on this machine only: whether an agent is installed is a property of the
 * machine, so a provider turned off here must not disappear elsewhere.
 *
 * Off means "don't offer it": the model picker, the Home agent strip and
 * the new-chat default skip it. A chat that already runs on it keeps
 * working, and the picker keeps showing the provider a chat is using.
 */
export const DISABLED_PROVIDERS_SETTING = "providers.disabled";

const KNOWN: ReadonlySet<AgentChatProviderKind> = new Set<AgentChatProviderKind>([
  "claude", "codex", "cursor", "grok", "hermes", "acp", "opencode",
]);

export function parseDisabledProviders(raw: string | undefined): ReadonlySet<AgentChatProviderKind> {
  const out = new Set<AgentChatProviderKind>();
  for (const part of (raw ?? "").split(",")) {
    const id = part.trim() as AgentChatProviderKind;
    if (KNOWN.has(id)) out.add(id);
  }
  return out;
}

export function disabledProviders(): ReadonlySet<AgentChatProviderKind> {
  return parseDisabledProviders(useSettingsStore.getState().settings[DISABLED_PROVIDERS_SETTING]);
}

export function isProviderEnabled(provider: AgentChatProviderKind): boolean {
  return !disabledProviders().has(provider);
}

/** Providers a brand-new chat can start on. At least one of them stays on,
 *  so there is always an enabled default to fall back to. Kept in step with
 *  `NEW_CHAT_DEFAULT_PROVIDERS`, which imports from here. */
export const DEFAULT_CAPABLE_PROVIDERS: readonly AgentChatProviderKind[] = [
  "claude", "codex", "cursor", "grok", "opencode",
];

/** Whether turning `provider` off would leave no provider a new chat can
 *  start on. */
export function isLastDefaultProvider(
  provider: AgentChatProviderKind,
  disabled: ReadonlySet<AgentChatProviderKind> = disabledProviders(),
): boolean {
  return DEFAULT_CAPABLE_PROVIDERS.includes(provider) &&
    DEFAULT_CAPABLE_PROVIDERS.every((p) => p === provider || disabled.has(p));
}

/** Returns false, changing nothing, when turning `provider` off would
 *  leave no provider for new chats. */
export function setProviderEnabled(provider: AgentChatProviderKind, enabled: boolean): boolean {
  const next = new Set(disabledProviders());
  if (!enabled && isLastDefaultProvider(provider, next)) return false;
  if (enabled) next.delete(provider);
  else next.add(provider);
  useSettingsStore.getState().set(DISABLED_PROVIDERS_SETTING, [...next].sort().join(","));
  return true;
}

export function useDisabledProviders(): ReadonlySet<AgentChatProviderKind> {
  const raw = useSettingsStore((s) => s.settings[DISABLED_PROVIDERS_SETTING]);
  return useMemo(() => parseDisabledProviders(raw), [raw]);
}
