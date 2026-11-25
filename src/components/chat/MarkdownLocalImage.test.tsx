/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

const readLocalChatImageMock = vi.fn();
const readChatImageMock = vi.fn();
const convertFileSrcMock = vi.hoisted(() => vi.fn((path: string) => "mock-asset:" + path));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => convertFileSrcMock(path),
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

const imageBytes = { bytes: new Uint8Array([137, 80, 78, 71]), media_type: "image/png" };
const kept = [{ source: "/tmp/proof.png", path: "/cfg/images/t/k.png" }];
const originalCreateObjectUrl = Object.getOwnPropertyDescriptor(URL, "createObjectURL");

beforeEach(() => {
  resetLocalImageBlobCache();
  readLocalChatImageMock.mockReset().mockResolvedValue(imageBytes);
  readChatImageMock.mockReset().mockResolvedValue(imageBytes);
  convertFileSrcMock.mockClear();
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    value: vi.fn(() => "blob:local-image"),
  });
});

afterEach(() => {
  cleanup();
  if (originalCreateObjectUrl) {
    Object.defineProperty(URL, "createObjectURL", originalCreateObjectUrl);
  } else {
    Reflect.deleteProperty(URL, "createObjectURL");
  }
});

describe("MarkdownLocalImage", () => {
  it("discloses the exact original path without reading or rendering it before Load", async () => {
    const path = "/tmp/shots/proof.png";
    const { container, getByRole } = render(
      <MarkdownLocalImage path={path} caption="Screenshot" />,
    );

    expect(getByRole("button", { name: "Load Screenshot" })).toHaveTextContent(path);
    expect(container.querySelector("img")).toBeNull();
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
    expect(convertFileSrcMock).not.toHaveBeenCalled();

    fireEvent.click(getByRole("button", { name: "Load Screenshot" }));
    await waitFor(() => expect(readLocalChatImageMock).toHaveBeenCalledExactlyOnceWith(path));
    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-image"),
    );
    expect(convertFileSrcMock).not.toHaveBeenCalledWith(path);

    fireEvent.click(getByRole("button", { name: "Open Screenshot" }));
    expect(getByRole("dialog")).toBeInTheDocument();
  });

  it("shows a mapped-drive path before consent and never starts a passive read", () => {
    const path = "Z:\\shared\\proof.png";
    const { container, getByRole } = render(
      <MarkdownLocalImage path={path} caption="Proof" />,
    );
    expect(getByRole("button", { name: "Load Proof" })).toHaveTextContent(path);
    expect(container.querySelector("img")).toBeNull();
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
    expect(convertFileSrcMock).not.toHaveBeenCalled();
  });

  it("requires a new click when the displayed path changes", async () => {
    const { container, getByRole, rerender } = render(
      <MarkdownLocalImage path="/tmp/first.png" caption="First" />,
    );
    fireEvent.click(getByRole("button", { name: "Load First" }));
    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-image"),
    );

    rerender(<MarkdownLocalImage path="/tmp/second.png" caption="Second" />);
    expect(getByRole("button", { name: "Load Second" })).toHaveTextContent("/tmp/second.png");
    expect(container.querySelector("img")).toBeNull();
    expect(readLocalChatImageMock).not.toHaveBeenCalledWith("/tmp/second.png");
    expect(convertFileSrcMock).not.toHaveBeenCalledWith("/tmp/second.png");
  });

  it("shows unavailable after an explicit original read fails", async () => {
    readLocalChatImageMock.mockRejectedValue(new Error("gone"));
    const { container, getByRole, findByText } = render(
      <MarkdownLocalImage path="/tmp/missing.png" caption="Missing" />,
    );
    expect(container.querySelector("img")).toBeNull();
    fireEvent.click(getByRole("button", { name: "Load Missing" }));
    expect(await findByText("Image unavailable")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  it("auto-previews a retained app-owned copy without touching its original", () => {
    const { container, getByRole } = render(
      <RetainedImagesProvider images={kept}>
        <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />
      </RetainedImagesProvider>,
    );
    expect(getByRole("button", { name: "Open Proof" })).toBeInTheDocument();
    expect(container.querySelector("img")).toHaveAttribute("src", "mock-asset:/cfg/images/t/k.png");
    expect(convertFileSrcMock).toHaveBeenCalledWith("/cfg/images/t/k.png");
    expect(convertFileSrcMock).not.toHaveBeenCalledWith("/tmp/proof.png");
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
  });

  it("falls back to bounded IPC for a retained copy without reading the original", async () => {
    const { container } = render(
      <RetainedImagesProvider images={kept}>
        <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />
      </RetainedImagesProvider>,
    );
    fireEvent.error(container.querySelector("img")!);
    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-image"),
    );
    expect(readChatImageMock).toHaveBeenCalledWith("/cfg/images/t/k.png");
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
  });

  it("offers an explicit original load only after the retained copy fails", async () => {
    readChatImageMock.mockRejectedValue(new Error("copy missing"));
    const { container, getByRole } = render(
      <RetainedImagesProvider images={kept}>
        <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />
      </RetainedImagesProvider>,
    );
    fireEvent.error(container.querySelector("img")!);
    await waitFor(() =>
      expect(getByRole("button", { name: "Load Proof" })).toHaveTextContent("/tmp/proof.png"),
    );
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
    expect(convertFileSrcMock).not.toHaveBeenCalledWith("/tmp/proof.png");

    fireEvent.click(getByRole("button", { name: "Load Proof" }));
    await waitFor(() =>
      expect(readLocalChatImageMock).toHaveBeenCalledExactlyOnceWith("/tmp/proof.png"),
    );
    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-image"),
    );
    expect(convertFileSrcMock).not.toHaveBeenCalledWith("/tmp/proof.png");
  });

  it("can display a retained copy announced after its first read failed", async () => {
    readChatImageMock.mockRejectedValue(new Error("still copying"));
    const { container, getByRole } = render(
      <RetainedImagesProvider images={kept}>
        <MarkdownLocalImage path="/tmp/proof.png" caption="Proof" />
      </RetainedImagesProvider>,
    );
    fireEvent.error(container.querySelector("img")!);
    await waitFor(() => expect(getByRole("button", { name: "Load Proof" })).toBeInTheDocument());
    readChatImageMock.mockResolvedValue(imageBytes);
    announceKept?.(["/cfg/images/t/k.png"]);
    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute("src", "blob:local-image"),
    );
    expect(readLocalChatImageMock).not.toHaveBeenCalled();
  });

  it("uses only an exact retained source path", () => {
    const { container, getByRole } = render(
      <RetainedImagesProvider images={[{ source: "/tmp/a b.png", path: "/cfg/images/t/k.png" }]}>
        <MarkdownLocalImage path="/tmp/a%20b.png" caption="Proof" />
      </RetainedImagesProvider>,
    );
    expect(getByRole("button", { name: "Load Proof" })).toHaveTextContent("/tmp/a%20b.png");
    expect(container.querySelector("img")).toBeNull();
    expect(convertFileSrcMock).not.toHaveBeenCalled();
  });
});
