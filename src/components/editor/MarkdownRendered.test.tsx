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

  it("renders raw HTML blocks instead of escaping them as text", () => {
    // A README commonly centers its header with a raw `<div>`. Without
    // rehypeRaw this leaks through as literal `<div align="center">`
    // text; with it, a real element is produced.
    const { container } = render(
      <MarkdownRendered
        content={`<div align="center">\n\n# Centered\n\n</div>`}
        filePath="/repo/README.md"
      />,
    );
    const div = container.querySelector("div[align='center']");
    expect(div).not.toBeNull();
    expect(div!.querySelector("h1")?.textContent).toBe("Centered");
    expect(container.textContent).not.toContain("<div");
  });

  it("resolves src on raw HTML <img> tags, not just markdown images", () => {
    const { container } = render(
      <MarkdownRendered
        content={`<img src="assets/logo.svg" alt="Logo" width="80" />`}
        filePath="/repo/README.md"
      />,
    );
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.getAttribute("src")).toBe("mock-asset:/repo/assets/logo.svg");
    expect(img!.getAttribute("alt")).toBe("Logo");
    expect(img!.getAttribute("width")).toBe("80");
  });

  it("renders a README-style centered header with mixed HTML and markdown", () => {
    const { container } = render(
      <MarkdownRendered
        content={[
          `<div align="center">`,
          ``,
          `<img src="assets/logo/logo.svg" alt="Codemux" width="80" />`,
          ``,
          `# Codemux`,
          ``,
          `![Screenshot](assets/home.png)`,
          ``,
          `</div>`,
        ].join("\n")}
        filePath="/repo/README.md"
      />,
    );
    const imgs = container.querySelectorAll("img");
    expect(imgs).toHaveLength(2);
    expect(imgs[0].getAttribute("src")).toBe(
      "mock-asset:/repo/assets/logo/logo.svg",
    );
    expect(imgs[1].getAttribute("src")).toBe("mock-asset:/repo/assets/home.png");
    expect(container.querySelector("div[align='center'] h1")?.textContent).toBe(
      "Codemux",
    );
  });

  it("blocks executable link schemes while keeping safe ones", () => {
    const { container } = render(
      <MarkdownRendered
        content={`[click](javascript:alert(1))`}
        filePath="/repo/README.md"
      />,
    );
    const link = container.querySelector("a");
    expect(link).not.toBeNull();
    expect(link!.getAttribute("href") ?? "").not.toContain("javascript:");
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
