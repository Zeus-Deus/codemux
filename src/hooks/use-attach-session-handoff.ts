import { useCallback } from "react";

import {
  ATTACHMENT_HARD_LIMIT,
  SESSION_ATTACHMENT_LIMIT,
} from "@/lib/agent-chat/attachment-limits";
import { metadataFromSessionContext } from "@/lib/agent-chat/session-handoff";
import {
  sessionMentionTitle,
  sessionMentionToken,
} from "@/lib/agent-chat/session-mentions";
import { toast } from "@/lib/toast";
import { utilitySelectionFromStores } from "@/lib/utility-agent";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useUIStore } from "@/stores/ui-store";
import {
  agentChatGetSessionContext,
  type AgentChatSessionMention,
} from "@/tauri/commands";

/**
 * Stage a persisted GUI conversation as a provider-neutral handoff.
 *
 * The source thread id is the durable reference; its title is presentation
 * only, so renaming a chat cannot break an already-staged mention. Shared by
 * the live pane and the draft surface so both enforce the same limits and
 * surface the same "no Utility agent" nudge.
 *
 * `threadId` / `workspaceId` are nullable because the draft surface only gains
 * a workspace once its target points at one; the returned callback is a no-op
 * until both are known.
 */
export function useAttachSessionHandoff(
  threadId: string | null,
  workspaceId: string | null,
): (session: AgentChatSessionMention) => void {
  const addStagedAttachment = useAgentChatStore((s) => s.addStagedAttachment);
  const updateStagedAttachment = useAgentChatStore(
    (s) => s.updateStagedAttachment,
  );

  return useCallback(
    (session: AgentChatSessionMention) => {
      if (!threadId || !workspaceId) return;
      const liveAttachments =
        useAgentChatStore.getState().threads[threadId]?.stagedAttachments ?? [];
      if (
        liveAttachments.some(
          (attachment) =>
            attachment.kind === "session" &&
            attachment.ref === session.thread_id,
        )
      ) {
        return;
      }
      if (
        liveAttachments.filter((attachment) => attachment.kind === "session")
          .length >= SESSION_ATTACHMENT_LIMIT
      ) {
        toast.error("Conversation handoff limit reached", {
          description: `Attach up to ${SESSION_ATTACHMENT_LIMIT} conversations per message.`,
        });
        return;
      }
      if (liveAttachments.length >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }

      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const label = sessionMentionTitle(session);
      const mentionToken = sessionMentionToken(session);
      addStagedAttachment(threadId, {
        id,
        kind: "session",
        ref: session.thread_id,
        metadata: {
          label,
          mentionToken,
          sourceProvider: session.provider,
          sourceCwd: session.cwd,
          sourceUpdatedAt: session.last_active_at,
          messageCount: session.message_count,
          isLoading: true,
        },
      });
      void agentChatGetSessionContext(
        workspaceId,
        session.thread_id,
        utilitySelectionFromStores(),
      )
        .then((context) => {
          updateStagedAttachment(threadId, id, {
            resolvedContent: context.content,
            metadata: metadataFromSessionContext(context, {
              label,
              mentionToken,
            }),
          });
          if (
            context.summary_error === "utility_model_required" ||
            context.summary_error === "utility_agent_unavailable"
          ) {
            toast.warning("Direct transcript attached", {
              description:
                "Choose or change the Utility agent to generate clean handoff summaries.",
              action: {
                label: "Choose agent",
                onClick: () =>
                  useUIStore.getState().setShowSettings(true, "agent"),
              },
            });
          }
        })
        .catch((error) => {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label,
              mentionToken,
              sourceProvider: session.provider,
              sourceCwd: session.cwd,
              sourceUpdatedAt: session.last_active_at,
              messageCount: session.message_count,
              isLoading: false,
              error: String(error),
            },
          });
        });
    },
    [threadId, workspaceId, addStagedAttachment, updateStagedAttachment],
  );
}
