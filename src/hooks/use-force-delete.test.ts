/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";

const { mockToast } = vi.hoisted(() => ({
  mockToast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    undoable: vi.fn(),
  },
}));

vi.mock("@/lib/toast", () => ({
  toast: mockToast,
}));

import {
  USE_FORCE_PATTERN,
  runDeleteWithForceToast,
  useForceDelete,
} from "./use-force-delete";

// The exact wording the backend's dirty-worktree guard produces (pinned
// by a Rust test on the backend side).
const DIRTY_MESSAGE =
  "Worktree has 3 uncommitted change(s). Use force to override.";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("USE_FORCE_PATTERN", () => {
  it("matches the backend's dirty-worktree message case-insensitively", () => {
    expect(USE_FORCE_PATTERN.test(DIRTY_MESSAGE)).toBe(true);
    expect(
      USE_FORCE_PATTERN.test("Teardown failed: x\nUse force delete to skip teardown."),
    ).toBe(true);
    expect(USE_FORCE_PATTERN.test("worktree path is locked")).toBe(false);
  });
});

describe("useForceDelete", () => {
  it("runs non-forced first and calls onDone on success", async () => {
    const run = vi.fn().mockResolvedValue(undefined);
    const onDone = vi.fn();
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useForceDelete({ run, onDone, onError }),
    );

    await act(() => result.current.confirm());

    expect(run).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith(false);
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
    expect(result.current.forceMessage).toBeNull();
  });

  it("escalates on a /use force/i rejection: forceMessage set, no onError", async () => {
    const run = vi.fn().mockRejectedValueOnce(DIRTY_MESSAGE);
    const onDone = vi.fn();
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useForceDelete({ run, onDone, onError }),
    );

    await act(() => result.current.confirm());

    expect(result.current.forceMessage).toBe(DIRTY_MESSAGE);
    expect(onDone).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });

  it("reissues with force=true from the escalated state and hits the success path", async () => {
    const run = vi
      .fn()
      .mockRejectedValueOnce(DIRTY_MESSAGE)
      .mockResolvedValueOnce(undefined);
    const onDone = vi.fn();
    const { result } = renderHook(() => useForceDelete({ run, onDone }));

    await act(() => result.current.confirm());
    expect(result.current.forceMessage).toBe(DIRTY_MESSAGE);

    await act(() => result.current.confirm());

    expect(run).toHaveBeenLastCalledWith(true);
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("does NOT re-escalate when the FORCED run itself rejects with a matching message", async () => {
    const run = vi.fn().mockRejectedValue(DIRTY_MESSAGE);
    const onError = vi.fn();
    const { result } = renderHook(() => useForceDelete({ run, onError }));

    await act(() => result.current.confirm());
    await act(() => result.current.confirm());

    // Second (forced) failure goes to onError instead of looping.
    expect(onError).toHaveBeenCalledWith(DIRTY_MESSAGE);
  });

  it("routes non-matching rejections to onError without escalating", async () => {
    const run = vi
      .fn()
      .mockRejectedValueOnce(new Error("worktree path is locked"));
    const onDone = vi.fn();
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useForceDelete({ run, onDone, onError }),
    );

    await act(() => result.current.confirm());

    expect(result.current.forceMessage).toBeNull();
    expect(onError).toHaveBeenCalledWith("worktree path is locked");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("reset clears the escalated state", async () => {
    const run = vi.fn().mockRejectedValueOnce(DIRTY_MESSAGE);
    const { result } = renderHook(() => useForceDelete({ run }));

    await act(() => result.current.confirm());
    expect(result.current.forceMessage).toBe(DIRTY_MESSAGE);

    act(() => result.current.reset());
    expect(result.current.forceMessage).toBeNull();
  });
});

describe("runDeleteWithForceToast", () => {
  it("resolves quietly on success (no toast)", async () => {
    const run = vi.fn().mockResolvedValue(undefined);
    await runDeleteWithForceToast({ run });
    expect(run).toHaveBeenCalledWith(false);
    expect(mockToast.error).not.toHaveBeenCalled();
  });

  it("surfaces a 'Force delete' toast action on a matching rejection, which reissues with force=true", async () => {
    const run = vi
      .fn()
      .mockRejectedValueOnce(DIRTY_MESSAGE)
      .mockResolvedValueOnce(undefined);
    await runDeleteWithForceToast({ run });

    expect(mockToast.error).toHaveBeenCalledTimes(1);
    const [title, opts] = mockToast.error.mock.calls[0];
    expect(title).toBe("Delete failed");
    expect(opts.description).toBe(DIRTY_MESSAGE);
    expect(opts.action.label).toBe("Force delete");

    // Clicking the action reissues the delete with forceDelete=true.
    opts.action.onClick();
    await vi.waitFor(() => expect(run).toHaveBeenLastCalledWith(true));
    expect(run).toHaveBeenCalledTimes(2);
  });

  it("keeps a plain error toast (no action) for non-matching rejections", async () => {
    const run = vi.fn().mockRejectedValueOnce("worktree path is locked");
    await runDeleteWithForceToast({ run });

    expect(run).toHaveBeenCalledTimes(1);
    expect(mockToast.error).toHaveBeenCalledTimes(1);
    const [, opts] = mockToast.error.mock.calls[0];
    expect(opts.description).toBe("worktree path is locked");
    expect(opts.action).toBeUndefined();
  });

  it("toasts again when the forced re-run also fails", async () => {
    const run = vi
      .fn()
      .mockRejectedValueOnce(DIRTY_MESSAGE)
      .mockRejectedValueOnce(new Error("disk on fire"));
    await runDeleteWithForceToast({ run });

    const [, opts] = mockToast.error.mock.calls[0];
    opts.action.onClick();
    await vi.waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(2));
    expect(mockToast.error.mock.calls[1][1].description).toBe("disk on fire");
  });
});
