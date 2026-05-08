/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

import { MarkdownRendered } from "./MarkdownRendered";

afterEach(() => cleanup());

describe("MarkdownRendered", () => {
  it("rewrites relative image paths against the markdown file's dir", () => {
    const { container } = render(
      <MarkdownRendered
        content={`![alt](./assets/img.png)`}
        filePath="/repo/docs/guide.md"
      />,
    );
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.getAttribute("src")).toBe(
      "mock-asset:/repo/docs/assets/img.png",
    );
    expect(img!.getAttribute("alt")).toBe("alt");
  });

  it("rewrites absolute image paths via convertFileSrc", () => {
    const { container } = render(
      <MarkdownRendered
        content={`![](/abs/x.png)`}
        filePath="/repo/docs/guide.md"
      />,
    );
    const img = container.querySelector("img");
    expect(img!.getAttribute("src")).toBe("mock-asset:/abs/x.png");
  });

  it("leaves remote URLs alone", () => {
    const { container } = render(
      <MarkdownRendered
        content={`![](https://example.com/x.png)`}
        filePath="/repo/docs/guide.md"
      />,
    );
    const img = container.querySelector("img");
    expect(img!.getAttribute("src")).toBe("https://example.com/x.png");
  });

  it("leaves data URIs alone", () => {
    const { container } = render(
      <MarkdownRendered
        content={`![](data:image/png;base64,AAA)`}
        filePath="/repo/docs/guide.md"
      />,
    );
    const img = container.querySelector("img");
    expect(img!.getAttribute("src")).toBe("data:image/png;base64,AAA");
  });

  it("renders relative paths verbatim when no filePath is supplied", () => {
    // No base path → can't resolve. We pass through so the browser
    // gets a chance and the document still renders something.
    const { container } = render(
      <MarkdownRendered content={`![](./x.png)`} filePath={null} />,
    );
    const img = container.querySelector("img");
    expect(img!.getAttribute("src")).toBe("./x.png");
  });

  it("still renders headings, code, and other markdown around images", () => {
    const { container, getByText } = render(
      <MarkdownRendered
        content={`# Title\n\nText\n\n![pic](./x.png)`}
        filePath="/r/d.md"
      />,
    );
    expect(getByText("Title")).toBeInTheDocument();
    expect(getByText("Text")).toBeInTheDocument();
    const img = container.querySelector("img");
    expect(img!.getAttribute("src")).toBe("mock-asset:/r/x.png");
  });
});
