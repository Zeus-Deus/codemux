/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

const readLocalChatImageMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

vi.mock("@/tauri/commands", () => ({
  readLocalChatImage: (...args: unknown[]) => readLocalChatImageMock(...args),
}));

import {
  MarkdownLocalImage,
  resetLocalImageBlobCache,
} from "./MarkdownLocalImage";

beforeEach(() => {
  resetLocalImageBlobCache();
  readLocalChatImageMock.mockReset();
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
});
