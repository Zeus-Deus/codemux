/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import { runSweep, SweepDialog, type SweepProgress } from "./sweep-dialog";
import type { SweepCandidate } from "./use-sweep-candidates";

// ── Mocks ───────────────────────────────────────────────────────

const h = vi.hoisted(() => ({
  /** In-flight removals, oldest first; each test settles them by hand. */
  pending: [] as { id: string; resolve: () => void; reject: (err: unknown) => void }[],
  close: vi.fn(),
  evict: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  closeWorkspaceWithWorktree: (...args: unknown[]) => {
    h.close(...args);
    return new Promise<void>((resolve, reject) => {
      h.pending.push({ id: String(args[0]), resolve, reject });
    });
  },
}));

vi.mock("./use-sweep-candidates", () => ({
  evictWorktreeSizes: (ids: string[]) => h.evict(ids),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    success: (...args: unknown[]) => h.toastSuccess(...args),
    warning: (...args: unknown[]) => h.toastWarning(...args),
  },
}));

// ── Helpers ─────────────────────────────────────────────────────

type Result = "ok" | "dirty" | "broken";

const REJECTION: Record<Exclude<Result, "ok">, string> = {
  dirty: "Worktree has 2 uncommitted change(s). Use force to override.",
  broken: "teardown script exited with status 1",
};

function makeCandidates(ids: string[]): SweepCandidate[] {
  return ids.map((id) => ({ id, title: `feat/${id}`, bytes: 100 }));
}

/** Wait for the next removal to be in flight, then answer it. */
async function answerNext(result: Result): Promise<void> {
  await vi.waitFor(() => {
    if (h.pending.length === 0) throw new Error("no removal in flight");
  });
  const next = h.pending.shift()!;
  if (result === "ok") next.resolve();
  else next.reject(REJECTION[result]);
}

async function answerNextInDialog(result: Result): Promise<void> {
  await act(async () => {
    await answerNext(result);
  });
}

beforeEach(() => {
  h.pending.length = 0;
  vi.clearAllMocks();
});

afterEach(cleanup);

// ── runSweep ────────────────────────────────────────────────────

describe("runSweep", () => {
  it("reports running totals against a denominator fixed at entry", async () => {
    const list = makeCandidates(["a", "b", "c"]);
    const seen: SweepProgress[] = [];
    const done = runSweep(list, (p) => seen.push(p));

    await answerNext("ok");
    await answerNext("dirty");
    await answerNext("broken");
    const outcome = await done;

    expect(h.close.mock.calls).toEqual([
      ["a", true, false, false],
      ["b", true, false, false],
      ["c", true, false, false],
    ]);
    expect(seen.map((p) => [p.index, p.total, p.title])).toEqual([
      [1, 3, "feat/a"],
      [2, 3, "feat/b"],
      [3, 3, "feat/c"],
    ]);
    expect(seen[1].outcome).toMatchObject({ closed: 1, skipped: 1, failed: 0, freedBytes: 100 });
    expect(outcome).toMatchObject({ closed: 1, skipped: 1, failed: 1, freedBytes: 100 });
    expect(outcome.failures).toEqual([`feat/c: ${REJECTION.broken}`]);
    expect(h.evict).toHaveBeenCalledWith(["a"]);
  });

  it("stops between candidates without interrupting the one in flight", async () => {
    const list = makeCandidates(["a", "b", "c"]);
    const controller = new AbortController();
    const done = runSweep(list, undefined, controller.signal);

    await vi.waitFor(() => expect(h.pending).toHaveLength(1));
    controller.abort();
    await answerNext("ok");
    const outcome = await done;

    expect(h.close).toHaveBeenCalledTimes(1);
    expect(outcome).toMatchObject({ closed: 1, skipped: 0, failed: 0 });
    expect(h.evict).toHaveBeenCalledWith(["a"]);
  });
});

// ── SweepDialog ─────────────────────────────────────────────────

describe("SweepDialog", () => {
  it("keeps the job line and denominator still while the numbers move", async () => {
    const onOpenChange = vi.fn();
    const list = makeCandidates(["a", "b", "c"]);
    const { rerender } = render(
      <SweepDialog open onOpenChange={onOpenChange} candidates={list} knownBytes={300} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Sweep" }));

    expect(screen.getByText("Sweeping settled workspaces")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getByText("of 3 left")).toBeInTheDocument();
    expect(screen.getByText("removing feat/a")).toBeInTheDocument();

    await answerNextInDialog("ok");
    // The settled shelf shrinks under the dialog as worktrees go.
    rerender(
      <SweepDialog
        open
        onOpenChange={onOpenChange}
        candidates={list.slice(1)}
        knownBytes={200}
      />,
    );
    expect(screen.getByText("Sweeping settled workspaces")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
    expect(screen.getByText("of 3 left")).toBeInTheDocument();
    expect(screen.getByText("100 B")).toBeInTheDocument();
    expect(screen.getByText("removing feat/b")).toBeInTheDocument();
    // Nothing skipped yet, so no skipped count at all — not even a zero.
    expect(screen.queryByText(/\d+ skipped/)).toBeNull();

    await answerNextInDialog("dirty");
    expect(screen.getByText("1 skipped")).toBeInTheDocument();

    await answerNextInDialog("ok");
    expect(await screen.findByText("Sweep complete")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByText("of 3 left")).toBeInTheDocument();
    expect(screen.getByText("1 held uncommitted or unpushed work")).toBeInTheDocument();
    expect(screen.queryByText("1 skipped")).toBeNull();
    // The dialog is the receipt, so no toast repeats it.
    expect(h.toastSuccess).not.toHaveBeenCalled();
    expect(h.toastWarning).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("says nothing was skipped when every worktree went", async () => {
    render(
      <SweepDialog
        open
        onOpenChange={vi.fn()}
        candidates={makeCandidates(["a"])}
        knownBytes={100}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Sweep" }));
    await answerNextInDialog("ok");

    expect(await screen.findByText("Sweep complete")).toBeInTheDocument();
    expect(screen.getByText("Nothing was skipped")).toBeInTheDocument();
  });

  it("stops after the in-flight removal and shows the partial receipt", async () => {
    render(
      <SweepDialog
        open
        onOpenChange={vi.fn()}
        candidates={makeCandidates(["a", "b", "c"])}
        knownBytes={300}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Sweep" }));
    await vi.waitFor(() => expect(h.pending).toHaveLength(1));

    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(screen.getByRole("button", { name: "Stopping" })).toBeDisabled();
    await answerNextInDialog("ok");

    expect(await screen.findByText("Sweep stopped")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
    expect(h.close).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
  });

  it("keeps the warning toast for failures, which need their error bodies", async () => {
    render(
      <SweepDialog
        open
        onOpenChange={vi.fn()}
        candidates={makeCandidates(["a"])}
        knownBytes={100}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Sweep" }));
    await answerNextInDialog("broken");

    expect(await screen.findByText("Sweep complete")).toBeInTheDocument();
    expect(screen.getByText("Nothing was skipped · 1 failed")).toBeInTheDocument();
    expect(h.toastWarning).toHaveBeenCalledWith("Nothing removed · 1 failed", {
      description: `feat/a: ${REJECTION.broken}`,
    });
  });
});
