import { describe, it, expect, beforeEach } from "vitest";
import {
  setJumpTargets,
  getJumpTarget,
  DEFAULT_JUMP_MODIFIER,
} from "./sidebar-inbox-jump";

beforeEach(() => {
  setJumpTargets([]);
});

describe("sidebar-inbox jump targets", () => {
  it("resolves 1-based slots to the ordered ids", () => {
    setJumpTargets(["ws-a", "ws-b", "ws-c"]);
    expect(getJumpTarget(1)).toBe("ws-a");
    expect(getJumpTarget(2)).toBe("ws-b");
    expect(getJumpTarget(3)).toBe("ws-c");
  });

  it("returns null for out-of-range, zero, and negative slots", () => {
    setJumpTargets(["ws-a"]);
    expect(getJumpTarget(2)).toBeNull();
    expect(getJumpTarget(0)).toBeNull();
    expect(getJumpTarget(-1)).toBeNull();
  });

  it("returns null once the targets are cleared (e.g. on unmount)", () => {
    setJumpTargets(["ws-a", "ws-b"]);
    setJumpTargets([]);
    expect(getJumpTarget(1)).toBeNull();
  });

  it("defaults the jump modifier to Alt (Ctrl+digit is reserved for tabs)", () => {
    expect(DEFAULT_JUMP_MODIFIER).toBe("Alt");
  });
});
