import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  TRANSCRIPT_SELECTION_CLASS,
  TRANSCRIPT_SELECTION_HIGHLIGHT,
  collectTranscriptSelectionRanges,
  useTranscriptSelectionHighlight,
} from "./use-transcript-selection-highlight";

function selectBetween(
  start: Text,
  startOffset: number,
  end: Text,
  endOffset: number,
) {
  const range = document.createRange();
  range.setStart(start, startOffset);
  range.setEnd(end, endOffset);
  const selection = document.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  return selection;
}

function transcriptFixture() {
  document.body.innerHTML = `
    <div data-slot="transcript-list">
      <div style="user-select: text"><p id="first">alpha</p></div>
      <div style="user-select: none">chrome</div>
      <div style="user-select: text"><p id="second">omega</p></div>
    </div>
  `;
  return {
    first: document.querySelector("#first")!.firstChild as Text,
    second: document.querySelector("#second")!.firstChild as Text,
  };
}

beforeEach(() => {
  document.getSelection()?.removeAllRanges();
  document.documentElement.classList.remove(TRANSCRIPT_SELECTION_CLASS);
});

afterEach(() => {
  cleanup();
  document.body.replaceChildren();
  document.getSelection()?.removeAllRanges();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("collectTranscriptSelectionRanges", () => {
  it("keeps selected text but omits structural and non-selectable nodes", () => {
    const { first, second } = transcriptFixture();
    const selection = selectBetween(first, 2, second, 3);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(pieces.map((range) => range.toString())).toEqual(["pha", "ome"]);
  });

  it("returns no paint ranges for a collapsed caret", () => {
    const { first } = transcriptFixture();
    const selection = selectBetween(first, 2, first, 2);

    expect(collectTranscriptSelectionRanges(selection)).toEqual([]);
  });
});

describe("useTranscriptSelectionHighlight", () => {
  it("gates native suppression on API support and preserves the native selection", async () => {
    const { first, second } = transcriptFixture();
    const selection = selectBetween(first, 1, second, 4);
    const selectedText = selection.toString();
    const registry = new Map<string, Highlight>();

    class MockHighlight {
      readonly ranges: AbstractRange[];
      constructor(...ranges: AbstractRange[]) {
        this.ranges = ranges;
      }
    }

    vi.stubGlobal("CSS", { highlights: registry });
    vi.stubGlobal("Highlight", MockHighlight);

    const { unmount } = renderHook(() => useTranscriptSelectionHighlight());

    await waitFor(() =>
      expect(registry.has(TRANSCRIPT_SELECTION_HIGHLIGHT)).toBe(true),
    );
    expect(document.documentElement).toHaveClass(TRANSCRIPT_SELECTION_CLASS);
    expect(document.getSelection()?.toString()).toBe(selectedText);

    unmount();
    expect(registry.has(TRANSCRIPT_SELECTION_HIGHLIGHT)).toBe(false);
    expect(document.documentElement).not.toHaveClass(TRANSCRIPT_SELECTION_CLASS);
  });
});
