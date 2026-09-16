// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { FileText, FolderTree, GitBranch } from "lucide-react";

import { TooltipProvider } from "@/components/ui/tooltip";

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

/**
 * jsdom has no layout. Report widths per test id: `scrollWidth` for the
 * scroller and the tab content, `clientWidth` for the scroller and header.
 */
function stubLayout(widths: {
  header: number;
  tabs: number;
  scroller: number;
}) {
  const testId = (el: HTMLElement) => el.dataset?.testid;
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
      const id = testId(this);
      return id === "right-panel-tabs-scroll" ||
        id === "right-panel-tabs-content"
        ? widths.tabs
        : 0;
    },
  });
  Object.defineProperty(proto, "clientWidth", {
    configurable: true,
    get() {
      const id = testId(this);
      if (id === "right-panel-tabs-scroll") return widths.scroller;
      if (id === "right-panel-tabs-header") return widths.header;
      return 0;
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
    expect(screen.getByTestId("right-panel-tabs-header")).not.toHaveAttribute(
      "data-stacked",
    );
  });

  // A count the user has to deal with reads the same whether or not its
  // pane is in front — otherwise a failure dims itself the moment you look
  // at another pane.
  it("paints an attention badge on the attention token, active or not", () => {
    renderStrip({
      tabs: TABS.map((tab) =>
        tab.id === "changes"
          ? { ...tab, badge: 2, badgeTone: "attention" as const }
          : tab,
      ),
    });
    expect(screen.getByText("2")).toHaveClass("text-status-attention");
  });

  it("keeps every tab full-size and fades the clipped edge on overflow", () => {
    const restore = stubLayout({ header: 2000, tabs: 800, scroller: 400 });
    try {
      renderStrip();
      const scroller = screen.getByTestId("right-panel-tabs-scroll");
      // Inactive tabs keep their labels and close affordances.
      expect(screen.getByText("Changes")).toBeInTheDocument();
      expect(screen.getByLabelText("Close Changes")).toBeInTheDocument();
      expect(screen.getByText("4")).toBeInTheDocument();
      // Content is clipped on the right, so that edge fades.
      expect(scroller.style.maskImage).toMatch(/transparent\)$/);
      expect(scroller.style.maskImage).toMatch(
        /^linear-gradient\(to right, black,/,
      );
    } finally {
      restore();
    }
  });

  it("stacks the tabs below the band when they can't fit beside it", () => {
    // A 360px panel: 174px of the band belongs to the fixed cluster and the
    // window buttons, which leaves far less than 300px of tabs need.
    const restore = stubLayout({ header: 360, tabs: 300, scroller: 300 });
    try {
      renderStrip({ actions: <button type="button">act</button> });
      const header = screen.getByTestId("right-panel-tabs-header");
      expect(header).toHaveAttribute("data-stacked", "true");
      // The tabs, the pane actions and the `+` are all still there.
      expect(screen.getByText("Files")).toBeInTheDocument();
      expect(screen.getByText("act")).toBeInTheDocument();
      expect(screen.getByTestId("right-panel-add-pane")).toBeInTheDocument();
    } finally {
      restore();
    }
  });

  it("keeps one row in a wide panel", () => {
    const restore = stubLayout({ header: 900, tabs: 300, scroller: 300 });
    try {
      renderStrip({ actions: <button type="button">act</button> });
      expect(screen.getByTestId("right-panel-tabs-header")).not.toHaveAttribute(
        "data-stacked",
      );
    } finally {
      restore();
    }
  });

  it("never stacks outside the titlebar band", () => {
    const restore = stubLayout({ header: 200, tabs: 300, scroller: 100 });
    try {
      render(
        <TooltipProvider>
          <PaneTabStrip
            tabs={TABS}
            activeTab="files"
            onSelect={() => {}}
            onClose={() => {}}
            onReorder={() => {}}
            surfaces={[]}
            onOpenFile={() => {}}
            openFileKeys=""
            inTitlebar={false}
            onToggleExpand={() => {}}
            expanded={false}
            onCollapsePanel={() => {}}
          />
        </TooltipProvider>,
      );
      expect(screen.getByTestId("right-panel-tabs-header")).not.toHaveAttribute(
        "data-stacked",
      );
    } finally {
      restore();
    }
  });
});
