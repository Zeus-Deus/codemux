import { afterEach, describe, expect, it, vi } from "vitest";

import { focusCmdkOnOpen } from "./focus-cmdk-root";

function openEvent(): { event: Event; input: HTMLInputElement; root: HTMLDivElement } {
  const content = document.createElement("div");
  const root = document.createElement("div");
  root.setAttribute("cmdk-root", "");
  root.tabIndex = -1;
  const input = document.createElement("input");
  input.setAttribute("cmdk-input", "");
  root.append(input);
  content.append(root);
  document.body.append(content);
  const event = new Event("focus", { cancelable: true });
  Object.defineProperty(event, "currentTarget", { value: content });
  return { event, input, root };
}

function pointer(coarse: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: coarse && query === "(pointer: coarse)",
  }));
}

describe("focusCmdkOnOpen", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("focuses the search field so typing filters at once", () => {
    pointer(false);
    const { event, input } = openEvent();
    focusCmdkOnOpen(event);
    expect(document.activeElement).toBe(input);
  });

  it("focuses the list on touch screens, keeping the keyboard down", () => {
    pointer(true);
    const { event, root } = openEvent();
    focusCmdkOnOpen(event);
    expect(document.activeElement).toBe(root);
  });
});
