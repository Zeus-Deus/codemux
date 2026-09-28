import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { PLATES, SidebarEmptyState, pickPlateIndex } from "./sidebar-empty-state";

describe("pickPlateIndex", () => {
  it("never repeats the previous plate, for any draw", () => {
    for (let last = 0; last < PLATES.length; last++) {
      for (let step = 0; step < 100; step++) {
        const pick = pickPlateIndex(last, PLATES.length, step / 100);
        expect(pick).not.toBe(last);
        expect(pick).toBeGreaterThanOrEqual(0);
        expect(pick).toBeLessThan(PLATES.length);
      }
    }
  });

  it("can still reach every other plate", () => {
    const seen = new Set<number>();
    for (let step = 0; step < 1000; step++) seen.add(pickPlateIndex(3, PLATES.length, step / 1000));
    expect(seen.size).toBe(PLATES.length - 1);
    expect(seen.has(3)).toBe(false);
  });

  it("draws from the whole set when there is no usable previous plate", () => {
    for (const last of [-1, Number.NaN, 99, 2.5]) {
      const seen = new Set<number>();
      for (let step = 0; step < 1000; step++) seen.add(pickPlateIndex(last, PLATES.length, step / 1000));
      expect(seen.size).toBe(PLATES.length);
    }
  });

  it("handles a single-entry set", () => {
    expect(pickPlateIndex(0, 1, 0.7)).toBe(0);
  });
});

describe("SidebarEmptyState", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("shows a different quote the next time the sidebar empties out", () => {
    vi.spyOn(Math, "random").mockReturnValue(0);
    render(<SidebarEmptyState filterName={null} />);
    const first = PLATES[0];
    expect(screen.getByText(`“${first.quote}”`)).toBeInTheDocument();
    expect(screen.getByText(`— ${first.source}`)).toBeInTheDocument();
    cleanup();

    // Same random draw, but plate 0 was just shown, so it must move on.
    render(<SidebarEmptyState filterName={null} />);
    expect(screen.queryByText(`“${first.quote}”`)).not.toBeInTheDocument();
    expect(screen.getByText(`“${PLATES[1].quote}”`)).toBeInTheDocument();
  });

  it("names the filtered project and offers to start an agent", () => {
    render(<SidebarEmptyState filterName="vexis" />);
    expect(screen.getByText(/Nothing running in/)).toBeInTheDocument();
    expect(screen.getByText("vexis")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Put them to work/ })).toBeInTheDocument();
  });
});
