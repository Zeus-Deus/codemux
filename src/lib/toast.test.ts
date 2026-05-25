/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// vi.mock is hoisted — declare the fakes inside the factory rather
// than capturing module-scope variables (which aren't initialised
// yet at hoist time).
vi.mock("sonner", () => {
  const success = vi.fn();
  const error = vi.fn();
  return {
    toast: {
      success,
      error,
      info: vi.fn(),
      warning: vi.fn(),
      dismiss: vi.fn(),
    },
  };
});

import { toast as sonnerToast } from "sonner";
import { fireUndoable } from "./toast";

const mockSuccess = sonnerToast.success as unknown as ReturnType<typeof vi.fn>;
const mockError = sonnerToast.error as unknown as ReturnType<typeof vi.fn>;

afterEach(() => {
  mockSuccess.mockClear();
  mockError.mockClear();
});

describe("fireUndoable", () => {
  beforeEach(() => {
    mockSuccess.mockReset();
    mockError.mockReset();
  });

  it("emits a success toast with the configured headline + description", () => {
    fireUndoable({
      message: "Pushed to homedesk",
      description: "Tap Undo within 10s",
      onUndo: () => Promise.resolve(),
    });
    expect(mockSuccess).toHaveBeenCalledTimes(1);
    const [headline, opts] = mockSuccess.mock.calls[0];
    expect(headline).toBe("Pushed to homedesk");
    expect(opts.description).toBe("Tap Undo within 10s");
    expect(opts.action.label).toBe("Undo");
  });

  it("invokes the reverse-action closure when the Undo action fires", async () => {
    const onUndo = vi.fn().mockResolvedValue(undefined);
    fireUndoable({
      message: "Pushed",
      onUndo,
    });
    const [, opts] = mockSuccess.mock.calls[0];
    opts.action.onClick();
    expect(onUndo).toHaveBeenCalledTimes(1);
  });

  it("guards against double-clicks on the undo button", async () => {
    let resolveFn: () => void = () => {};
    const onUndo = vi.fn(
      () =>
        new Promise<void>((res) => {
          resolveFn = res;
        }),
    );
    fireUndoable({ message: "x", onUndo });
    const [, opts] = mockSuccess.mock.calls[0];
    // First click — starts the reverse action.
    opts.action.onClick();
    expect(onUndo).toHaveBeenCalledTimes(1);
    // Second click WHILE the first is still running — must be a no-op.
    opts.action.onClick();
    expect(onUndo).toHaveBeenCalledTimes(1);
    // Resolve the first one — second click STILL must not fire.
    resolveFn();
    await Promise.resolve();
    opts.action.onClick();
    expect(onUndo).toHaveBeenCalledTimes(1);
  });

  it("surfaces a follow-up error toast when the reverse action throws", async () => {
    const onUndo = vi.fn().mockRejectedValue(new Error("boom"));
    fireUndoable({ message: "x", onUndo });
    const [, opts] = mockSuccess.mock.calls[0];
    opts.action.onClick();
    // Wait one microtask for the promise rejection to surface.
    await Promise.resolve();
    await Promise.resolve();
    expect(mockError).toHaveBeenCalledWith(
      "Undo failed",
      expect.objectContaining({ description: "boom" }),
    );
  });

  it("clamps duration to [3000, 60000]", () => {
    fireUndoable({ message: "x", onUndo: () => Promise.resolve(), durationMs: 1 });
    expect(mockSuccess.mock.calls[0][1].duration).toBe(3000);
    mockSuccess.mockClear();
    fireUndoable({ message: "x", onUndo: () => Promise.resolve(), durationMs: 1_000_000 });
    expect(mockSuccess.mock.calls[0][1].duration).toBe(60000);
  });

  it("defaults duration to 10 seconds when not specified", () => {
    fireUndoable({ message: "x", onUndo: () => Promise.resolve() });
    expect(mockSuccess.mock.calls[0][1].duration).toBe(10_000);
  });
});
