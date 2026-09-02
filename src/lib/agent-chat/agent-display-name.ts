import type { AgentChatProviderKind } from "@/tauri/types";

/** What the user calls the CLI behind a provider kind — the name that
 *  appears in row metadata ("master · Claude Code") and the resumed-from-
 *  the-terminal divider. Distinct from the short provider label used in
 *  error copy, where "Claude" reads better than the product name. */
const AGENT_DISPLAY_NAME: Record<AgentChatProviderKind, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  opencode: "OpenCode",
};

export function agentDisplayName(
  provider: AgentChatProviderKind | string | null | undefined,
): string {
  if (typeof provider === "string" && provider in AGENT_DISPLAY_NAME) {
    return AGENT_DISPLAY_NAME[provider as AgentChatProviderKind];
  }
  return "the agent";
}
