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
    <TooltipProvider>
      <PaneTabStrip
        tabs={TABS}
        activeTab="files"
        onSelect={onSelect}
        onClose={onClose}
        onCloseMany={() => {}}
        onReorder={onReorder}
        surfaces={[]}
        onOpenFile={() => {}}
        openFileKeys=""
        inTitlebar
        onToggleExpand={() => {}}
        expanded={false}
        onCollapsePanel={() => {}}
        {...overrides}
      />
    </TooltipProvider>,
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
 * scroller, the tab content and the inline pane actions, `clientWidth` for
 * the scroller and header.
 */
function stubLayout(widths: {
  header: number;
  tabs: number;
  scroller: number;
  actions?: number;
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
      if (id === "right-panel-pane-actions") return widths.actions ?? 0;
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

  // A badge can be a count plus a status icon (Review); the two sit in one
  // row instead of stacking out of the 26px chip.
  it("lays a badge's count and icon out in a row", () => {
    renderStrip();
    expect(screen.getByText("4")).toHaveClass("inline-flex", "items-center");
  });

  // At the default panel width the top-right reserve leaves the row about
  // 160px. The tabs keep their floor and the actions fold into "…" instead
  // of being squeezed or clipped out of reach.
  describe("pane actions in a narrow row", () => {
    const onRefresh = vi.fn();
    const actions = (
      <>
        <button type="button" onClick={onRefresh}>
          Refresh
        </button>
        <button type="button">Filter</button>
      </>
    );

    it("keeps the actions inline while they fit beside the tabs", () => {
      const restore = stubLayout({
        header: 2000,
        tabs: 300,
        scroller: 300,
        actions: 50,
      });
      try {
        renderStrip({ actions });
        expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
          "RefreshFilter",
        );
        expect(screen.queryByLabelText("Pane actions")).toBeNull();
        expect(
          screen.getByTestId("right-panel-tabs-header"),
        ).toContainElement(screen.getByLabelText("Open pane"));
      } finally {
        restore();
      }
    });

    it("folds them and the + into one button whose popover holds the same controls", async () => {
      const restore = stubLayout({
        header: 300,
        tabs: 300,
        scroller: 100,
        actions: 120,
      });
      try {
        renderStrip({ actions });
        expect(screen.queryByTestId("right-panel-pane-actions")).toBeNull();
        expect(screen.queryByText("Refresh")).toBeNull();
        // The "+" leaves the row too, so the tabs keep its width.
        expect(screen.queryByLabelText("Open pane")).toBeNull();

        fireEvent.click(screen.getByLabelText("Pane actions"));
        const popover = await screen.findByTestId(
          "right-panel-pane-actions-popover",
        );
        expect(popover).toHaveTextContent("RefreshFilter");
        // First in the row, ahead of the pane's own actions.
        expect(popover.querySelector("button")).toHaveAttribute(
          "aria-label",
          "Open pane",
        );
        fireEvent.click(screen.getByText("Refresh"));
        expect(onRefresh).toHaveBeenCalledTimes(1);
      } finally {
        restore();
      }
    });

    it("unfolds them once the row has room again, and holds the tabs' floor", () => {
      const widths = { header: 300, tabs: 300, scroller: 100, actions: 120 };
      const restore = stubLayout(widths);
      try {
        const view = renderStrip({ actions });
        expect(screen.getByLabelText("Pane actions")).toBeInTheDocument();
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 44px))",
        );

        widths.header = 2000;
        view.rerender(
          <TooltipProvider>
            <PaneTabStrip
              tabs={TABS}
              activeTab="files"
              onSelect={() => {}}
              onClose={() => {}}
              onCloseMany={() => {}}
              onReorder={() => {}}
              surfaces={[]}
              onOpenFile={() => {}}
              openFileKeys=""
              inTitlebar
              onToggleExpand={() => {}}
              expanded={false}
              onCollapsePanel={() => {}}
              actions={actions}
            />
          </TooltipProvider>,
        );
        expect(screen.queryByLabelText("Pane actions")).toBeNull();
        expect(screen.getByTestId("right-panel-pane-actions")).toBeInTheDocument();
        expect(screen.getByLabelText("Open pane")).toBeInTheDocument();
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 49px))",
        );
      } finally {
        restore();
      }
    });

    const stripWith = (paneActions: React.ReactNode) => (
      <TooltipProvider>
        <PaneTabStrip
          tabs={TABS}
          activeTab="files"
          onSelect={() => {}}
          onClose={() => {}}
          onCloseMany={() => {}}
          onReorder={() => {}}
          surfaces={[]}
          onOpenFile={() => {}}
          openFileKeys=""
          inTitlebar
          onToggleExpand={() => {}}
          expanded={false}
          onCollapsePanel={() => {}}
          actions={paneActions}
        />
      </TooltipProvider>
    );
    const oneAction = <button type="button">Split view</button>;

    // A toggle can swap a pane's buttons without a pane switch (the Diff
    // layout); a folded row must not keep deciding on the old width.
    it("measures folded actions again when they change in place", () => {
      const widths = { header: 600, tabs: 300, scroller: 100, actions: 200 };
      const restore = stubLayout(widths);
      try {
        const view = render(stripWith(actions));
        expect(screen.getByLabelText("Pane actions")).toBeInTheDocument();

        widths.actions = 50;
        view.rerender(stripWith(oneAction));
        expect(screen.queryByLabelText("Pane actions")).toBeNull();
        expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
          "Split view",
        );
      } finally {
        restore();
      }
    });

    it("waits for the popover to close before measuring a change made inside it", async () => {
      const widths = { header: 600, tabs: 300, scroller: 100, actions: 200 };
      const restore = stubLayout(widths);
      try {
        const view = render(stripWith(actions));
        fireEvent.click(screen.getByLabelText("Pane actions"));
        await screen.findByTestId("right-panel-pane-actions-popover");

        widths.actions = 50;
        view.rerender(stripWith(oneAction));
        const popover = screen.getByTestId("right-panel-pane-actions-popover");
        expect(popover).toHaveAccessibleName("Pane actions");
        expect(popover).toHaveTextContent("Split view");

        fireEvent.keyDown(popover, { key: "Escape" });
        expect(screen.queryByTestId("right-panel-pane-actions-more")).toBeNull();
        expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
          "Split view",
        );
      } finally {
        restore();
      }
    });

    // The interface-size setting rescales rem: the drag gap's floor and the
    // "…" grow with it, and the budget has to as well.
    it("budgets the row in the current root font size", () => {
      const restore = stubLayout({
        header: 300,
        tabs: 300,
        scroller: 100,
        actions: 120,
      });
      document.documentElement.style.fontSize = "20px";
      try {
        renderStrip({ actions });
        expect(screen.getByLabelText("Pane actions")).toBeInTheDocument();
        // 20px drag gap + 30px "…" + two 2px gaps.
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 54px))",
        );
      } finally {
        document.documentElement.style.fontSize = "";
        restore();
      }
    });
  });

  it("names the band as the container narrow pane actions query", () => {
    renderStrip();
    expect(screen.getByTestId("right-panel-tabs-header")).toHaveClass(
      "@container/pane-band",
    );
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

  // The row used to drop its tabs into a second row whenever they didn't all
  // fit beside the titlebar cluster, so the header jumped mid-drag. Narrow
  // or not, it stays one row and the tabs scroll; a narrow row folds the
  // pane actions and the `+` behind "…" in that same row.
  it.each([
    ["narrow", true, 360, true],
    ["wide", true, 900, false],
    ["legacy chrome", false, 200, true],
  ])("keeps one row in a %s panel", (_name, inTitlebar, header, folded) => {
    const restore = stubLayout({ header, tabs: 300, scroller: 100 });
    try {
      renderStrip({ inTitlebar, actions: <button type="button">act</button> });
      const row = screen.getByTestId("right-panel-tabs-header");
      expect(row).not.toHaveAttribute("data-stacked");
      const shared = folded
        ? [screen.getByLabelText("Pane actions")]
        : [screen.getByText("act"), screen.getByTestId("right-panel-add-pane")];
      for (const node of [
        screen.getByTestId("right-panel-tabs-scroll"),
        ...shared,
      ]) {
        expect(row).toContainElement(node);
      }
    } finally {
      restore();
    }
  });

  it("puts the close affordance at the start of the tab, before the label", () => {
    renderStrip();
    const close = screen.getByLabelText("Close Changes");
    const label = screen.getByText("Changes");
    expect(
      close.compareDocumentPosition(label) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });
});
