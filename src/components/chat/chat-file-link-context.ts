import { createContext, useContext } from "react";

export interface ChatFileLinkContextValue {
  workspaceId?: string | null;
  cwd?: string | null;
}

export const ChatFileLinkContext = createContext<ChatFileLinkContextValue>({});

export function useChatFileLinkContext() {
  return useContext(ChatFileLinkContext);
}
