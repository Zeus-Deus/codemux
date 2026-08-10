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

  it("paints the spaces between inline elements but not markup between rows", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <div style="user-select: text">
          <p id="prose"><em id="a">alpha</em> <em id="b">omega</em></p>
          <p id="tail">delta</p>
        </div>
      </div>
    `;
    const start = document.querySelector("#a")!.firstChild as Text;
    const end = document.querySelector("#tail")!.firstChild as Text;
    const selection = selectBetween(start, 0, end, 5);

    const pieces = collectTranscriptSelectionRanges(selection);

    // The space between the two <em>s is prose and paints; the newline the
    // markup puts between the two block paragraphs is scaffolding and does not.
    expect(pieces.map((range) => range.toString())).toEqual([
      "alpha",
      " ",
      "omega",
      "delta",
    ]);
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

  it("repaints rows the virtualizer remounts inside a live selection", async () => {
    const { first, second } = transcriptFixture();
    selectBetween(first, 0, second, 5);
    const registry = new Map<string, { ranges: AbstractRange[] }>();

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
      expect(registry.get(TRANSCRIPT_SELECTION_HIGHLIGHT)?.ranges).toHaveLength(
        2,
      ),
    );

    // A row scrolling back into view brings text nodes no existing range
    // covers. Nothing touches the selection, so `selectionchange` never fires.
    const row = document.createElement("div");
    row.style.userSelect = "text";
    row.textContent = "middle";
    second.parentElement!.before(row);

    await waitFor(() =>
      expect(registry.get(TRANSCRIPT_SELECTION_HIGHLIGHT)?.ranges).toHaveLength(
        3,
      ),
    );

    unmount();
  });
});
