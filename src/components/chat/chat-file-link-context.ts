import { createContext, useContext } from "react";

export interface ChatFileLinkContextValue {
  workspaceId?: string | null;
  cwd?: string | null;
  /** Absolute paths the surrounding turn's tool calls mentioned; the
   *  stat-verified fallback pool when a chip's resolved path is missing. */
  referencePaths?: readonly string[];
}

export const ChatFileLinkContext = createContext<ChatFileLinkContextValue>({});

export function useChatFileLinkContext() {
  return useContext(ChatFileLinkContext);
}
