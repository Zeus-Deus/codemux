import type { ChatMode } from "@/stores/agent-chat-store";
import type { AgentChatProviderKind } from "@/tauri/types";

/** Whether Codemux can control a composer mode for this provider. */
export function isChatModeSupported(
  provider: AgentChatProviderKind,
  mode: ChatMode,
): boolean {
  // Grok owns its ask/agent permissions inside ACP. They are not the same
  // thing as Codemux's Plan/Ask pills, which impose Claude-style plan mode
  // and prompt wrappers. Debug remains provider-agnostic.
  return provider !== "grok" || (mode !== "plan" && mode !== "ask");
}

/** Heal persisted or externally-supplied modes before rendering/launch. */
export function normalizeChatModeForProvider(
  provider: AgentChatProviderKind,
  mode: ChatMode,
): ChatMode {
  return isChatModeSupported(provider, mode) ? mode : "default";
}
