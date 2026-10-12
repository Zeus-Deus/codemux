import { describe, expect, it } from "vitest";
import { revealSelection } from "./reveal-selection";

// A line "the needle" starting at document offset 100.
const line = { from: 100, to: 110 };

describe("revealSelection", () => {
  it("collapses the cursor at the column when there is no end", () => {
    expect(revealSelection(line, 5, undefined)).toEqual({ anchor: 104, head: 104 });
    expect(revealSelection(line, undefined, undefined)).toEqual({ anchor: 100, head: 100 });
  });

  it("selects a hit at column 1", () => {
    // "the" is columns [1, 4)
    expect(revealSelection(line, 1, 4)).toEqual({ anchor: 100, head: 103 });
  });

  it("selects a hit that ends the line", () => {
    // "needle" is columns [5, 11)
    expect(revealSelection(line, 5, 11)).toEqual({ anchor: 104, head: 110 });
  });

  it("clamps an end past the line", () => {
    expect(revealSelection(line, 5, 99)).toEqual({ anchor: 104, head: 110 });
  });

  it("clamps a start past the line", () => {
    expect(revealSelection(line, 50, 60)).toEqual({ anchor: 110, head: 110 });
  });
});
