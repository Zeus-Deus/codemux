/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import type { ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn().mockResolvedValue([]),
    listProjectFolders: vi.fn().mockResolvedValue([]),
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import {
  collapseDisabledFor,
  Composer,
  composerFooterLadder,
  FOOTER_COMPACT_STEPS,
  FOOTER_MIN_GAP_EXPANDED_PX,
  FOOTER_MIN_GAP_PX,
  footerMinGapFor,
  MIN_TEXTAREA_PX,
  OPEN_TEXTAREA_PX,
  nextFooterFit,
  type FooterFit,
} from "./Composer";

type ComposerProps = ComponentProps<typeof Composer>;

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/repo",
    provider: "claude",
    model: null,
    permissionMode: null,
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: null,
    ultrathinkInBodyText: false,
    streaming: false,
    sessionReady: true,
    showProviderPicker: false,
    mode: "default",
    onDraftChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    onProviderModelChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function ui(props: Partial<ComposerProps> = {}) {
  return (
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>
  );
}

function renderComposer(props: Partial<ComposerProps> = {}) {
  const result = render(ui(props));
  const wrapper = () => result.getByTestId("composer-wrapper");
  const body = () => result.getByTestId("composer-body");
  const textarea = () =>
    result.container.querySelector("textarea") as HTMLTextAreaElement;
  return { ...result, wrapper, body, textarea };
}

const USAGE = { used_tokens: 44_000, max_tokens: 200_000 };

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("Composer pill collapse", () => {
  it("rests as a 22px-radius pill with the placeholder in the controls gap", () => {
    const { wrapper, body, getByTestId, queryByTestId } = renderComposer({
      contextUsage: USAGE,
    });
    expect(wrapper()).not.toHaveAttribute("data-expanded");
    expect(wrapper().className).toContain("rounded-[22px]");
    expect(body()).toHaveAttribute("data-collapsed");
    expect(body().className).toContain("grid-rows-[0fr]");
    expect(getByTestId("composer-gap")).toHaveTextContent(
      "Reply or steer the agent…",
    );
    // The ring rests in the chin under the pill, not on the pill's row.
    expect(getByTestId("composer-chin")).toContainElement(
      getByTestId("context-usage-trigger"),
    );
    expect(
      getByTestId("composer-controls-row").querySelector('[data-testid="context-usage-trigger"]'),
    ).toBeNull();
    expect(queryByTestId("composer-chin")).not.toBeNull();
  });

  it("expands for draft text and keeps the ring in the expanded card", () => {
    const { wrapper, getByTestId } = renderComposer({
      draft: "hi",
      contextUsage: USAGE,
    });
    expect(wrapper()).toHaveAttribute("data-expanded");
    expect(wrapper().className).toContain("rounded-[22px]");
    expect(getByTestId("context-usage-trigger")).toBeInTheDocument();
  });

  it("compacts until a newly visible context ring fits a 40px gap with 60px of content", () => {
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    const gapWidth = (element: HTMLElement) =>
      element.parentElement?.querySelector('[aria-label^="Model:"]') ? 64 : 40;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === "composer-wrapper") return new DOMRect(0, 0, 358, 100);
      if (this.dataset.testid === "composer-gap") return new DOMRect(0, 0, gapWidth(this), 44);
      return originalRect.call(this);
    });
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === "composer-gap" ? gapWidth(this) : 0;
    });
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid !== "composer-gap") return 0;
      return this.querySelector('[data-testid="context-usage-trigger"]')
        ? Math.max(60, gapWidth(this))
        : gapWidth(this);
    });

    const props = { draft: "next", model: "claude-opus-4-8" };
    const result = renderComposer(props);
    const gap = result.getByTestId("composer-gap");
    gap.style.minWidth = "0px";
    gap.style.paddingLeft = "8px";
    // Without the ring, the small phone gap leaves the model label intact.
    expect(result.getByTitle("Claude Opus 4.8")).toHaveTextContent("Claude Opus 4.8");

    result.rerender(ui({ ...props, contextUsage: USAGE }));
    expect(result.getByTestId("context-usage-trigger")).toBeInTheDocument();
    expect(result.getByRole("button", { name: "Model: Claude Opus 4.8" })).toBeInTheDocument();
    expect(gap.scrollWidth).toBeLessThanOrEqual(gap.clientWidth);
  });

  it("turns the gap into 'Enter to queue' while a turn streams", () => {
    const { getByTestId } = renderComposer({ draft: "next", streaming: true });
    expect(getByTestId("composer-gap")).toHaveTextContent("Enter to queue");
  });

  it("offers Stop but no delivery choice while a turn streams without a draft", () => {
    const { queryByRole, getByRole } = renderComposer({ streaming: true });
    // Queue / steer only appears once there is something to send.
    expect(queryByRole("button", { name: /Message delivery/ })).toBeNull();
    expect(getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  it("offers queue / steer once there is a draft to send mid-turn", () => {
    const { getByRole } = renderComposer({ draft: "next", streaming: true });
    expect(getByRole("button", { name: "Message delivery: Queue" })).toBeInTheDocument();
  });

  it("rests model and access in the chin, and opens them into the card", () => {
    const permissionModes = [
      { value: "bypassPermissions", label: "Full access", description: "", is_default: true },
    ];
    const resting = renderComposer({ permissionModes });
    expect(resting.getByTestId("composer-chin")).toContainElement(
      resting.getByRole("button", { name: /Full access/ }),
    );
    cleanup();

    const open = renderComposer({ permissionModes, draft: "x" });
    expect(open.queryByTestId("composer-chin")).toBeNull();
    expect(open.getByTestId("composer-controls-row")).toContainElement(
      open.getByRole("button", { name: /Full access/ }),
    );
  });

  it("stays open with its settings inside when a strip sits under the card", () => {
    const { wrapper, queryByTestId } = renderComposer({
      belowComposerSlot: <div>project · branch</div>,
    });
    expect(wrapper()).toHaveAttribute("data-expanded");
    expect(queryByTestId("composer-chin")).toBeNull();
  });

  it("uses the 94px textarea floor on drafts and two lines on a live card", () => {
    const draft = renderComposer({ isDraft: true });
    expect(draft.wrapper()).toHaveAttribute("data-expanded");
    expect(draft.textarea().style.minHeight).toBe(`${MIN_TEXTAREA_PX}px`);
    cleanup();

    const live = renderComposer({ draft: "x" });
    expect(live.textarea().style.minHeight).toBe(`${OPEN_TEXTAREA_PX}px`);
    expect(live.textarea().style.maxHeight).toBe("220px");
  });

  it.each<[string, Partial<ComposerProps>]>([
    ["a staged attachment", {
      stagedAttachments: [
        { id: "i", kind: "image", ref: "image:1", metadata: { label: "a.png" } },
      ],
    }],
    ["an active mode", { mode: "plan" }],
  ])("expands for %s", (_name, props) => {
    const { wrapper } = renderComposer(props);
    expect(wrapper()).toHaveAttribute("data-expanded");
  });

  it("stays collapsed while a queued message waits in the strip", () => {
    const { wrapper } = renderComposer({
      streaming: true,
      stripSlot: <div>Queued follow-up</div>,
    });
    expect(wrapper()).not.toHaveAttribute("data-expanded");
  });

  it("expands on focus, collapses on blur — but never within 400ms of a send", () => {
    vi.useFakeTimers();
    const onSubmit = vi.fn();
    const result = renderComposer({ draft: "ship it", onSubmit });
    act(() => result.textarea().focus());
    expect(result.wrapper()).toHaveAttribute("data-expanded");

    fireEvent.keyDown(result.textarea(), { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledOnce();
    // The send clears the draft and focus leaves the card.
    result.rerender(ui({ draft: "", onSubmit }));
    act(() => result.textarea().blur());
    expect(result.wrapper()).toHaveAttribute("data-expanded");

    act(() => {
      vi.advanceTimersByTime(400);
    });
    expect(result.wrapper()).not.toHaveAttribute("data-expanded");
  });

  it("folds back to the pill after a send even though the caret stays", () => {
    vi.useFakeTimers();
    const onSubmit = vi.fn();
    const result = renderComposer({ draft: "ship it", onSubmit });
    act(() => result.textarea().focus());
    fireEvent.keyDown(result.textarea(), { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledOnce();
    result.rerender(ui({ draft: "", onSubmit }));
    act(() => {
      vi.advanceTimersByTime(400);
    });
    // Still focused — typing keeps landing — but the card is a pill.
    expect(document.activeElement).toBe(result.textarea());
    expect(result.wrapper()).not.toHaveAttribute("data-expanded");
    // The fold animates; the unfold must stay instant.
    expect(result.body().className).toContain("transition-");

    // Typing the next message re-expands it.
    result.rerender(ui({ draft: "a", onSubmit }));
    expect(result.wrapper()).toHaveAttribute("data-expanded");
    expect(result.body().className).not.toContain("transition-");
    result.rerender(ui({ draft: "", onSubmit }));
    expect(result.wrapper()).not.toHaveAttribute("data-expanded");

    // So does reaching for the card.
    fireEvent.pointerDown(result.wrapper(), { button: 0 });
    expect(result.wrapper()).toHaveAttribute("data-expanded");
  });

  it("focus carried across a send by focusOnMount rests as a pill", () => {
    const result = renderComposer({ focusOnMount: true });
    expect(document.activeElement).toBe(result.textarea());
    expect(result.wrapper()).not.toHaveAttribute("data-expanded");
    // Tabbing away and back is a real focus, which opens it.
    act(() => result.textarea().blur());
    act(() => result.textarea().focus());
    expect(result.wrapper()).toHaveAttribute("data-expanded");
  });

  it("clicking the gap focuses the textarea, which expands the card", () => {
    const { getByTestId, textarea, wrapper } = renderComposer();
    fireEvent.pointerDown(getByTestId("composer-gap"), { button: 0 });
    expect(document.activeElement).toBe(textarea());
    expect(wrapper()).toHaveAttribute("data-expanded");
  });

  it("a pane drag grows the collapsed pill into a 74px drop target", () => {
    const { wrapper, getByTestId, rerender } = renderComposer({
      paneDragActive: true,
    });
    expect(wrapper()).toHaveAttribute("data-drop-target");
    expect(getByTestId("composer-drop-target").className).toContain("h-[30px]");
    rerender(ui({ paneDragActive: false }));
    expect(wrapper()).not.toHaveAttribute("data-drop-target");
  });

  it("docks the strip slot above the pill, outside the card", () => {
    const { wrapper, getByText } = renderComposer({
      stripSlot: <div>strip here</div>,
    });
    const strip = getByText("strip here");
    expect(wrapper().contains(strip)).toBe(false);
    expect(strip.compareDocumentPosition(wrapper())).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});

describe("collapseDisabledFor", () => {
  it("keeps the pill expanded below 400px", () => {
    expect(collapseDisabledFor(400)).toBe(false);
    expect(collapseDisabledFor(399)).toBe(true);
  });
});

describe("composerFooterLadder", () => {
  it("gives up labels in order: leaf model, access, effort, model, config", () => {
    expect(composerFooterLadder(0)).toEqual({
      leafModelLabel: false,
      accessIconOnly: false,
      effortIconOnly: false,
      modelIconOnly: false,
      configInMenu: false,
    });
    expect(composerFooterLadder(2)).toMatchObject({
      leafModelLabel: true,
      accessIconOnly: true,
      effortIconOnly: false,
    });
    expect(composerFooterLadder(4)).toMatchObject({
      modelIconOnly: true,
      configInMenu: false,
    });
    expect(composerFooterLadder(FOOTER_COMPACT_STEPS).configInMenu).toBe(true);
  });
});

describe("nextFooterFit", () => {
  const fit = (level: number, fitWidths: number[] = []): FooterFit => ({
    key: "k",
    level,
    fitWidths,
  });

  it("holds while the gap is wide enough", () => {
    const prev = fit(0);
    expect(nextFooterFit(prev, "k", 700, FOOTER_MIN_GAP_PX)).toBe(prev);
  });

  it("steps in and records the width at which the step fits again", () => {
    expect(nextFooterFit(fit(0), "k", 540, FOOTER_MIN_GAP_PX - 30)).toEqual(
      fit(1, [570]),
    );
  });

  it("steps back out once the pill reaches the recorded width", () => {
    const prev = fit(2, [570, 600]);
    expect(nextFooterFit(prev, "k", 599, 200)).toBe(prev);
    expect(nextFooterFit(prev, "k", 600, 200)).toEqual(fit(1, [570]));
  });

  it("stops at the last step", () => {
    const prev = fit(FOOTER_COMPACT_STEPS, [500, 520, 540, 560, 580]);
    expect(nextFooterFit(prev, "k", 300, 10)).toBe(prev);
  });

  it("measures against the given minimum gap", () => {
    // A phone's card leaves only 30px of gap: enough there, not on desktop.
    const prev = fit(0);
    expect(nextFooterFit(prev, "k", 358, 30, 12)).toBe(prev);
    expect(nextFooterFit(prev, "k", 358, 10, 12)).toEqual(fit(1, [360]));
  });

  it("re-fits from step 0 when the content changes", () => {
    expect(nextFooterFit(fit(3, [500, 520, 540]), "k2", 400, 10)).toEqual({
      key: "k2",
      level: 0,
      fitWidths: [],
    });
  });
});

describe("footerMinGapFor", () => {
  it("keeps room for the placeholder while the card can still collapse", () => {
    expect(footerMinGapFor(400)).toBe(FOOTER_MIN_GAP_PX);
  });

  it("only asks for a sliver of gap once the card never collapses", () => {
    expect(footerMinGapFor(358)).toBe(FOOTER_MIN_GAP_EXPANDED_PX);
  });

  it("stays above the gap's own floor, so an overflowing row still steps", () => {
    // Desktop reserves 50px for the context ring; a phone only has padding.
    expect(footerMinGapFor(358, 50)).toBe(54);
    expect(footerMinGapFor(358, 8)).toBe(12);
  });
});

describe("Composer /permissions", () => {
  it("opens the access picker from the + menu once the row has moved it there", async () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === "composer-wrapper") return new DOMRect(0, 0, 300, 100);
      return new DOMRect(0, 0, 0, 0);
    });
    const result = renderComposer({
      draft: "/permissions",
      permissionModes: [
        { value: "default", label: "Ask", description: "", is_default: true },
        { value: "bypassPermissions", label: "Full access", description: "", is_default: false },
      ],
    });
    // The row gave up its last step: access lives in the + menu.
    const row = within(result.getByTestId("composer-controls-row"));
    expect(row.queryByRole("button", { name: /Ask/ })).toBeNull();
    const textarea = result.textarea();
    fireEvent.focus(textarea);
    await act(async () => {});
    fireEvent.keyDown(textarea, { key: "Enter" });
    await act(async () => {
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    });
    const menu = within(result.getByTestId("composer-command-menu"));
    expect(menu.getByRole("button", { name: /Ask/ })).toHaveAttribute("data-state", "open");
  });
});
