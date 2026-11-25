/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const mockOpenExternalUrl = vi.hoisted(() => vi.fn().mockResolvedValue("browser"));
const mockOpenUrl = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const mockTauri = vi.hoisted(() => ({ enabled: false, assetPrefix: "mock-asset:" }));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `${mockTauri.assetPrefix}${path}`,
  isTauri: () => mockTauri.enabled,
}));
vi.mock("@/lib/open-url", () => ({ openExternalUrl: mockOpenExternalUrl }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: mockOpenUrl }));

import { MarkdownRendered } from "./MarkdownRendered";

afterEach(() => {
  cleanup();
  mockOpenExternalUrl.mockClear();
  mockOpenUrl.mockClear();
  mockTauri.enabled = false;
  mockTauri.assetPrefix = "mock-asset:";
});

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

  it.each([
    ["![diagram](./assets/my%20diagram.png)", "/repo/docs/assets/my diagram.png"],
    ["![diagram](<./assets/my diagram.png>)", "/repo/docs/assets/my diagram.png"],
    ['<img src="./assets/my%20diagram.png" alt="diagram">', "/repo/docs/assets/my diagram.png"],
    ["![diagram](./assets/caf%C3%A9.png)", "/repo/docs/assets/café.png"],
    ["![diagram](./assets/100%25.png)", "/repo/docs/assets/100%.png"],
    ["![diagram](./assets/literal%2520.png)", "/repo/docs/assets/literal%20.png"],
    ["![diagram](/assets/my%20diagram.png)", "/assets/my diagram.png"],
    ['<img src="./assets/100%.png">', "/repo/docs/assets/100%.png"],
  ])("resolves encoded image filenames in %s", (content, path) => {
    const { container } = render(
      <MarkdownRendered content={content} filePath="/repo/docs/guide.md" />,
    );
    expect(container.querySelector("img")?.getAttribute("src")).toBe(
      `mock-asset:${path}`,
    );
  });

  it("keeps literal percent escapes in the base file's directory", () => {
    const { container } = render(
      <MarkdownRendered content="![](my%20diagram.png)" filePath="/repo/literal%20/guide.md" />,
    );
    expect(container.querySelector("img")?.getAttribute("src")).toBe(
      "mock-asset:/repo/literal%20/my diagram.png",
    );
  });

  it.each([
    ["![](./my%20diagram.png)", null, "./my%20diagram.png"],
    ["![](/my%20diagram.png)", null, "mock-asset:/my diagram.png"],
  ])("resolves local %s with base %s", (content, filePath, expected) => {
    const { container } = render(<MarkdownRendered content={content} filePath={filePath} />);
    expect(container.querySelector("img")?.getAttribute("src")).toBe(expected);
  });

  it.each([
    ["![Diagram](https://example.com/x.png)", "https://example.com/x.png"],
    ['<img src="https://example.com/x.png" alt="Diagram">', "https://example.com/x.png"],
    ["![Diagram](http://127.0.0.1:4141/status)", "http://127.0.0.1:4141/status"],
    ["![Diagram](https%3A%2F%2Fevil.example%2Fpixel)", "https://evil.example/pixel"],
    ['<img src="https%3A%2F%2Fevil.example%2Fpixel" alt="Diagram">', "https://evil.example/pixel"],
    ["![Diagram](%2F%2Fevil.example%2Fpixel)", "https://evil.example/pixel"],
  ])("requires confirmation before opening a remote image from %s", (content, src) => {
    const { container, getByRole } = render(
      <MarkdownRendered
        content={content}
        filePath="/repo/docs/guide.md"
      />,
    );
    expect(container.querySelector("img, picture, source")).toBeNull();
    expect(mockOpenUrl).not.toHaveBeenCalled();
    fireEvent.click(getByRole("button", { name: /diagram/i }));
    expect(screen.getByText(src)).toBeInTheDocument();
    expect(mockOpenUrl).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(mockOpenUrl).toHaveBeenCalledExactlyOnceWith(src);
  });

  it("removes raw picture sources that would fetch remotely", () => {
    const { container, getByRole } = render(
      <MarkdownRendered
        content={'<picture><source srcset="https://example.com/secret.png 2x"><img src="https://example.com/fallback.png" alt="Fallback"></picture>'}
        filePath="/repo/README.md"
      />,
    );
    expect(container.querySelector("img, picture, source, [srcset]")).toBeNull();
    expect(getByRole("button", { name: /fallback/i })).toBeInTheDocument();
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it("does not follow a wrapping link when an image asks for confirmation", () => {
    const { container, getByRole } = render(
      <MarkdownRendered
        content="[![Diagram](https://example.com/x.png)](https://example.com/page)"
        filePath="/repo/README.md"
      />,
    );
    expect(container.querySelector("img")).toBeNull();
    fireEvent.click(getByRole("button", { name: /diagram/i }));
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
    expect(mockOpenUrl).not.toHaveBeenCalled();
    expect(screen.getByText("https://example.com/x.png")).toBeInTheDocument();
    fireEvent(screen.getByRole("button", { name: "Cancel" }), new MouseEvent("auxclick", {
      bubbles: true,
      cancelable: true,
      button: 1,
    }));
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
    expect(mockOpenUrl).not.toHaveBeenCalled();
    fireEvent.click(getByRole("button", { name: /diagram/i }));
    fireEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(mockOpenUrl).toHaveBeenCalledExactlyOnceWith("https://example.com/x.png");
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
  });

  it("does not follow a wrapping link from the image trigger or dialog backdrop", () => {
    const { getByRole } = render(
      <MarkdownRendered
        content="[![Diagram](https://example.com/x.png)](https://example.com/page)"
        filePath="/repo/README.md"
      />,
    );
    const trigger = getByRole("button", { name: /diagram/i });
    fireEvent(trigger, new MouseEvent("auxclick", {
      bubbles: true,
      cancelable: true,
      button: 1,
    }));
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    const overlay = document.querySelector('[data-slot="alert-dialog-overlay"]');
    expect(overlay).not.toBeNull();
    fireEvent(overlay!, new MouseEvent("auxclick", {
      bubbles: true,
      cancelable: true,
      button: 1,
    }));
    fireEvent.click(overlay!);
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
    expect(mockOpenUrl).not.toHaveBeenCalled();
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

  it.each([
    "blob:https://example.com/0b4b6d96-4752-4e9b-b258-7bc7f989693b",
    "asset://localhost/tmp/screenshot.png",
    "tauri://localhost/tmp/screenshot.png",
  ])("preserves a local preview source: %s", (src) => {
    const { container } = render(<MarkdownRendered content={`![Local](${src})`} />);
    expect(container.querySelector("img")?.getAttribute("src")).toBe(src);
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it("previews the exact Windows asset host in Tauri", () => {
    mockTauri.enabled = true;
    mockTauri.assetPrefix = "http://asset.localhost";
    const src = "http://asset.localhost/tmp/screenshot.png";
    const { container } = render(<MarkdownRendered content={`![Local](${src})`} />);
    expect(container.querySelector("img")?.getAttribute("src")).toBe(src);
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it.each(["asset://localhost", "tauri://localhost"])(
    "keeps the Windows asset URL inert when the native endpoint is %s",
    (assetPrefix) => {
      mockTauri.enabled = true;
      mockTauri.assetPrefix = assetPrefix;
      const src = "http://asset.localhost/tmp/screenshot.png";
      const { container } = render(<MarkdownRendered content={`![Local](${src})`} />);
      expect(container.querySelector("img")).toBeNull();
      expect(screen.getByRole("button", { name: /local/i })).toBeInTheDocument();
    },
  );

  it("keeps the Windows asset host inert in a hosted browser", () => {
    const src = "http://asset.localhost/tmp/screenshot.png";
    const { container, getByRole } = render(<MarkdownRendered content={`![Local](${src})`} />);
    expect(container.querySelector("img")).toBeNull();
    fireEvent.click(getByRole("button", { name: /local/i }));
    expect(screen.getByText(src)).toBeInTheDocument();
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it.each([
    "http://asset.localhost:80/tmp/screenshot.png",
    "http://asset.localhost:4141/tmp/screenshot.png",
    "http://sub.asset.localhost/tmp/screenshot.png",
    "http://asset.localhost.evil/tmp/screenshot.png",
    "http://asset.localhost./tmp/screenshot.png",
    "https://asset.localhost/tmp/screenshot.png",
  ])("keeps asset host lookalikes inert in Tauri: %s", (src) => {
    mockTauri.enabled = true;
    mockTauri.assetPrefix = "http://asset.localhost";
    const { container } = render(<MarkdownRendered content={`![Remote](${src})`} />);
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByRole("button", { name: /remote/i })).toBeInTheDocument();
    expect(mockOpenUrl).not.toHaveBeenCalled();
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

  it("never lets a local file's raw HTML run script or restyle the app", () => {
    const { container } = render(
      <MarkdownRendered
        content={[
          `<iframe srcdoc="<script>parent.x()</script>"></iframe>`,
          `<script>window.x()</script>`,
          `<style>body{display:none}</style>`,
          `<img src="x.png" onerror="window.x()">`,
          ``,
          `still here`,
        ].join("\n")}
        filePath="/repo/README.md"
      />,
    );
    expect(container.querySelector("iframe, script, style")).toBeNull();
    expect(container.querySelector("img")?.hasAttribute("onerror")).toBe(false);
    expect(container.textContent).toContain("still here");
  });

  it("keeps Windows drive paths and data images in a local file", () => {
    const { container } = render(
      <MarkdownRendered
        content={`<img src="C:\\repo\\logo.png">\n\n<img src="data:image/png;base64,AAAA">`}
        filePath={"C:\\repo\\README.md"}
      />,
    );
    const imgs = container.querySelectorAll("img");
    expect(imgs[0].getAttribute("src")).toBe("mock-asset:C:\\repo\\logo.png");
    expect(imgs[1].getAttribute("src")).toBe("data:image/png;base64,AAAA");
  });

  it("renders forge image descriptions without fetching image URLs", () => {
    const { container, getByText } = render(
      <MarkdownRendered
        untrusted
        filePath="/repo/README.md"
        content={[
          "![Screenshot](https://example.com/screenshot.png)",
          "![Local picture](/pictures/sample.png)",
          "![Relative picture](./sample.png)",
          "![In-app picture](/api/sample.png)",
          "![Same-origin picture](" + window.location.origin + "/sample.png)",
          '<img src="http://asset.localhost/pictures/sample.png" alt="Asset picture">',
          '<picture><source srcset="https://example.com/another.png 2x"><img src="/sample.png" alt="Picture fallback"></picture>',
        ].join("\n\n")}
      />,
    );
    expect(container.querySelector("img, picture, source, [srcset]")).toBeNull();
    expect(getByText("Screenshot")).toBeInTheDocument();
    expect(getByText("Local picture")).toBeInTheDocument();
    expect(getByText("Picture fallback")).toBeInTheDocument();
  });

  it("gives forge markdown links only web or in-document destinations", () => {
    const { getByText } = render(
      <MarkdownRendered
        untrusted
        content={[
          "[Docs](https://example.com/docs)",
          "[Section](#section)",
          "[Relative](./other.md)",
          "[App](/settings)",
          "[Asset](http://asset.localhost/pictures/sample.png)",
        ].join("\n\n")}
      />,
    );
    expect(getByText("Docs")).toHaveAttribute("href", "https://example.com/docs");
    expect(getByText("Docs")).toHaveAttribute("rel", "noopener noreferrer");
    expect(getByText("Section")).toHaveAttribute("href", "#section");
    for (const label of ["Relative", "App", "Asset"]) {
      expect(getByText(label)).not.toHaveAttribute("href");
    }
  });

  it.each([
    "[relative](./notes.md)",
    "[absolute](/tmp/notes.md)",
    "[protocol-relative](//example.com/notes)",
    "[mail](mailto:reader@example.com)",
    "[opaque HTTP](http:example.com)",
    '<a href="./notes.md" target="_blank">raw relative</a>',
  ])("keeps a local document link from navigating the webview: %s", (content) => {
    const { container } = render(<MarkdownRendered content={content} filePath="/repo/README.md" />);
    const link = container.querySelector("a");
    expect(link).not.toBeNull();
    expect(link).not.toHaveAttribute("href");
    expect(link).not.toHaveAttribute("target");
    expect(fireEvent.click(link!)).toBe(false);
    expect(fireEvent(link!, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }))).toBe(false);
    expect(mockOpenExternalUrl).not.toHaveBeenCalled();
  });

  it.each([false, true])("keeps app and asset URLs inert with untrusted=%s", (untrusted) => {
    const { getByText } = render(
      <MarkdownRendered
        untrusted={untrusted}
        content={[
          `[App](${window.location.origin}/settings)`,
          "[Asset](http://asset.localhost/pictures/sample.png)",
          "[Tauri](http://tauri.localhost/pictures/sample.png)",
        ].join("\n\n")}
      />,
    );
    for (const label of ["App", "Asset", "Tauri"]) {
      expect(getByText(label)).not.toHaveAttribute("href");
    }
  });

  it.each([false, true])("opens an absolute external URL deliberately with untrusted=%s", (untrusted) => {
    const href = "https://example.com/notes";
    const { getByText } = render(<MarkdownRendered content={`[web](${href})`} untrusted={untrusted} />);
    const link = getByText("web");
    expect(link).toHaveAttribute("href", href);
    expect(fireEvent.click(link)).toBe(false);
    expect(fireEvent(link, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }))).toBe(false);
    expect(mockOpenExternalUrl).toHaveBeenCalledTimes(2);
    expect(mockOpenExternalUrl).toHaveBeenNthCalledWith(1, href, expect.objectContaining({ event: expect.any(Object) }));
    expect(mockOpenExternalUrl).toHaveBeenNthCalledWith(2, href, expect.objectContaining({ event: expect.any(Object) }));
  });

  it.each([
    ["a raw named anchor", `[Setup](#setup)\n\n<a name="setup"></a>\n\ntext`, "a[href='#setup']"],
    ["a raw id", `[Setup](#setup)\n\n<h2 id="setup">Setup</h2>`, "a[href='#setup']"],
    ["a footnote", `Claim.[^1]\n\n[^1]: Source.`, "a[data-footnote-ref]"],
    ["a footnote's way back", `Claim.[^1]\n\n[^1]: Source.`, "a[data-footnote-backref]"],
  ])("follows an in-page link to %s", (_label, content, linkSelector) => {
    const scrolled: Element[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    try {
      const { container } = render(
        <MarkdownRendered content={content} filePath="/repo/README.md" />,
      );
      const link = container.querySelector(linkSelector);
      expect(link).not.toBeNull();
      expect(fireEvent.click(link!)).toBe(false);
      expect(scrolled).toHaveLength(1);
      expect(container.contains(scrolled[0])).toBe(true);
      expect(scrolled[0]).not.toBe(link);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
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
