// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { FileText, FolderTree, GitBranch } from "lucide-react";

import { PaneTabStrip, type DeckTab } from "./pane-tab-strip";

const TABS: DeckTab[] = [
  { id: "files", label: "Files", icon: FolderTree, testId: "files-tab" },
  {
    id: "changes",
    label: "Changes",
    icon: GitBranch,
    testId: "changes-tab",
    badge: 4,
  },
  {
    id: "doc:/p/README.md",
    label: "README.md",
    icon: FileText,
    testId: "doc-tab",
  },
];

function renderStrip(
  overrides: Partial<React.ComponentProps<typeof PaneTabStrip>> = {},
) {
  const onSelect = vi.fn();
  const onClose = vi.fn();
  const onReorder = vi.fn();
  const utils = render(
    <PaneTabStrip
      tabs={TABS}
      activeTab="files"
      onSelect={onSelect}
      onClose={onClose}
      onReorder={onReorder}
      surfaces={[]}
      onOpenFile={() => {}}
      openFileKeys=""
      inTitlebar
      onToggleExpand={() => {}}
      expanded={false}
      onCollapsePanel={() => {}}
      {...overrides}
    />,
  );
  const chip = (id: string) =>
    utils.container.querySelector(
      `[data-tab-id="${CSS.escape(id)}"]`,
    ) as HTMLElement;
  // Lay the three chips out left-to-right in 100px slots so the drop index
  // has something to measure against.
  TABS.forEach((tab, i) => stubRect(chip(tab.id), i * 100, 100));
  return { ...utils, onSelect, onClose, onReorder, chip };
}

function stubRect(el: HTMLElement, left: number, width: number) {
  Object.defineProperty(el, "getBoundingClientRect", {
    value: () =>
      ({
        left,
        right: left + width,
        width,
        top: 0,
        bottom: 26,
        height: 26,
        x: left,
        y: 0,
        toJSON() {},
      }) as DOMRect,
    configurable: true,
  });
}

/** jsdom has no layout; make the strip report an overflowing one. */
function stubScrollerOverflow(scrollWidth: number, clientWidth: number) {
  const isScroller = (el: HTMLElement) =>
    el.dataset?.testid === "right-panel-tabs-scroll";
  const proto = HTMLElement.prototype;
  const original = {
    scrollWidth: Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollWidth",
    )!,
    clientWidth: Object.getOwnPropertyDescriptor(
      Element.prototype,
      "clientWidth",
    )!,
  };
  Object.defineProperty(proto, "scrollWidth", {
    configurable: true,
    get() {
      return isScroller(this) ? scrollWidth : 0;
    },
  });
  Object.defineProperty(proto, "clientWidth", {
    configurable: true,
    get() {
      return isScroller(this) ? clientWidth : 0;
    },
  });
  return () => {
    delete (proto as unknown as Record<string, unknown>).scrollWidth;
    delete (proto as unknown as Record<string, unknown>).clientWidth;
    Object.defineProperty(
      Element.prototype,
      "scrollWidth",
      original.scrollWidth,
    );
    Object.defineProperty(
      Element.prototype,
      "clientWidth",
      original.clientWidth,
    );
  };
}

afterEach(cleanup);

describe("PaneTabStrip — drag-to-reorder", () => {
  it("selects on a plain click and never reorders", () => {
    const { chip, onSelect, onReorder } = renderStrip();
    fireEvent.pointerDown(chip("changes"), {
      pointerId: 1,
      button: 0,
      clientX: 150,
      clientY: 14,
    });
    fireEvent.pointerUp(document, {
      pointerId: 1,
      button: 0,
      clientX: 150,
      clientY: 14,
    });
    fireEvent.click(screen.getByText("Changes"));
    expect(onSelect).toHaveBeenCalledWith("changes");
    expect(onReorder).not.toHaveBeenCalled();
  });

  it("reports the new order once the drag threshold is crossed and dropped", () => {
    const { chip, onReorder } = renderStrip();
    fireEvent.pointerDown(chip("files"), {
      pointerId: 1,
      button: 0,
      clientX: 50,
      clientY: 14,
    });
    // Past the 5px threshold and past the last chip's midpoint (250), so
    // the tab lands at the end of the strip.
    fireEvent.pointerMove(document, {
      pointerId: 1,
      clientX: 260,
      clientY: 14,
    });
    expect(screen.getByTestId("tab-drop-indicator")).toBeInTheDocument();
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });
    expect(onReorder).toHaveBeenCalledWith([
      "changes",
      "doc:/p/README.md",
      "files",
    ]);
  });

  it("swallows the click a real drag leaves behind", () => {
    const { chip, onSelect } = renderStrip();
    fireEvent.pointerDown(chip("changes"), {
      pointerId: 1,
      button: 0,
      clientX: 150,
      clientY: 14,
    });
    fireEvent.pointerMove(document, {
      pointerId: 1,
      clientX: 260,
      clientY: 14,
    });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.click(screen.getByText("Changes"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("never starts a drag from the close button, which still closes", () => {
    const { onClose, onReorder } = renderStrip();
    const close = screen.getByLabelText("Close Files");
    fireEvent.pointerDown(close, {
      pointerId: 1,
      button: 0,
      clientX: 90,
      clientY: 14,
    });
    fireEvent.pointerMove(document, {
      pointerId: 1,
      clientX: 260,
      clientY: 14,
    });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.click(close);
    expect(onReorder).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledWith("files");
  });

  it("closes on middle-click", () => {
    const { chip, onClose } = renderStrip();
    chip("changes").dispatchEvent(
      new MouseEvent("auxclick", {
        button: 1,
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(onClose).toHaveBeenCalledWith("changes");
  });
});

describe("PaneTabStrip — overflow", () => {
  it("pans with a vertical wheel once the tabs overflow", () => {
    renderStrip();
    const scroller = screen.getByTestId("right-panel-tabs-scroll");
    Object.defineProperty(scroller, "scrollWidth", {
      value: 800,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientWidth", {
      value: 400,
      configurable: true,
    });
    Object.defineProperty(scroller, "scrollLeft", {
      value: 0,
      writable: true,
      configurable: true,
    });
    fireEvent.wheel(scroller, { deltaY: 120 });
    expect(scroller.scrollLeft).toBe(120);
  });

  it("keeps every label while the row fits", () => {
    renderStrip();
    expect(screen.getByText("Files")).toBeInTheDocument();
    expect(screen.getByText("Changes")).toBeInTheDocument();
    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.getByTestId("right-panel-tabs-scroll")).not.toHaveAttribute(
      "data-compact",
    );
  });

  it("collapses inactive tabs to icon + badge once the full row overflows", () => {
    const restore = stubScrollerOverflow(800, 400);
    try {
      renderStrip();
      const scroller = screen.getByTestId("right-panel-tabs-scroll");
      expect(scroller).toHaveAttribute("data-compact", "true");
      // The active tab keeps its label and its close button…
      expect(screen.getByText("Files")).toBeInTheDocument();
      expect(screen.getByLabelText("Close Files")).toBeInTheDocument();
      // …the others drop to icon + badge, with the name on the button and
      // no overlaid close affordance to mis-hit.
      expect(screen.queryByText("Changes")).toBeNull();
      expect(screen.getByLabelText("Changes")).toBeInTheDocument();
      expect(screen.getByText("4")).toBeInTheDocument();
      expect(screen.queryByLabelText("Close Changes")).toBeNull();
      // Content is clipped on the right, so that edge fades.
      expect(scroller.style.maskImage).toMatch(/transparent\)$/);
      expect(scroller.style.maskImage).toMatch(
        /^linear-gradient\(to right, black,/,
      );
    } finally {
      restore();
    }
  });

  it("keeps the compact strip's scroll offset when another tab is activated", () => {
    const restore = stubScrollerOverflow(800, 400);
    try {
      const { rerender } = renderStrip();
      const scroller = screen.getByTestId("right-panel-tabs-scroll");
      expect(scroller).toHaveAttribute("data-compact", "true");
      // The user has panned to the right…
      Object.defineProperty(scroller, "scrollLeft", {
        value: 300,
        writable: true,
        configurable: true,
      });
      // …and activating a tab re-measures the strip (full layout, then
      // compact again). That bounce must not throw the offset away.
      rerender(
        <PaneTabStrip
          tabs={TABS}
          activeTab="changes"
          onSelect={() => {}}
          onClose={() => {}}
          onReorder={() => {}}
          surfaces={[]}
          onOpenFile={() => {}}
          openFileKeys=""
          inTitlebar
          onToggleExpand={() => {}}
          expanded={false}
          onCollapsePanel={() => {}}
        />,
      );
      expect(scroller).toHaveAttribute("data-compact", "true");
      expect(scroller.scrollLeft).toBe(300);
    } finally {
      restore();
    }
  });
});
