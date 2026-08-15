import { describe, expect, it } from "vitest";

import {
  CHAT_SELECTION_TEXT_ATTRIBUTE,
  rehypeSelectionSafeText,
} from "./selection-safe-text";

interface TestNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: TestNode[];
}

describe("rehypeSelectionSafeText", () => {
  it("gives prose text a real inline box without wrapping markup whitespace", () => {
    const prose = { type: "text", value: "direct prose" };
    const whitespace = { type: "text", value: "\n  " };
    const emphasized = { type: "text", value: "emphasis" };
    const tree: TestNode = {
      type: "root",
      children: [
        {
          type: "element",
          tagName: "p",
          children: [
            prose,
            whitespace,
            {
              type: "element",
              tagName: "em",
              children: [emphasized],
            },
          ],
        },
      ],
    };

    rehypeSelectionSafeText()(tree);

    const paragraph = tree.children![0]!;
    expect(paragraph.children![0]).toEqual({
      type: "element",
      tagName: "span",
      properties: { [CHAT_SELECTION_TEXT_ATTRIBUTE]: "" },
      children: [prose],
    });
    expect(paragraph.children![1]).toBe(whitespace);
    expect(paragraph.children![2]!.children![0]).toEqual({
      type: "element",
      tagName: "span",
      properties: { [CHAT_SELECTION_TEXT_ATTRIBUTE]: "" },
      children: [emphasized],
    });
  });
});
