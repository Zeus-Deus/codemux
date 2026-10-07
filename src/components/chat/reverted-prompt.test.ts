import { describe, expect, it, vi } from "vitest";

import type { ChatViewItem, UserMessageItem } from "@/lib/agent-chat/types";

vi.mock("@/tauri/commands", () => ({
  readChatImage: vi.fn(async (path: string) => {
    if (path === "/images/missing.png") throw new Error("gone");
    return { bytes: new Uint8Array([1, 2, 3]), media_type: "image/png" };
  }),
}));

import {
  captureRevertedPrompt,
  findRevertedUserMessage,
  mergeRevertedDraft,
} from "./reverted-prompt";

const message: UserMessageItem = {
  kind: "user_message",
  id: "m1",
  seq: 1,
  text: "Rename the helper",
  clientNonce: "nonce-1",
  images: [
    { src: "/images/a.png", mediaType: "image/png" },
    { src: "data:image/jpeg;base64,AAEC" },
    { src: "/images/missing.png" },
  ],
};

describe("reverted prompt", () => {
  it("finds the user message a checkpoint belongs to", () => {
    const messages: ChatViewItem[] = [
      { kind: "user_message", id: "m0", seq: 0, text: "earlier", clientNonce: "nonce-0" },
      message,
    ];
    expect(findRevertedUserMessage(messages, "nonce-1")).toBe(message);
    expect(findRevertedUserMessage(messages, null)).toBeNull();
    expect(findRevertedUserMessage(messages, "other")).toBeNull();
  });

  it("reads back the text and every image it still can", async () => {
    const prompt = await captureRevertedPrompt(message);
    expect(prompt.text).toBe("Rename the helper");
    expect(prompt.missingImages).toBe(1);
    expect(prompt.images.map((file) => [file.name, file.type, file.size])).toEqual([
      ["reverted-image-1.png", "image/png", 3],
      ["reverted-image-2.jpg", "image/jpeg", 3],
    ]);
  });

  it("puts the prompt ahead of anything already typed", () => {
    expect(mergeRevertedDraft("prompt", "")).toBe("prompt");
    expect(mergeRevertedDraft("prompt", "typed")).toBe("prompt\ntyped");
    expect(mergeRevertedDraft("", "typed")).toBe("typed");
  });
});
