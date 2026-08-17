import { createContext, useContext } from "react";

export interface ChatFileLinkContextValue {
  workspaceId?: string | null;
  cwd?: string | null;
  /** The workspace root, when `cwd` is a tool working directory instead. A
   *  relative reference also resolves against this as a click-time fallback,
   *  so prose naming a repo-root file still opens while a tool runs in a
   *  subdirectory. */
  workspaceCwd?: string | null;
  /** Absolute paths the surrounding turn's tool calls mentioned; the
   *  stat-verified fallback pool when a chip's resolved path is missing. */
  referencePaths?: readonly string[];
}

export const ChatFileLinkContext = createContext<ChatFileLinkContextValue>({});

export function useChatFileLinkContext() {
  return useContext(ChatFileLinkContext);
}
