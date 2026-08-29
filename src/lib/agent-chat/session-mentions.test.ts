import { describe, expect, it } from "vitest";

import type { AgentChatSessionMention } from "@/tauri/commands";

import {
  compactSessionPreview,
  removeSessionMentionToken,
  sessionMentionTitle,
  sessionMentionToken,
  sessionProviderLabel,
} from "./session-mentions";

const session: AgentChatSessionMention = {
  thread_id: "thread-8f51ab",
  workspace_id: "workspace-1",
  cwd: "/repo",
  provider: "codex",
  title: "Fix authentication refresh",
  last_active_at: "2026-08-14 08:00:00",
  preview: "Latest progress",
  message_count: 12,
};

describe("session mention presentation", () => {
  it("creates a readable collision-resistant stable token", () => {
    expect(sessionMentionToken(session)).toBe("fix-authentication-ref-8f51ab");
  });

  it("falls back cleanly for missing titles and future providers", () => {
    expect(sessionMentionTitle({ ...session, title: null })).toBe("Chat 8f51ab");
    expect(sessionProviderLabel("gemini")).toBe("Gemini");
    expect(sessionProviderLabel("grok")).toBe("Grok");
    expect(sessionProviderLabel("opencode")).toBe("OpenCode");
  });

  it("compacts previews and removes a dismissed inline token", () => {
    expect(compactSessionPreview("one\n two   three", 50)).toBe(
      "one two three",
    );
    expect(
      removeSessionMentionToken(
        "Use @session:fix-authentication-ref-8f51ab then test",
        "fix-authentication-ref-8f51ab",
      ),
    ).toBe("Use then test");
  });
});
