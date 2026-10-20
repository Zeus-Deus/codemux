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
  if (provider === "acp") return mode === "default";
  return provider !== "grok" || (mode !== "plan" && mode !== "ask");
}

/** Heal persisted or externally-supplied modes before rendering/launch. */
export function normalizeChatModeForProvider(
  provider: AgentChatProviderKind,
  mode: ChatMode,
): ChatMode {
  return isChatModeSupported(provider, mode) ? mode : "default";
}

/** Codex runs the Plan and Ask pills as its per-turn `plan` collaboration
 *  mode rather than a session permission mode, so each send names the mode
 *  its turn runs in: "plan" with the pill on, otherwise the thread's
 *  permission mode. Other providers apply their mode to the session. */
export function codexTurnPermissionOverride(
  provider: AgentChatProviderKind,
  mode: ChatMode,
  permissionMode: string | null,
): string | null {
  if (provider !== "codex") return null;
  return turnPermissionMode(mode, permissionMode);
}

/** The mode a turn started from this composer state runs in: "plan" while
 *  the Plan or Ask pill is on, otherwise the thread's permission mode. */
export function turnPermissionMode(
  mode: ChatMode,
  permissionMode: string | null,
): string | null {
  return mode === "plan" || mode === "ask" ? "plan" : permissionMode;
}
