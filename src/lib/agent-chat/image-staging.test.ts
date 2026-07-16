import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const stageChatImageMock = vi.fn();
const discardStagedChatImageMock = vi.fn();
vi.mock("@/tauri/commands", () => ({
  stageChatImage: (...a: unknown[]) => stageChatImageMock(...a),
  discardStagedChatImage: (...a: unknown[]) => discardStagedChatImageMock(...a),
}));

import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  __resetImageStagingForTests,
  awaitImageStaging,
  beginImageStaging,
  discardStagedImage,
} from "./image-staging";

const THREAD = "t-stage";

function seedImageChip(id: string) {
  const store = useAgentChatStore.getState();
  store.ensureThread(THREAD);
  store.addStagedAttachment(THREAD, {
    id,
    kind: "image",
    ref: `image:${id}`,
    metadata: { label: "shot.png", isLoading: false },
    resolvedImage: { mime: "image/png", bytes: new Uint8Array([1, 2, 3]) },
  });
}

function chip(id: string) {
  return useAgentChatStore
    .getState()
    .threads[THREAD]?.stagedAttachments.find((a) => a.id === id);
}

describe("image-staging", () => {
  beforeEach(() => {
    stageChatImageMock.mockReset();
    discardStagedChatImageMock.mockReset().mockResolvedValue(undefined);
    __resetImageStagingForTests();
    useAgentChatStore.setState({ threads: {} });
  });
  afterEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("patches the chip with stagedImage once staging resolves", async () => {
    stageChatImageMock.mockResolvedValue({
      path: "/staging/a.png",
      media_type: "image/png",
    });
    seedImageChip("a");
    beginImageStaging(THREAD, "a", new Uint8Array([1, 2, 3]), "image/png");
    await awaitImageStaging(["a"]);
    expect(stageChatImageMock).toHaveBeenCalledWith(
      new Uint8Array([1, 2, 3]),
      "image/png",
    );
    expect(chip("a")?.stagedImage).toEqual({
      path: "/staging/a.png",
      mediaType: "image/png",
    });
  });

  it("marks the chip with an error and leaves stagedImage unset on failure", async () => {
    stageChatImageMock.mockRejectedValue(new Error("disk full"));
    seedImageChip("b");
    beginImageStaging(THREAD, "b", new Uint8Array([1]), "image/png");
    await awaitImageStaging(["b"]);
    expect(chip("b")?.stagedImage).toBeUndefined();
    expect(chip("b")?.metadata.error).toContain("disk full");
    // Existing metadata (label) is preserved through the error patch.
    expect(chip("b")?.metadata.label).toBe("shot.png");
  });

  it("awaitImageStaging is a no-op for ids with no in-flight staging", async () => {
    await expect(awaitImageStaging(["nope"])).resolves.toBeUndefined();
  });

  it("discardStagedImage fires the backend delete best-effort", () => {
    discardStagedImage("/staging/a.png");
    expect(discardStagedChatImageMock).toHaveBeenCalledWith("/staging/a.png");
  });
});
