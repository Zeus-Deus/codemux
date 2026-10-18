/**
 * Turn a `/btw <question>` draft into a side chat. Kept apart from the
 * store so the chat pane's submit path can call it without pulling the
 * pane's UI into the store module.
 */
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
import type { AgentChatProviderKind } from "@/tauri/types";

import {
  parseBtwCommand,
  sideChatSupported,
  sideChatUnavailableMessage,
  startSideChat,
} from "./side-chat";

/**
 * What a send did with a `/btw` draft:
 * - `null`: not a side question, send normally.
 * - `"started"`: a side chat opened; clear the draft.
 * - `"refused"`: told the user why; keep the draft so they can edit it.
 *
 * Never sends anything to the main session either way.
 */
export function askSideQuestion(args: {
  text: string;
  threadId: string | null;
  provider: AgentChatProviderKind | null | undefined;
  workspaceId: string | null | undefined;
}): "started" | "refused" | null {
  const command = parseBtwCommand(args.text);
  if (!command) return null;
  if (!command.question) {
    toast.error("Add a question after /btw", {
      description: "For example: /btw what does the retry loop do?",
    });
    return "refused";
  }
  if (!sideChatSupported(args.provider)) {
    toast.info(sideChatUnavailableMessage(args.provider));
    return "refused";
  }
  if (!args.threadId || !args.workspaceId) {
    toast.info("Start the chat first", {
      description: "Side questions fork the chat's session, so send a message before asking one.",
    });
    return "refused";
  }
  void startSideChat({
    threadId: args.threadId,
    workspaceId: args.workspaceId,
    provider: args.provider!,
    question: command.question,
  });
  useUIStore.getState().setRightPanelTab(args.workspaceId, "side-chat");
  return "started";
}

/** The draft with a leading `/btw ` added (Ctrl+;), unchanged if present. */
export function withBtwPrefix(draft: string): string {
  if (/^\s*\/btw(\s|$)/i.test(draft)) return draft;
  return `/btw ${draft.replace(/^\s+/, "")}`;
}
