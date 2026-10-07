import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";

import { TerminalFindBar } from "./TerminalFindBar";

function fakeSearch() {
  let listener: ((event: ISearchResultChangeEvent) => void) | null = null;
  const search = {
    findNext: vi.fn(() => true),
    findPrevious: vi.fn(() => true),
    clearDecorations: vi.fn(),
    onDidChangeResults: vi.fn((cb: (event: ISearchResultChangeEvent) => void) => {
      listener = cb;
      return { dispose: () => (listener = null) };
    }),
  };
  const emit = (event: ISearchResultChangeEvent) => act(() => listener?.(event));
  return { search, emit };
}

describe("TerminalFindBar", () => {
  afterEach(cleanup);

  it("searches as you type and shows the match position", () => {
    const { search, emit } = fakeSearch();
    const view = render(<TerminalFindBar search={search} focusToken={1} onClose={() => {}} />);
    const input = view.getByRole("textbox", { name: "Find in terminal" });
    expect(document.activeElement).toBe(input);

    fireEvent.change(input, { target: { value: "error" } });
    expect(search.findNext).toHaveBeenLastCalledWith(
      "error",
      expect.objectContaining({ incremental: true, caseSensitive: false, regex: false }),
    );
    emit({ resultIndex: 2, resultCount: 17 });
    expect(view.getByText("3/17")).toBeTruthy();
  });

  it("steps with Enter and Shift+Enter, and closes on Escape", () => {
    const { search } = fakeSearch();
    const onClose = vi.fn();
    const view = render(<TerminalFindBar search={search} focusToken={1} onClose={onClose} />);
    const input = view.getByRole("textbox", { name: "Find in terminal" });
    fireEvent.change(input, { target: { value: "x" } });

    fireEvent.keyDown(input, { key: "Enter" });
    expect(search.findNext).toHaveBeenLastCalledWith("x", expect.objectContaining({ incremental: false }));
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(search.findPrevious).toHaveBeenLastCalledWith("x", expect.objectContaining({ incremental: false }));
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("re-runs with case and regex toggles and flags a bad pattern", () => {
    const { search } = fakeSearch();
    const view = render(<TerminalFindBar search={search} focusToken={1} onClose={() => {}} />);
    fireEvent.change(view.getByRole("textbox", { name: "Find in terminal" }), {
      target: { value: "a(" },
    });
    fireEvent.click(view.getByRole("button", { name: "Match case" }));
    expect(search.findNext).toHaveBeenLastCalledWith("a(", expect.objectContaining({ caseSensitive: true }));

    search.findNext.mockImplementation(() => {
      throw new SyntaxError("Unterminated group");
    });
    fireEvent.click(view.getByRole("button", { name: "Use regular expression" }));
    expect(view.getByText("Invalid pattern")).toBeTruthy();
  });

  it("shows no results for a query that matches nothing", () => {
    const { search, emit } = fakeSearch();
    const view = render(<TerminalFindBar search={search} focusToken={1} onClose={() => {}} />);
    fireEvent.change(view.getByRole("textbox", { name: "Find in terminal" }), {
      target: { value: "zzz" },
    });
    emit({ resultIndex: -1, resultCount: 0 });
    expect(view.getByText("No results")).toBeTruthy();
  });
});
