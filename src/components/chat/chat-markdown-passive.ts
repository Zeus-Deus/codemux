import { createContext, useContext } from "react";

/** Imported prose and unresolved history must not request resources on display. */
export const ChatMarkdownPassiveContext = createContext(false);
export function useChatMarkdownPassive() {
  return useContext(ChatMarkdownPassiveContext);
}
