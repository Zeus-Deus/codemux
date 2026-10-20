/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

const readLocalChatImageMock = vi.fn();
const readChatImageMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

let announceKept: ((paths: string[]) => void) | null = null;
vi.mock("@/tauri/events", () => ({
  onAgentChatReplyImagesKept: (cb: (paths: string[]) => void) => {
    announceKept = cb;
    return Promise.resolve(() => {});
  },
}));

vi.mock("@/tauri/commands", () => ({
  readLocalChatImage: (...args: unknown[]) => readLocalChatImageMock(...args),
  readChatImage: (...args: unknown[]) => readChatImageMock(...args),
}));

import {
  MarkdownLocalImage,
  RetainedImagesProvider,
  resetLocalImageBlobCache,
} from "./MarkdownLocalImage";

beforeEach(() => {
  resetLocalImageBlobCache();
  readLocalChatImageMock.mockReset();
  readChatImageMock.mockReset();
});
afterEach(() => cleanup());

describe("MarkdownLocalImage", () => {
  it("renders an absolute path as a labelled preview card", () => {
    const { container, getByRole } = render(
      <MarkdownLocalImage path="/tmp/proof.png" caption="Terminal screenshot">
        Terminal screenshot
      </MarkdownLocalImage>,
    );

    expect(container.querySelector("[data-chat-local-image]")).toBeInTheDocument();
    expect(getByRole("button", { name: "Open Terminal screenshot" })).toBeInTheDocument();
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "mock-asset:/tmp/proof.png",
    );
    expect(container).toHaveTextContent("Expand");
  });

  it("opens a full-size lightbox when the preview is clicked", () => {
    const { getByRole, getAllByAltText } = render(
      <MarkdownLocalImage path="/tmp/proof.png" caption="Terminal screenshot" />,
    );

    fireEvent.click(getByRole("button", { name: "Open Terminal screenshot" }));

    expect(getByRole("dialog")).toBeInTheDocument();
    expect(getAllByAltText("Terminal screenshot")).toHaveLength(2);
  });

  it("falls back to IPC bytes when the asset URL cannot load", async () => {
    const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
    const createObjectUrl = vi.fn(() => "blob:local-proof");
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectUrl,
    });
    readLocalChatImageMock.mockResolvedValue({
      bytes: new Uint8Array([137, 80, 78, 71]),
      media_type: "image/png",
    });
    const { container } = render(
      <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />,
    );

    fireEvent.error(container.querySelector("img")!);

    await waitFor(() => {
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-proof");
    });
    expect(readLocalChatImageMock).toHaveBeenCalledWith("/tmp/proof.png");
    if (originalCreateObjectUrl) {
      Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
    } else {
      Reflect.deleteProperty(URL, "createObjectURL");
    }
  });

  it("does not reuse a stale blob when the rendered path changes", async () => {
    const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => "blob:first-proof"),
    });
    readLocalChatImageMock.mockResolvedValue({
      bytes: new Uint8Array([137, 80, 78, 71]),
      media_type: "image/png",
    });
    const { container, rerender } = render(
      <MarkdownLocalImage path="/tmp/first.png" caption="First" />,
    );

    fireEvent.error(container.querySelector("img")!);
    await waitFor(() => {
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:first-proof");
    });

    rerender(<MarkdownLocalImage path="/tmp/second.png" caption="Second" />);

    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "mock-asset:/tmp/second.png",
    );
    if (originalCreateObjectUrl) {
      Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
    } else {
      Reflect.deleteProperty(URL, "createObjectURL");
    }
  });

  it("shows a stable unavailable state when both loading paths fail", async () => {
    readLocalChatImageMock.mockRejectedValue(new Error("gone"));
    const { container, findByText } = render(
      <MarkdownLocalImage path="/tmp/missing.png" caption="Missing proof" />,
    );

    fireEvent.error(container.querySelector("img")!);

    expect(await findByText("Image unavailable")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  describe("with a kept copy", () => {
    const kept = [{ source: "/tmp/proof.png", path: "/cfg/images/t/k.png" }];
    const renderKept = () =>
      render(
        <RetainedImagesProvider images={kept}>
          <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />
        </RetainedImagesProvider>,
      );

    it("loads the conversation's copy instead of the original path", () => {
      const { container, getByText } = renderKept();

      expect(container.querySelector("img")).toHaveAttribute(
        "src",
        "mock-asset:/cfg/images/t/k.png",
      );
      // The card still names what the reply linked to.
      expect(getByText("Proof")).toBeInTheDocument();
    });

    it("reads the copy over IPC, then falls back to the original", async () => {
      readChatImageMock.mockRejectedValue(new Error("still copying"));
      const { container } = renderKept();

      fireEvent.error(container.querySelector("img")!);

      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute(
          "src",
          "mock-asset:/tmp/proof.png",
        );
      });
      expect(readChatImageMock).toHaveBeenCalledWith("/cfg/images/t/k.png");
      expect(readLocalChatImageMock).not.toHaveBeenCalled();
    });

    it("is unavailable only when the copy and the original both fail", async () => {
      readChatImageMock.mockRejectedValue(new Error("gone"));
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      const { container, findByText } = renderKept();

      fireEvent.error(container.querySelector("img")!);
      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute(
          "src",
          "mock-asset:/tmp/proof.png",
        );
      });
      fireEvent.error(container.querySelector("img")!);

      expect(await findByText("Image unavailable")).toBeInTheDocument();
      expect(readLocalChatImageMock).toHaveBeenCalledWith("/tmp/proof.png");
    });

    it("loads a copy the backend records after every source failed", async () => {
      const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
      Object.defineProperty(URL, "createObjectURL", {
        configurable: true,
        value: vi.fn(() => "blob:landed-copy"),
      });
      // Still copying on every early read; the original is already gone.
      readChatImageMock.mockRejectedValue(new Error("not yet"));
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      const { container, findByText } = renderKept();

      fireEvent.error(container.querySelector("img")!);
      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "mock-asset:/tmp/proof.png");
      });
      fireEvent.error(container.querySelector("img")!);
      expect(await findByText("Image unavailable")).toBeInTheDocument();
      // However late the copy lands, its announcement loads it.
      await waitFor(() => expect(readChatImageMock).toHaveBeenCalledTimes(2));
      readChatImageMock.mockResolvedValue({
        bytes: new Uint8Array([137, 80, 78, 71]),
        media_type: "image/png",
      });
      announceKept?.(["/cfg/images/t/other.png", "/cfg/images/t/k.png"]);

      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "blob:landed-copy");
      });
      if (originalCreateObjectUrl) {
        Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
      } else {
        Reflect.deleteProperty(URL, "createObjectURL");
      }
    });

    it("reads afresh when the copy is announced during a failing read", async () => {
      const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
      Object.defineProperty(URL, "createObjectURL", {
        configurable: true,
        value: vi.fn(() => "blob:fresh-copy"),
      });
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      let rejectCatchUp: (error: Error) => void = () => {};
      readChatImageMock
        .mockRejectedValueOnce(new Error("not yet"))
        .mockImplementationOnce(
          () => new Promise((_, reject) => { rejectCatchUp = reject; }),
        )
        .mockResolvedValue({ bytes: new Uint8Array([137, 80, 78, 71]), media_type: "image/png" });
      const { container, findByText } = renderKept();

      fireEvent.error(container.querySelector("img")!);
      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "mock-asset:/tmp/proof.png");
      });
      fireEvent.error(container.querySelector("img")!);
      expect(await findByText("Image unavailable")).toBeInTheDocument();
      await waitFor(() => expect(readChatImageMock).toHaveBeenCalledTimes(2));
      // The copy lands while the catch-up read is still failing.
      announceKept?.(["/cfg/images/t/k.png"]);
      rejectCatchUp(new Error("not yet"));

      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "blob:fresh-copy");
      });
      expect(readChatImageMock).toHaveBeenCalledTimes(3);
      if (originalCreateObjectUrl) {
        Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
      } else {
        Reflect.deleteProperty(URL, "createObjectURL");
      }
    });

    it("does not revive a kept copy the browser could not decode", async () => {
      const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
      Object.defineProperty(URL, "createObjectURL", {
        configurable: true,
        value: vi.fn(() => "blob:undecodable"),
      });
      readChatImageMock.mockResolvedValue({ bytes: new Uint8Array([137, 80]), media_type: "image/png" });
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      const { container, findByText } = renderKept();

      fireEvent.error(container.querySelector("img")!);
      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "blob:undecodable");
      });
      fireEvent.error(container.querySelector("img")!);
      await waitFor(() => {
        expect(container.querySelector("img")).toHaveAttribute("src", "mock-asset:/tmp/proof.png");
      });
      fireEvent.error(container.querySelector("img")!);
      expect(await findByText("Image unavailable")).toBeInTheDocument();
      announceKept?.(["/cfg/images/t/k.png"]);
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(container.querySelector("img")).toBeNull();
      expect(readChatImageMock).toHaveBeenCalledTimes(1);
      if (originalCreateObjectUrl) {
        Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
      } else {
        Reflect.deleteProperty(URL, "createObjectURL");
      }
    });

    it("shows a copy the lightbox loaded after the thumbnail gave up on it", async () => {
      const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
      Object.defineProperty(URL, "createObjectURL", {
        configurable: true,
        value: vi.fn(() => "blob:lightbox-copy"),
      });
      readChatImageMock
        .mockRejectedValueOnce(new Error("not yet"))
        .mockResolvedValue({ bytes: new Uint8Array([137, 80, 78, 71]), media_type: "image/png" });
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      const { container, getByRole } = renderKept();
      const thumbnail = () => container.querySelector("[data-chat-local-image] img")!;

      // The thumbnail misses the copy and moves on to the original.
      fireEvent.error(thumbnail());
      await waitFor(() => expect(thumbnail()).toHaveAttribute("src", "mock-asset:/tmp/proof.png"));
      // Meanwhile the lightbox reads the landed copy into the shared cache.
      fireEvent.click(getByRole("button", { name: "Open Proof" }));
      const dialog = getByRole("dialog");
      fireEvent.error(dialog.querySelector("img")!);
      await waitFor(() =>
        expect(dialog.querySelector("img")).toHaveAttribute("src", "blob:lightbox-copy"),
      );
      // The original fails: the thumbnail picks up the cached copy.
      fireEvent.error(thumbnail());
      await waitFor(() => expect(thumbnail()).toHaveAttribute("src", "blob:lightbox-copy"));
      if (originalCreateObjectUrl) {
        Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
      } else {
        Reflect.deleteProperty(URL, "createObjectURL");
      }
    });

    it("starts over when it shows a link again after another", async () => {
      readChatImageMock.mockRejectedValue(new Error("not yet"));
      readLocalChatImageMock.mockRejectedValue(new Error("gone"));
      const card = (path: string) => (
        <RetainedImagesProvider images={kept}>
          <MarkdownLocalImage path={path} caption="Proof" />
        </RetainedImagesProvider>
      );
      const { container, findByText, rerender } = render(card("/tmp/proof.png"));
      fireEvent.error(container.querySelector("img")!);
      await waitFor(() =>
        expect(container.querySelector("img")).toHaveAttribute("src", "mock-asset:/tmp/proof.png"),
      );
      fireEvent.error(container.querySelector("img")!);
      expect(await findByText("Image unavailable")).toBeInTheDocument();

      rerender(card("/tmp/other.png"));
      rerender(card("/tmp/proof.png"));

      expect(container.querySelector("img")).toHaveAttribute(
        "src",
        "mock-asset:/cfg/images/t/k.png",
      );
    });

    it("never takes a copy kept under another spelling of the path", () => {
      const { container } = render(
        <RetainedImagesProvider
          images={[{ source: "/tmp/a b.png", path: "/cfg/images/t/k.png" }]}
        >
          <MarkdownLocalImage path="/tmp/a%20b.png" caption="Proof" />
        </RetainedImagesProvider>,
      );

      expect(container.querySelector("img")).toHaveAttribute(
        "src",
        "mock-asset:/tmp/a%20b.png",
      );
    });

    it("keeps two files whose names differ only in escaping apart", () => {
      const images = [
        { source: "/tmp/a b.png", path: "/cfg/images/t/space.png" },
        { source: "/tmp/a%20b.png", path: "/cfg/images/t/literal.png" },
      ];
      const srcFor = (path: string) => {
        const { container, unmount } = render(
          <RetainedImagesProvider images={images}>
            <MarkdownLocalImage path={path} caption="Proof" />
          </RetainedImagesProvider>,
        );
        const src = container.querySelector("img")?.getAttribute("src");
        unmount();
        return src;
      };

      expect(srcFor("/tmp/a b.png")).toBe("mock-asset:/cfg/images/t/space.png");
      expect(srcFor("/tmp/a%20b.png")).toBe("mock-asset:/cfg/images/t/literal.png");
      expect(srcFor("/tmp/a%20%62.png")).toBe("mock-asset:/tmp/a%20%62.png");
    });

    it("ignores copies kept for other links", () => {
      const { container } = render(
        <RetainedImagesProvider images={kept}>
          <MarkdownLocalImage path="/tmp/other.png" caption="Other" />
        </RetainedImagesProvider>,
      );

      expect(container.querySelector("img")).toHaveAttribute(
        "src",
        "mock-asset:/tmp/other.png",
      );
    });
  });
});
