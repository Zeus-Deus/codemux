import { useProviderRuntimeIntent } from "@/stores/provider-runtime-intent-store";
import { agentChatCreatePane } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/** Provider a pane resolves to when none is pinned — mirrors
 *  `AgentChatPane`'s `pane.provider ?? "claude"`. */
const DEFAULT_LAUNCH_PROVIDER: AgentChatProviderKind = "claude";

/**
 * Create an agent-chat pane the user explicitly asked for.
 *
 * Provider runtimes only start behind user intent (see
 * `provider-runtime-intent-store`): a pane restored from the persisted layout
 * at boot must not spawn a CLI. A launcher click, a `+` on the title bar, a
 * preset tile or an empty project workspace the user just opened IS that
 * intent, so record it before the pane exists. Otherwise the freshly mounted
 * pane sits on "Starting session…" with disabled pickers until the user
 * touches it a second time.
 *
 * Same signature as `agentChatCreatePane`; arguments are forwarded as given.
 */
export function launchAgentChatPane(
  ...args: Parameters<typeof agentChatCreatePane>
): Promise<string> {
  const [, provider] = args;
  useProviderRuntimeIntent
    .getState()
    .observe(provider ?? DEFAULT_LAUNCH_PROVIDER);
  return agentChatCreatePane(...args);
}
