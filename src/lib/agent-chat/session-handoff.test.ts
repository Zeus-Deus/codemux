import { describe, expect, it } from "vitest";

import { metadataFromSessionContext } from "@/lib/agent-chat/session-handoff";
import type { AgentChatSessionContext } from "@/tauri/commands";

const context: AgentChatSessionContext = {
  thread_id: "source-thread-abc123",
  workspace_id: "ws-1",
  cwd: "/projects/foo",
  provider: "codex",
  title: "Authentication follow-up",
  last_active_at: "2026-08-14T09:00:00Z",
  content: "User:\nHarden authentication.",
  message_count: 8,
  included_message_count: 8,
  truncated: false,
  handoff_kind: "summary",
  summary_cached: false,
  summary_error: null,
  summarizer_provider: "codex",
  summarizer_model: "gpt-5.6-luna",
  summarizer_effort: "low",
  revision_message_id: 44,
  full_history_available: true,
};

describe("metadataFromSessionContext", () => {
  it("keeps presentation fields the context does not carry", () => {
    const metadata = metadataFromSessionContext(context, {
      label: "stale label",
      mentionToken: "authentication-abc123",
    });
    expect(metadata.mentionToken).toBe("authentication-abc123");
    // A titled context wins over the label captured at attach time.
    expect(metadata.label).toBe("Authentication follow-up");
    expect(metadata.isLoading).toBe(false);
  });

  it("clears an earlier failure once the refresh succeeds", () => {
    // A chip that failed its attach-time read and then resolved on the
    // pre-send refresh must not keep rendering the error indicator: its
    // content is about to be sent.
    const metadata = metadataFromSessionContext(context, {
      label: "Authentication follow-up",
      mentionToken: "authentication-abc123",
      isLoading: false,
      error: "conversation not found",
    });
    expect(metadata.error).toBeUndefined();
  });
});
