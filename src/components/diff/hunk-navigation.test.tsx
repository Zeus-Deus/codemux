import { createRef } from "react";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DiffLine } from "@/lib/diff-parser";
import { DiffSplitView, type DiffViewHandle } from "./DiffSplitView";
import { DiffUnifiedView } from "./DiffUnifiedView";

function line(type: DiffLine["type"], content: string): DiffLine {
  return { type, content, oldLine: 1, newLine: 1 };
}

// Three hunks with two context lines between them.
const LINES: DiffLine[] = [
  line("hunk-header", "@@ -1,2 +1,2 @@"),
  line("context", "one"),
  line("context", "two"),
  line("hunk-header", "@@ -40,2 +40,2 @@"),
  line("context", "three"),
  line("context", "four"),
  line("hunk-header", "@@ -80,2 +80,2 @@"),
  line("context", "five"),
];

/** jsdom has no layout, so stand in for it: every hunk sits 100px apart in
 * container coordinates, which is what the `relative` scroll container makes
 * `offsetTop` mean. */
function stubLayout(container: HTMLElement): void {
  container.querySelectorAll<HTMLElement>("[data-diff-hunk]").forEach((hunk, index) => {
    Object.defineProperty(hunk, "offsetTop", { configurable: true, value: index * 100 });
  });
}

describe("diff hunk navigation", () => {
  afterEach(cleanup);

  it("scrolls the unified container to the next and previous hunk", () => {
    const ref = createRef<DiffViewHandle>();
    const { container } = render(<DiffUnifiedView ref={ref} lines={LINES} />);
    const scroller = container.querySelector<HTMLElement>(".code-surface")!;

    // Positioned on purpose: it makes each hunk's offsetTop a scroll offset
    // rather than a coordinate measured from the app shell above the toolbar.
    expect(scroller.className).toContain("relative");

    stubLayout(scroller);
    const scrollTo = vi.fn();
    scroller.scrollTo = scrollTo as unknown as HTMLElement["scrollTo"];

    ref.current!.scrollToHunk(1);
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 100, behavior: "smooth" });

    scroller.scrollTop = 200;
    ref.current!.scrollToHunk(-1);
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 100, behavior: "smooth" });
  });

  it("scrolls both split panes to the same hunk offset", () => {
    const ref = createRef<DiffViewHandle>();
    const { container } = render(<DiffSplitView ref={ref} lines={LINES} />);
    const panes = [...container.querySelectorAll<HTMLElement>(".overflow-auto")];
    expect(panes).toHaveLength(2);

    const scrollTos = panes.map((pane) => {
      expect(pane.className).toContain("relative");
      stubLayout(pane);
      const scrollTo = vi.fn();
      pane.scrollTo = scrollTo as unknown as HTMLElement["scrollTo"];
      return scrollTo;
    });

    ref.current!.scrollToHunk(1);

    for (const scrollTo of scrollTos) {
      expect(scrollTo).toHaveBeenCalledWith({ top: 100, behavior: "smooth" });
    }
  });
});
