/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
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

import { Composer, composerWidthLadder, MIN_TEXTAREA_PX } from "./Composer";

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
});

describe("Composer pill collapse", () => {
  it("rests as a 22px-radius pill with the placeholder in the controls gap", () => {
    const { wrapper, body, getByTestId, queryByTestId } = renderComposer({
      contextUsage: USAGE,
    });
    expect(wrapper()).not.toHaveAttribute("data-expanded");
    expect(wrapper().className).toContain("rounded-[22px]");
    expect(body()).toHaveAttribute("data-collapsed");
    expect(body().className).toContain("h-0");
    expect(getByTestId("composer-gap")).toHaveTextContent(
      "Reply or steer the agent…",
    );
    // No context ring on the collapsed row.
    expect(queryByTestId("context-usage-trigger")).toBeNull();
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

  it("turns the gap into 'Enter to queue' while a turn streams", () => {
    const { getByTestId } = renderComposer({ draft: "next", streaming: true });
    expect(getByTestId("composer-gap")).toHaveTextContent("Enter to queue");
  });

  it("uses the 94px textarea floor only on drafts", () => {
    const draft = renderComposer({ isDraft: true });
    expect(draft.wrapper()).toHaveAttribute("data-expanded");
    expect(draft.textarea().style.minHeight).toBe(`${MIN_TEXTAREA_PX}px`);
    cleanup();

    const live = renderComposer({ draft: "x" });
    expect(live.textarea().style.minHeight).toBe("0px");
    expect(live.textarea().style.maxHeight).toBe("220px");
  });

  it.each<[string, Partial<ComposerProps>]>([
    ["a staged attachment", {
      stagedAttachments: [
        { id: "i", kind: "image", ref: "image:1", metadata: { label: "a.png" } },
      ],
    }],
    ["an active mode pill", { mode: "plan" }],
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

describe("composerWidthLadder", () => {
  it("steps down at 620 / 540 / 500 / 460 / 400", () => {
    expect(composerWidthLadder(760)).toEqual({
      leafModelLabel: false,
      accessIconOnly: false,
      effortIconOnly: false,
      configInMenu: false,
      collapseDisabled: false,
    });
    expect(composerWidthLadder(619).leafModelLabel).toBe(true);
    expect(composerWidthLadder(539)).toMatchObject({
      accessIconOnly: true,
      effortIconOnly: false,
    });
    expect(composerWidthLadder(499).effortIconOnly).toBe(true);
    expect(composerWidthLadder(459).configInMenu).toBe(true);
    expect(composerWidthLadder(399).collapseDisabled).toBe(true);
    expect(composerWidthLadder(null).collapseDisabled).toBe(false);
  });
});
