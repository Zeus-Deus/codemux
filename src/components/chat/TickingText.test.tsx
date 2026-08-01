/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

import { TickingText } from "./TickingText";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const parentRenders = { count: 0 };

function Parent({
  active = true,
  suffix = "",
}: {
  active?: boolean;
  suffix?: string;
}) {
  parentRenders.count += 1;
  return (
    <TickingText
      testId="elapsed"
      active={active}
      compute={(now) => `${Math.floor(now / 1000)}${suffix}`}
    />
  );
}

describe("TickingText", () => {
  it("advances its own text without re-rendering the parent", () => {
    vi.setSystemTime(new Date(60_000));
    parentRenders.count = 0;
    render(<Parent />);

    const node = screen.getByTestId("elapsed");
    expect(node).toHaveTextContent("60");
    const rendersAfterMount = parentRenders.count;

    act(() => {
      vi.advanceTimersByTime(3000);
    });

    expect(node).toHaveTextContent("63");
    // Three seconds of ticking, zero React commits.
    expect(parentRenders.count).toBe(rendersAfterMount);
  });

  it("freezes when inactive", () => {
    vi.setSystemTime(new Date(60_000));
    render(<Parent active={false} />);
    const node = screen.getByTestId("elapsed");
    expect(node).toHaveTextContent("60");

    act(() => {
      vi.advanceTimersByTime(9000);
    });

    expect(node).toHaveTextContent("60");
  });

  it("repaints on commit when the compute closure changes", () => {
    vi.setSystemTime(new Date(60_000));
    const { rerender } = render(<Parent />);
    const node = screen.getByTestId("elapsed");
    expect(node).toHaveTextContent("60");

    rerender(<Parent suffix="s" />);
    expect(node).toHaveTextContent("60s");
  });

  it("stops ticking after unmount", () => {
    vi.setSystemTime(new Date(60_000));
    const { unmount } = render(<Parent />);
    unmount();
    expect(() => {
      act(() => {
        vi.advanceTimersByTime(5000);
      });
    }).not.toThrow();
  });
});
