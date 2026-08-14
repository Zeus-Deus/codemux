import type { AttachmentMetadata } from "@/stores/agent-chat-store";
import type { AgentChatSessionContext } from "@/tauri/commands";

export function metadataFromSessionContext(
  context: AgentChatSessionContext,
  previous: AttachmentMetadata,
): AttachmentMetadata {
  return {
    ...previous,
    label: context.title?.trim() || previous.label,
    sourceProvider: context.provider,
    sourceCwd: context.cwd,
    sourceUpdatedAt: context.last_active_at,
    messageCount: context.message_count,
    includedMessageCount: context.included_message_count,
    isContextTruncated: context.truncated,
    handoffKind: context.handoff_kind,
    summaryCached: context.summary_cached,
    summaryError: context.summary_error,
    summarizerProvider: context.summarizer_provider,
    summarizerModel: context.summarizer_model,
    summarizerEffort: context.summarizer_effort,
    sourceRevision: context.revision_message_id,
    fullHistoryAvailable: context.full_history_available,
    fetchedAt: Date.now(),
    isLoading: false,
    // A successful read supersedes any earlier failure. Without this the
    // `...previous` spread would keep a stale `error` on a chip that has since
    // resolved, so the chip would render the failure indicator for content
    // that is about to be sent.
    error: undefined,
  };
}

export function utilitySummaryFallbackLabel(
  error: string | null | undefined,
): string {
  switch (error) {
    case "utility_model_required":
      return "Choose a Utility agent in Settings";
    case "utility_agent_unavailable":
      return "Utility agent unavailable — direct transcript attached";
    case "utility_agent_timeout":
      return "Summary timed out — direct transcript attached";
    case "utility_summary_failed":
      return "Summary unavailable — direct transcript attached";
    default:
      return "Direct transcript attached";
  }
}
