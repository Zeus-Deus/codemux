import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import type { UserMessageItem } from "@/lib/agent-chat/types";

// The IPC read fallback the hydrated fs-path images lean on.
const readChatImageMock = vi.fn();
vi.mock("@/tauri/commands", () => ({
  readChatImage: (...args: unknown[]) => readChatImageMock(...args),
}));

// Force `resolveAssetSrc` to hand an absolute fs path straight to the
// `<img>` (as a webview / dev mock that can't reach the asset protocol
// would), so the load errors and exercises the IPC-read fallback. The
// real `isAbsoluteFsPath` is kept so the fallback's path guard is tested.
vi.mock("@/lib/asset-url", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/asset-url")>();
  return { ...actual, resolveAssetSrc: (src: string | undefined) => src };
});

import { UserMessage } from "./UserMessage";

afterEach(cleanup);

function makeItem(images: UserMessageItem["images"]): UserMessageItem {
  return { kind: "user_message", id: "u1", seq: 0, text: "hi", images };
}

describe("UserMessage image fallback", () => {
  beforeEach(() => {
    readChatImageMock.mockReset();
    // jsdom lacks createObjectURL — stub it so the blob path resolves.
    (
      globalThis.URL as unknown as { createObjectURL: unknown }
    ).createObjectURL = vi.fn(() => "blob:mock-url");
  });

  it("reads the bytes over IPC and swaps in a blob URL when an fs-path image fails to load", async () => {
    readChatImageMock.mockResolvedValue({
      bytes: new Uint8Array([1, 2, 3]),
      media_type: "image/png",
    });
    // Unique path per test so the module-level blob cache doesn't bleed.
    const path = "/abs/hydrated-a.png";
    const { container } = render(
      <UserMessage item={makeItem([{ src: path, mediaType: "image/png" }])} />,
    );

    const img = container.querySelector("img") as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("src")).toBe(path);

    // Simulate the asset-protocol load failing.
    fireEvent.error(img);

    await waitFor(() => {
      expect(readChatImageMock).toHaveBeenCalledWith(path);
    });
    await waitFor(() => {
      const swapped = container.querySelector("img") as HTMLImageElement;
      expect(swapped.getAttribute("src")).toBe("blob:mock-url");
    });
  });

  it("shows the broken-image placeholder when the IPC read also fails", async () => {
    readChatImageMock.mockRejectedValue(new Error("gone"));
    const path = "/abs/hydrated-b.png";
    const { container } = render(
      <UserMessage item={makeItem([{ src: path, mediaType: "image/png" }])} />,
    );

    const img = container.querySelector("img") as HTMLImageElement;
    fireEvent.error(img);

    await waitFor(() => {
      expect(readChatImageMock).toHaveBeenCalledWith(path);
    });
    await waitFor(() => {
      expect(
        container.querySelector('[aria-label="Image failed to load"]'),
      ).not.toBeNull();
    });
  });

  it("never reaches the IPC fallback for a data: URL (passes through)", () => {
    const dataUrl = "data:image/png;base64,AQIDBA==";
    const { container } = render(
      <UserMessage
        item={makeItem([{ src: dataUrl, mediaType: "image/png" }])}
      />,
    );
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img.getAttribute("src")).toBe(dataUrl);
    // No error fired → the read fallback is untouched.
    expect(readChatImageMock).not.toHaveBeenCalled();
  });
});

describe("UserMessage turn revert", () => {
  it("renders a checkpoint-bound action and delegates confirmation upstream", () => {
    const onRevert = vi.fn();
    const { getByRole } = render(
      <UserMessage item={makeItem([])} onRevert={onRevert} />,
    );

    fireEvent.click(
      getByRole("button", { name: "Revert to before this turn" }),
    );

    expect(onRevert).toHaveBeenCalledTimes(1);
  });

  it("does not offer revert on a queued turn", () => {
    const item = {
      ...makeItem([]),
      queued: { queuedId: "queued-1" },
    };
    const { queryByRole } = render(
      <UserMessage item={item} onRevert={() => undefined} />,
    );

    expect(
      queryByRole("button", { name: "Revert to before this turn" }),
    ).toBeNull();
  });
});
