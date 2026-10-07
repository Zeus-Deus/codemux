/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// vi.mock is hoisted — declare the fakes inside the factory rather
// than capturing module-scope variables (which aren't initialised
// yet at hoist time).
vi.mock("sonner", () => {
  const success = vi.fn(() => "toast-1");
  const error = vi.fn();
  return {
    toast: {
      success,
      error,
      info: vi.fn(),
      warning: vi.fn(),
      loading: vi.fn(),
      dismiss: vi.fn(),
    },
  };
});
vi.mock("@/lib/clipboard", () => ({
  copyToClipboard: vi.fn().mockResolvedValue(true),
  COPY_FAILED_MESSAGE: "copy failed",
}));

import { toast as sonnerToast } from "sonner";
import { copyToClipboard } from "@/lib/clipboard";
import { errorMessage, fireUndoable, reportFailure, toast } from "./toast";

const mockSuccess = sonnerToast.success as unknown as ReturnType<typeof vi.fn>;
const mockError = sonnerToast.error as unknown as ReturnType<typeof vi.fn>;
const mockLoading = sonnerToast.loading as unknown as ReturnType<typeof vi.fn>;
const mockDismiss = sonnerToast.dismiss as unknown as ReturnType<typeof vi.fn>;

/** The slice of the click event Sonner hands an action's onClick. */
function clickEvent() {
  return { preventDefault: vi.fn() };
}

afterEach(() => {
  mockSuccess.mockClear();
  mockError.mockClear();
  mockLoading.mockClear();
  mockDismiss.mockClear();
});

describe("fireUndoable", () => {
  beforeEach(() => {
    mockSuccess.mockClear();
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
    opts.action.onClick(clickEvent());
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
    opts.action.onClick(clickEvent());
    expect(onUndo).toHaveBeenCalledTimes(1);
    // Second click WHILE the first is still running — must be a no-op.
    opts.action.onClick(clickEvent());
    expect(onUndo).toHaveBeenCalledTimes(1);
    // Resolve the first one — second click STILL must not fire.
    resolveFn();
    await Promise.resolve();
    opts.action.onClick(clickEvent());
    expect(onUndo).toHaveBeenCalledTimes(1);
  });

  it("surfaces 'Undo failed' in place when the reverse action throws", async () => {
    const onUndo = vi.fn().mockRejectedValue(new Error("boom"));
    fireUndoable({ message: "x", onUndo });
    const [, opts] = mockSuccess.mock.calls[0];
    opts.action.onClick(clickEvent());
    // Wait one microtask for the promise rejection to surface.
    await Promise.resolve();
    await Promise.resolve();
    expect(mockError).toHaveBeenCalledWith(
      "Undo failed",
      expect.objectContaining({ id: "toast-1", description: "boom" }),
    );
  });

  it("keeps the toast and shows progress while the reverse action runs", async () => {
    let resolveFn: () => void = () => {};
    const onUndo = vi.fn(
      () =>
        new Promise<void>((res) => {
          resolveFn = res;
        }),
    );
    fireUndoable({ message: "Pushed", onUndo });
    const [, opts] = mockSuccess.mock.calls[0];
    const event = clickEvent();
    opts.action.onClick(event);
    // Sonner deletes an action's toast unless the click is default-prevented.
    expect(event.preventDefault).toHaveBeenCalled();
    expect(mockLoading).toHaveBeenCalledWith(
      "Undoing…",
      expect.objectContaining({ id: "toast-1", action: undefined }),
    );
    expect(mockDismiss).not.toHaveBeenCalled();
    resolveFn();
    await Promise.resolve();
    await Promise.resolve();
    expect(mockDismiss).toHaveBeenCalledWith("toast-1");
  });

  it("carries its window length for the draining bar", () => {
    fireUndoable({ message: "x", onUndo: () => Promise.resolve(), durationMs: 15_000 });
    const [, opts] = mockSuccess.mock.calls[0];
    expect(opts.className).toBe("cn-toast-undo");
    expect(opts.style["--toast-undo-duration"]).toBe("15000ms");
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

describe("errorMessage", () => {
  it("drops the Error prefix that String() adds", () => {
    expect(errorMessage(new Error("fatal: not a git repository"))).toBe(
      "fatal: not a git repository",
    );
    expect(errorMessage("Error: Command failed: git push")).toBe("Command failed: git push");
    expect(errorMessage("TypeError: x is undefined")).toBe("x is undefined");
  });

  it("keeps a Tauri string rejection as is", () => {
    const message = "No run command configured. Set one in Settings > Projects.";
    expect(errorMessage(message)).toBe(message);
  });

  it("reads message-shaped objects and never returns an empty string", () => {
    expect(errorMessage({ message: "denied" })).toBe("denied");
    expect(errorMessage("")).toBe("Unknown error");
  });
});

describe("toast.failure", () => {
  it("puts the human title first and the error in a closable, clamped description", () => {
    toast.failure("Couldn't push the branch", new Error("rejected"));
    const [title, opts] = mockError.mock.calls[0];
    expect(title).toBe("Couldn't push the branch");
    expect(opts.description).toBe("rejected");
    expect(opts.closeButton).toBe(true);
    expect(opts.classNames.description).toContain("line-clamp-3");
  });

  it("copies the full error without closing the toast", () => {
    toast.failure("Couldn't push", "long git output");
    const [, opts] = mockError.mock.calls[0];
    const event = clickEvent();
    opts.action.onClick(event);
    expect(event.preventDefault).toHaveBeenCalled();
    expect(copyToClipboard).toHaveBeenCalledWith("long git output");
  });

  it("gives every error toast a close button", () => {
    toast.error("Something failed");
    expect(mockError.mock.calls[0][1].closeButton).toBe(true);
  });
});

describe("reportFailure", () => {
  it("turns a silent catch into a titled error toast", () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    reportFailure("Couldn't run the dev command")("No run command configured.");
    expect(mockError).toHaveBeenCalledWith(
      "Couldn't run the dev command",
      expect.objectContaining({ description: "No run command configured." }),
    );
    expect(log).toHaveBeenCalled();
    log.mockRestore();
  });
});
