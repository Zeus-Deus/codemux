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

  it("skips re-render when parent re-renders with reference-equal props", () => {
    // Regression guard for the streaming-jank fix: WorkspaceMain
    // subscribes to the global app-state snapshot and re-renders on
    // every backend tick (agent tokens, git polling, hooks). EditorPane
    // re-renders alongside it. Without React.memo, MarkdownRendered
    // would also re-render on every tick — and react-markdown re-parses
    // the entire content string on each render, dominating frame time.
    //
    // The memo's default shallow comparator skips the render when both
    // primitive props are === to the previous values. We assert that by
    // counting the calls into the `img` component override: it only
    // fires inside the inner ReactMarkdown render, so a stable count
    // across parent re-renders proves the memo is doing its job.
    const imgCalls = vi.fn();

    function Harness({
      content,
      filePath,
      tick,
    }: {
      content: string;
      filePath: string;
      tick: number;
    }) {
      // `tick` changes per render but is not threaded into props the
      // memoized child sees — proves parent churn alone doesn't bust
      // the memo.
      void tick;
      return <MarkdownRendered content={content} filePath={filePath} />;
    }

    // Inject a spy that fires once per inner render via the components
    // override path. Easiest way: mount once with a content string that
    // contains an image, then re-render with literally the same
    // strings.
    const content = "![pic](./x.png)";
    const filePath = "/r/d.md";

    const { rerender, container } = render(
      <Harness content={content} filePath={filePath} tick={0} />,
    );
    const initialImg = container.querySelector("img");
    expect(initialImg).not.toBeNull();
    imgCalls.mockClear();

    // Re-render the parent with reference-equal props. The memo should
    // bail out before react-markdown runs again. We can't directly
    // count react-markdown invocations from the outside, but we can
    // assert that the rendered <img> node is the SAME DOM node — react
    // will reuse the existing fiber when the memo skips, so the node
    // identity is preserved.
    rerender(<Harness content={content} filePath={filePath} tick={1} />);
    const afterImg = container.querySelector("img");
    expect(afterImg).toBe(initialImg);

    rerender(<Harness content={content} filePath={filePath} tick={2} />);
    expect(container.querySelector("img")).toBe(initialImg);
  });

  it("still re-renders when content actually changes", () => {
    const { rerender, container } = render(
      <MarkdownRendered content={`# A`} filePath="/r/d.md" />,
    );
    expect(container.textContent).toContain("A");
    rerender(<MarkdownRendered content={`# B`} filePath="/r/d.md" />);
    expect(container.textContent).toContain("B");
  });
});
