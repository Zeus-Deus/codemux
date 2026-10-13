import { describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";

import { useMountedOnceOpen } from "./use-mounted-once-open";

describe("useMountedOnceOpen", () => {
  it("stays unmounted until the first open, then stays mounted", () => {
    const { result, rerender } = renderHook(
      ({ open }) => useMountedOnceOpen(open),
      { initialProps: { open: false } },
    );
    expect(result.current).toBe(false);

    rerender({ open: true });
    expect(result.current).toBe(true);

    // Closing keeps it mounted so the dialog's exit animation can play.
    rerender({ open: false });
    expect(result.current).toBe(true);
  });
});
