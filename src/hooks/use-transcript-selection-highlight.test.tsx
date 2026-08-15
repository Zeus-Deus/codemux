import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CHAT_SELECTION_TEXT_ATTRIBUTE } from "@/lib/agent-chat/selection-safe-text";
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

  it("omits an embedded hard break and its indentation from the paint ranges", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <div style="user-select: text">
          <p id="prose">alpha<br>\n    omega</p>
        </div>
      </div>
    `;
    const prose = document.querySelector("#prose")!;
    const first = prose.firstChild as Text;
    const second = prose.lastChild as Text;
    const selection = selectBetween(first, 0, second, second.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    // The native selection still contains the hard break for copying, but
    // WebKitGTK must not receive it as part of a painted custom range.
    expect(selection.toString()).toContain("\n");
    expect(pieces.map((range) => range.toString())).toEqual(["alpha", "omega"]);
  });

  it("preserves a newline that collapses to a space inside inline prose", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <p id="prose" style="user-select: text"><em>alpha</em>\n<em>omega</em></p>
      </div>
    `;
    const prose = document.querySelector("#prose")!;
    const first = prose.firstChild!.firstChild as Text;
    const space = prose.childNodes[1] as Text;
    const second = prose.lastChild!.firstChild as Text;
    const selection = selectBetween(first, 0, second, second.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(space.data).toBe("\n");
    expect(pieces.map((range) => range.toString())).toEqual([
      "alpha",
      "\n",
      "omega",
    ]);
  });

  it("sees through selection-text wrappers when trimming structural edges", () => {
    // Assistant markdown wraps each prose text node in its own span, so a
    // soft-wrapped line next to emphasis has no siblings of its own. The
    // trailing newline is still the collapsed space before "omega".
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <p id="prose" style="user-select: text"><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>alpha\n</span><em><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>omega</span></em></p>
      </div>
    `;
    const prose = document.querySelector("#prose")!;
    const first = prose.firstChild!.firstChild as Text;
    const second = prose.lastChild!.firstChild!.firstChild as Text;
    const selection = selectBetween(first, 0, second, second.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(pieces.map((range) => range.toString())).toEqual([
      "alpha\n",
      "omega",
    ]);
  });

  it("sees through selection-text wrappers on a leading collapsed space", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <p id="prose" style="user-select: text"><em><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>alpha</span></em><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>\nomega</span></p>
      </div>
    `;
    const prose = document.querySelector("#prose")!;
    const first = prose.firstChild!.firstChild!.firstChild as Text;
    const second = prose.lastChild!.firstChild as Text;
    const selection = selectBetween(first, 0, second, second.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(pieces.map((range) => range.toString())).toEqual([
      "alpha",
      "\nomega",
    ]);
  });

  it("still trims structural edges for a wrapper at a block boundary", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <div style="user-select: text">
          <p id="prose"><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>alpha\n</span></p>
          <p id="tail"><span ${CHAT_SELECTION_TEXT_ATTRIBUTE}>omega</span></p>
        </div>
      </div>
    `;
    const first = document.querySelector("#prose")!.firstChild!
      .firstChild as Text;
    const second = document.querySelector("#tail")!.firstChild!
      .firstChild as Text;
    const selection = selectBetween(first, 0, second, second.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(pieces.map((range) => range.toString())).toEqual(["alpha", "omega"]);
  });

  it("preserves structural-looking whitespace in preformatted content", () => {
    document.body.innerHTML = `
      <div data-slot="transcript-list">
        <pre id="code" style="user-select: text; white-space: pre-wrap">\n\n  omega\n</pre>
      </div>
    `;
    const code = document.querySelector("#code")!.firstChild as Text;
    const selection = selectBetween(code, 0, code, code.length);

    const pieces = collectTranscriptSelectionRanges(selection);

    expect(pieces.map((range) => range.toString())).toEqual(["\n  omega\n"]);
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
