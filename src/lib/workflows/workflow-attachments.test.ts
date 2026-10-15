import { describe, expect, it } from "vitest";
import type { Attachment } from "@/stores/agent-chat-store";
import { workflowAttachmentProblem } from "./workflow-attachments";

const attachment = (overrides: Partial<Omit<Attachment, "metadata">> & { metadata?: Partial<Attachment["metadata"]> } = {}): Attachment => ({
  id: "a", kind: "pr", ref: "!509", resolvedContent: "PR body", ...overrides,
  metadata: { label: "#509 Workflows", ...overrides.metadata },
} as Attachment);

describe("workflowAttachmentProblem", () => {
  it("accepts resolved text attachments", () => {
    expect(workflowAttachmentProblem([attachment()])).toBeNull();
    expect(workflowAttachmentProblem([])).toBeNull();
  });

  it("refuses images, loading attachments and attachments that failed to load", () => {
    expect(workflowAttachmentProblem([attachment({ kind: "image", resolvedContent: undefined })])?.title).toBe("Workflows can't use images yet");
    expect(workflowAttachmentProblem([attachment({ resolvedContent: undefined, metadata: { isLoading: true } })])).toEqual({
      title: "Attachments are still loading", description: "Wait for #509 Workflows to finish, then send again.",
    });
    expect(workflowAttachmentProblem([attachment({ resolvedContent: "", metadata: { error: "gh failed" } })])?.title).toBe("An attachment couldn't be loaded");
  });
});
