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

export function setProviderEnabled(provider: AgentChatProviderKind, enabled: boolean): void {
  const next = new Set(disabledProviders());
  if (enabled) next.delete(provider);
  else next.add(provider);
  useSettingsStore.getState().set(DISABLED_PROVIDERS_SETTING, [...next].sort().join(","));
}

export function useDisabledProviders(): ReadonlySet<AgentChatProviderKind> {
  const raw = useSettingsStore((s) => s.settings[DISABLED_PROVIDERS_SETTING]);
  return useMemo(() => parseDisabledProviders(raw), [raw]);
}
