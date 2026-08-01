import { createContext, useContext } from "react";

/**
 * Whether the markdown currently being rendered is still arriving token by
 * token.
 *
 * This is a context rather than a plugin option on purpose: `ChatMarkdown`
 * passes Streamdown module-level constant plugin/component arrays so its
 * internal processor cache stays keyed on stable references. A context lets
 * per-message state reach the custom elements those plugins emit without
 * rebuilding the plugin list per render (and context updates cross `memo`
 * boundaries, so a message settling still repaints its links).
 */
export const ChatMarkdownStreamingContext = createContext(false);

export function useChatMarkdownStreaming(): boolean {
  return useContext(ChatMarkdownStreamingContext);
}
