import { createContext, useContext } from "react";

import type { AgentChatProviderKind } from "@/tauri/types";

/** The transcript's chat provider, for rows whose controls depend on what
 *  the provider supports. `null` when the transcript does not know it. */
export const ChatProviderContext = createContext<AgentChatProviderKind | null>(null);
export function useChatProvider() {
  return useContext(ChatProviderContext);
}
