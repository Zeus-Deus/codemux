import { fallbackModelLabel } from "@/lib/agent-chat/capability-defaults";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { PROVIDER_LABELS } from "@/stores/update-all-store";
import type { AgentChatProviderKind } from "@/tauri/types";

/** What reached the new provider, as the switch divider says it. */
export function handoffLabel(handoffKind: string, truncated: boolean): string {
  switch (handoffKind) {
    case "summary":
      return "summary carried over";
    case "direct":
      return truncated ? "recent context carried over" : "context carried over";
    case "failed":
      return "context couldn't be carried over";
    default:
      return "nothing to carry yet";
  }
}

/** "Claude · Opus 4.8": the provider, then its model when one is known. */
export function endpointLabel(
  provider: AgentChatProviderKind,
  modelLabel: string | null,
): string {
  const name = PROVIDER_LABELS[provider];
  return modelLabel ? `${name} · ${modelLabel}` : name;
}

/** A stored model id as people read it, given the roster's label if any. */
export function modelLabel(
  model: string | null,
  rosterLabel: string | null | undefined,
): string | null {
  if (rosterLabel) return rosterLabel;
  if (!model || model === "default") return null;
  return fallbackModelLabel(model);
}

/** The picker's note while it shows another provider than the chat's. */
export function switchHint(target: AgentChatProviderKind): string {
  return `Continues this chat in ${PROVIDER_LABELS[target]}. The conversation so far carries over.`;
}

/** The picker's switch note for `railKey`, or `null`: only a chat that has
 *  started switches in place, and only to another provider. */
export function useSwitchHint(
  threadId: string | null | undefined,
  provider: AgentChatProviderKind,
  railKey: string,
): string | null {
  const started = useAgentChatStore((state) =>
    threadId
      ? (state.threads[threadId]?.messages.some((item) => item.kind === "user_message") ?? false)
      : false,
  );
  if (!started || railKey === provider || !(railKey in PROVIDER_LABELS)) return null;
  return switchHint(railKey as AgentChatProviderKind);
}

const REFUSED = "provider_switch_refused: ";

/** The reason a switch was refused before anything changed (the old
 *  provider still runs), or `null` when the switch failed part-way. */
export function switchRefusal(error: unknown): string | null {
  const message = error instanceof Error ? error.message : String(error);
  const at = message.indexOf(REFUSED);
  return at === -1 ? null : message.slice(at + REFUSED.length);
}
