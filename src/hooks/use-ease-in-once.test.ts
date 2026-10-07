import { describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";

import { EASED_ID_LIMIT, markEased, useEaseInOnce } from "./use-ease-in-once";

describe("useEaseInOnce", () => {
  it("eases on the first mount for an id and not on later mounts", () => {
    expect(renderHook(() => useEaseInOnce("ease-a")).result.current).toBe(true);
    expect(renderHook(() => useEaseInOnce("ease-a")).result.current).toBe(false);
  });

  it("forgets the oldest ids once the cap is reached", () => {
    markEased("ease-oldest");
    markEased("ease-recent");
    // One past the cap pushes out exactly the oldest id.
    for (let i = 0; i < EASED_ID_LIMIT - 1; i++) markEased(`ease-filler-${i}`);

    expect(renderHook(() => useEaseInOnce("ease-recent")).result.current).toBe(false);
    expect(renderHook(() => useEaseInOnce("ease-oldest")).result.current).toBe(true);
  });
});
