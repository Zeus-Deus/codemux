// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { FileText, FolderTree, GitBranch } from "lucide-react";
import userEvent from "@testing-library/user-event";

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

/** Enough extra tabs that the run overflows even with every tab icon-only. */
const EXTRA_TABS: DeckTab[] = Array.from({ length: 20 }, (_, i) => ({
  id: `doc:/p/extra-${i}.md`,
  label: `extra-${i}.md`,
  icon: FileText,
}));

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

  // Labels that scroll away under the top-right cluster are as good as
  // missing, so a row too narrow for every label drops the inactive ones
  // and keeps the active tab's.
  it("drops inactive labels once the labelled tabs outgrow the row", () => {
    // The header's right padding is the titlebar reserve, so 600px leaves
    // the tabs about 350.
    const restore = stubLayout({ header: 600, tabs: 700, scroller: 100 });
    try {
      const { onSelect } = renderStrip();
      expect(screen.getByText("Files")).toBeInTheDocument();
      expect(screen.queryByText("Changes")).toBeNull();
      const changes = screen.getByRole("button", { name: "Changes", pressed: false });
      expect(changes.closest("[data-compact]")).toHaveAttribute("data-state", "inactive");
      // The icon selects; an icon-only tab has no close button to hit.
      expect(screen.queryByLabelText("Close Changes")).toBeNull();
      fireEvent.click(changes);
      expect(onSelect).toHaveBeenCalledWith("changes");
    } finally {
      restore();
    }
  });

  // Rather than truncate the active label to a stub, a row with no room for
  // it shows the active tab icon-only too: still filled, still closable,
  // still named.
  it("goes icon-only on the active tab too when its label cannot fit", () => {
    // Narrower than the titlebar reserve leaves the active tab no room.
    const restore = stubLayout({ header: 200, tabs: 700, scroller: 100 });
    try {
      const { onClose } = renderStrip();
      const active = screen.getByTestId("files-tab");
      // No visible label, just the icon (the name is screen-reader text).
      expect(active.querySelector(".truncate")).toBeNull();
      expect(active).toHaveAttribute("data-state", "active");
      expect(active).toHaveAttribute("data-fit", "icon");
      expect(active).toHaveTextContent("Files (current tab)");
      fireEvent.click(screen.getByLabelText("Close Files"));
      expect(onClose).toHaveBeenCalledWith("files");
    } finally {
      restore();
    }
  });

  // Hiding labels is what lets folded pane actions come back inline, and
  // labels coming back is what folds them again. Each hook must budget for
  // the other's next layout, or the two flip together forever.
  it("settles instead of alternating labels and folded actions", () => {
    const proto = HTMLElement.prototype;
    const original = {
      scrollWidth: Object.getOwnPropertyDescriptor(Element.prototype, "scrollWidth")!,
      clientWidth: Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!,
      offsetWidth: Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetWidth")!,
    };
    let header = 0;
    // The row's other children, as the tab fit measures them.
    Object.defineProperty(proto, "offsetWidth", {
      configurable: true,
      get() {
        const id = (this as HTMLElement).dataset?.testid;
        if (id === "right-panel-pane-actions") return 110;
        if (
          id === "right-panel-add-pane" ||
          id === "right-panel-pane-actions-more" ||
          id === "right-panel-tabs-overflow"
        ) {
          return 24;
        }
        return 0;
      },
    });
    Object.defineProperty(proto, "scrollWidth", {
      configurable: true,
      get() {
        const id = (this as HTMLElement).dataset?.testid;
        if (id === "right-panel-pane-actions") return 110;
        if (id === "right-panel-tabs-scroll" || id === "right-panel-tabs-content") {
          // Labelled, the run is 260px. Compact, it depends on how the
          // active tab is fitted too: 172px with its count, 132px without.
          if (!document.querySelector("[data-compact]")) return 260;
          return document.querySelector('[data-state="active"][data-fit]') ? 132 : 172;
        }
        return 0;
      },
    });
    Object.defineProperty(proto, "clientWidth", {
      configurable: true,
      get() {
        const id = (this as HTMLElement).dataset?.testid;
        if (id === "right-panel-tabs-header") return header;
        if (id === "right-panel-tabs-scroll") return 100;
        return 0;
      },
    });
    // The active chip measures 120px with its count, 80px without.
    const rect = Object.getOwnPropertyDescriptor(Element.prototype, "getBoundingClientRect")!;
    Object.defineProperty(Element.prototype, "getBoundingClientRect", {
      configurable: true,
      value(this: Element) {
        const el = this as HTMLElement;
        const width = el.hasAttribute("data-tab-badge")
          ? 33
          : el.getAttribute("data-state") === "active" && el.hasAttribute("data-tab-id")
            ? el.querySelector("[data-tab-badge]") ? 120 : 80
            : 0;
        return { left: 0, right: width, width, top: 0, bottom: 26, height: 26, x: 0, y: 0, toJSON() {} } as DOMRect;
      },
    });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      for (header = 400; header <= 800; header += 5) {
        for (const activeTab of ["files", "changes"] as const) {
          const { unmount } = renderStrip({
            activeTab,
            actions: <button type="button" aria-label="Refresh">R</button>,
          });
          unmount();
        }
      }
      const loops = errors.mock.calls.filter((call) =>
        String(call[0]).includes("Maximum update depth"),
      );
      expect(loops).toEqual([]);
    } finally {
      Object.defineProperty(Element.prototype, "getBoundingClientRect", rect);
      errors.mockRestore();
      delete (proto as unknown as Record<string, unknown>).scrollWidth;
      delete (proto as unknown as Record<string, unknown>).clientWidth;
      Object.defineProperty(Element.prototype, "scrollWidth", original.scrollWidth);
      Object.defineProperty(Element.prototype, "clientWidth", original.clientWidth);
      Object.defineProperty(HTMLElement.prototype, "offsetWidth", original.offsetWidth);
    }
  });

  // A count can grow in place (Changes, Tasks) without a new tab set or a
  // pane switch; a compact row must fit the active tab against its current
  // width, not the one it measured when the labels were last shown.
  it("refits the active tab when its count grows in place", () => {
    const restore = stubLayout({ header: 600, tabs: 700, scroller: 100 });
    const rect = Object.getOwnPropertyDescriptor(Element.prototype, "getBoundingClientRect")!;
    let badgeWidth = 10;
    Object.defineProperty(Element.prototype, "getBoundingClientRect", {
      configurable: true,
      value(this: Element) {
        const el = this as HTMLElement;
        const width = el.hasAttribute("data-tab-badge")
          ? badgeWidth
          : el.getAttribute("data-state") === "active" && el.hasAttribute("data-tab-id")
            ? 80 + badgeWidth
            : 0;
        return { left: 0, right: width, width, top: 0, bottom: 26, height: 26, x: 0, y: 0, toJSON() {} } as DOMRect;
      },
    });
    try {
      // A fresh element each time, so the rerender actually commits.
      const strip = () => (
        <TooltipProvider>
          <PaneTabStrip
            tabs={TABS}
            activeTab="changes"
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
          />
        </TooltipProvider>
      );
      const { rerender } = render(strip());
      const active = () => screen.getByTestId("changes-tab");
      expect(active()).not.toHaveAttribute("data-fit");
      // Same tabs, same badge prop: only its rendered width grew.
      badgeWidth = 400;
      rerender(strip());
      expect(active()).toHaveAttribute("data-fit");
    } finally {
      Object.defineProperty(Element.prototype, "getBoundingClientRect", rect);
      restore();
    }
  });

  // A component badge (Review's PR status) is sized by what its props point
  // at. Hidden by the fit, it can't be measured, so a different PR or
  // workspace has to count as a new badge and be measured afresh.
  it("re-measures a hidden component badge when its props change", () => {
    const restore = stubLayout({ header: 600, tabs: 700, scroller: 100 });
    const rect = Object.getOwnPropertyDescriptor(Element.prototype, "getBoundingClientRect")!;
    const widths: Record<string, number> = { "1": 400, "2": 10 };
    Object.defineProperty(Element.prototype, "getBoundingClientRect", {
      configurable: true,
      value(this: Element) {
        const el = this as HTMLElement;
        const badge = el.querySelector?.("[data-pr]") as HTMLElement | null;
        const own = el.hasAttribute("data-tab-badge")
          ? widths[(el.querySelector("[data-pr]") as HTMLElement | null)?.dataset.pr ?? ""] ?? 0
          : el.getAttribute("data-state") === "active" && el.hasAttribute("data-tab-id")
            ? 80 + (badge ? widths[badge.dataset.pr ?? ""] ?? 0 : 0)
            : 0;
        return { left: 0, right: own, width: own, top: 0, bottom: 26, height: 26, x: 0, y: 0, toJSON() {} } as DOMRect;
      },
    });
    function PrBadge({ prNumber }: { prNumber: number }) {
      return <span data-pr={String(prNumber)}>#{prNumber}</span>;
    }
    const strip = (prNumber: number) => (
      <TooltipProvider>
        <PaneTabStrip
          tabs={TABS.map((tab) =>
            tab.id === "changes" ? { ...tab, badge: <PrBadge prNumber={prNumber} /> } : tab,
          )}
          activeTab="changes"
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
        />
      </TooltipProvider>
    );
    try {
      const { rerender } = render(strip(1));
      const active = () => screen.getByTestId("changes-tab");
      // PR 1's badge is too wide: the fit hides it.
      expect(active()).toHaveAttribute("data-fit");
      expect(active().querySelector("[data-pr]")).toBeNull();
      // Another PR, same tab set: its narrower badge is measured and shown.
      rerender(strip(2));
      expect(active()).not.toHaveAttribute("data-fit");
      expect(active().querySelector('[data-pr="2"]')).not.toBeNull();
    } finally {
      Object.defineProperty(Element.prototype, "getBoundingClientRect", rect);
      restore();
    }
  });

  it("brings the labels back once the row has room for them", () => {
    const widths = { header: 600, tabs: 700, scroller: 100 };
    const restore = stubLayout(widths);
    try {
      const { rerender } = renderStrip();
      expect(screen.queryByText("Changes")).toBeNull();
      // Wide enough for the labels plus the hysteresis: the next layout
      // pass shows them again.
      widths.header = 1200;
      rerender(
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
          />
        </TooltipProvider>,
      );
      expect(screen.getByText("Changes")).toBeInTheDocument();
    } finally {
      restore();
    }
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
        // A compact row: the "…" and its gaps, no drag-gap floor.
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 28px))",
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
        // 30px "…" + two 2px gaps; a compact row reserves no drag gap.
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 34px))",
        );
      } finally {
        document.documentElement.style.fontSize = "";
        restore();
      }
    });
  });

  // Panning alone left the tabs behind the fade hard to find: at the
  // default width the row shows about one and a half. While any are out of
  // view the row lists them all, so every open tab is one pick away.
  describe("tab list while tabs are out of view", () => {
    const actions = <button type="button">Refresh</button>;
    const listRow = (list: HTMLElement, label: string) =>
      within(list).getByText(label).closest("button") as HTMLButtonElement;

    it("lists every open tab in the folded \u2026 and selects a hidden one", async () => {
      const restore = stubLayout({
        header: 300,
        tabs: 300,
        scroller: 100,
        actions: 120,
      });
      try {
        const { onSelect } = renderStrip({ actions });
        // Too narrow for every label: the hidden tab is icon-only in the
        // strip, still named for assistive tech.
        expect(
          screen.getByRole("button", { name: "README.md", pressed: false }),
        ).toBeInTheDocument();

        fireEvent.click(screen.getByLabelText("Pane actions"));
        const popover = await screen.findByTestId(
          "right-panel-pane-actions-popover",
        );
        const list = within(popover).getByRole("group", { name: "Open tabs" });
        expect(
          within(list)
            .getAllByRole("button")
            .map((row) => row.textContent),
        ).toEqual(["Files", "Changes4", "README.md"]);
        expect(listRow(list, "Files")).toHaveAttribute("aria-current", "true");
        // The pane actions are still there, ahead of the list.
        expect(within(popover).getByText("Refresh")).toBeInTheDocument();

        fireEvent.click(listRow(list, "README.md"));
        expect(onSelect).toHaveBeenCalledWith("doc:/p/README.md");
        expect(
          screen.queryByTestId("right-panel-pane-actions-popover"),
        ).toBeNull();
      } finally {
        restore();
      }
    });

    it("leaves the list out of the \u2026 while every tab is in view", async () => {
      const restore = stubLayout({
        header: 300,
        tabs: 100,
        scroller: 100,
        actions: 400,
      });
      try {
        renderStrip({ actions });
        fireEvent.click(screen.getByLabelText("Pane actions"));
        const popover = await screen.findByTestId(
          "right-panel-pane-actions-popover",
        );
        expect(within(popover).queryByRole("group")).toBeNull();
      } finally {
        restore();
      }
    });

    it("adds an All tabs button beside inline actions, budgeted in the row", async () => {
      const restore = stubLayout({
        header: 600,
        tabs: 2500,
        scroller: 400,
        actions: 50,
      });
      try {
        const { onSelect } = renderStrip({
          actions,
          tabs: [...EXTRA_TABS, ...TABS],
        });
        // The actions stay inline; only the tabs needed help.
        expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
          "Refresh",
        );
        expect(screen.queryByLabelText("Pane actions")).toBeNull();
        // Too many tabs even icon-only: the "+", the row gaps and the 24px
        // button + its gap (a compact row reserves no drag gap).
        expect(screen.getByTestId("right-panel-tabs-scroll").style.minWidth).toBe(
          "min(140px, calc(100% - 59px))",
        );

        // A keyboard open lands on the active tab; arrows walk the list.
        fireEvent.click(screen.getByLabelText("All tabs"));
        const popover = await screen.findByTestId(
          "right-panel-tabs-overflow-popover",
        );
        const list = within(popover).getByRole("group", { name: "Open tabs" });
        expect(listRow(list, "Files")).toHaveFocus();
        fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
        expect(listRow(list, "Changes")).toHaveFocus();
        fireEvent.keyDown(document.activeElement!, { key: "End" });
        expect(listRow(list, "README.md")).toHaveFocus();

        fireEvent.click(listRow(list, "Changes"));
        expect(onSelect).toHaveBeenCalledWith("changes");
      } finally {
        restore();
      }
    });

    it("starts keyboard navigation from the active tab when it is not first", async () => {
      const restore = stubLayout({
        header: 600,
        tabs: 2500,
        scroller: 400,
        actions: 50,
      });
      try {
        const user = userEvent.setup();
        const { onSelect } = renderStrip({
          actions,
          tabs: [...EXTRA_TABS, ...TABS],
          activeTab: "doc:/p/README.md",
        });
        screen.getByLabelText("All tabs").focus();
        await user.keyboard("{Enter}");
        const popover = await screen.findByTestId(
          "right-panel-tabs-overflow-popover",
        );
        const list = within(popover).getByRole("group", { name: "Open tabs" });
        expect(listRow(list, "README.md")).toHaveFocus();
        await user.keyboard("{ArrowUp}");
        expect(listRow(list, "Changes")).toHaveFocus();
        await user.keyboard("{Enter}");
        expect(onSelect).toHaveBeenCalledWith("changes");
        expect(screen.queryByTestId("right-panel-tabs-overflow-popover")).toBeNull();
      } finally {
        restore();
      }
    });

    it("shows no All tabs button while the tabs fit", () => {
      const restore = stubLayout({
        header: 2000,
        tabs: 300,
        scroller: 300,
        actions: 50,
      });
      try {
        renderStrip({ actions });
        expect(screen.queryByLabelText("All tabs")).toBeNull();
      } finally {
        restore();
      }
    });

    // Picking the tab that is already active must still bring it back into
    // view — the user may have panned it behind the fade.
    it("scrolls a picked tab into view even when it is already active", async () => {
      const restore = stubLayout({ header: 600, tabs: 2500, scroller: 400 });
      const scrollIntoView = vi.fn();
      Object.defineProperty(Element.prototype, "scrollIntoView", {
        value: scrollIntoView,
        configurable: true,
      });
      try {
        const { chip } = renderStrip({ tabs: [...EXTRA_TABS, ...TABS] });
        scrollIntoView.mockClear();
        fireEvent.click(screen.getByLabelText("All tabs"));
        const popover = await screen.findByTestId(
          "right-panel-tabs-overflow-popover",
        );
        fireEvent.click(
          listRow(within(popover).getByRole("group"), "Files"),
        );
        expect(scrollIntoView).toHaveBeenCalled();
        expect(scrollIntoView.mock.contexts).toContain(chip("files"));
      } finally {
        delete (Element.prototype as { scrollIntoView?: unknown })
          .scrollIntoView;
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
