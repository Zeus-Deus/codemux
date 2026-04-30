/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  listBranchesDetailed: vi.fn(),
}));

import { DerivativeBranchPicker } from "./DerivativeBranchPicker";
import { listBranchesDetailed } from "@/tauri/commands";
import type { BranchDetail } from "@/tauri/types";

afterEach(() => cleanup());

const NOW = Math.floor(Date.now() / 1000);

function detail(
  name: string,
  overrides: Partial<BranchDetail> = {},
): BranchDetail {
  return {
    name,
    last_commit_unix: NOW - 3600,
    is_local: true,
    is_remote: false,
    ...overrides,
  };
}

function renderPicker(
  overrides: Partial<Parameters<typeof DerivativeBranchPicker>[0]> = {},
) {
  const onChange = vi.fn();
  const utils = render(
    <DerivativeBranchPicker
      projectPath="/projects/foo"
      value="main"
      onChange={onChange}
      {...overrides}
    />,
  );
  const trigger = utils.container.querySelector(
    "button",
  ) as HTMLButtonElement | null;
  return { ...utils, trigger, onChange };
}

describe("DerivativeBranchPicker", () => {
  beforeEach(() => {
    vi.mocked(listBranchesDetailed).mockReset();
  });

  it("renders the current value on the trigger pill", () => {
    // Pre-populate with the current value so the auto-correct effect
    // no-ops (we're testing display, not defaulting).
    vi.mocked(listBranchesDetailed).mockResolvedValue([detail("develop")]);
    const { trigger } = renderPicker({ value: "develop" });
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("develop");
    expect(trigger!.textContent).toContain("from");
  });

  it("disabled prop disables the trigger", () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([detail("main")]);
    const { trigger } = renderPicker({ disabled: true });
    expect(trigger!.disabled).toBe(true);
  });

  it("opens the menu and lists branches from the detailed source", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("main", { is_local: true, is_remote: true }),
      detail("feat/x", { is_local: true }),
      detail("dev", { is_local: false, is_remote: true }),
    ]);
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    await waitFor(() => {
      expect(listBranchesDetailed).toHaveBeenCalledWith("/projects/foo");
    });
    const options = await screen.findAllByRole("option");
    const labels = options.map((el) => el.textContent ?? "");
    expect(labels.some((t) => t.includes("main"))).toBe(true);
    expect(labels.some((t) => t.includes("feat/x"))).toBe(true);
    expect(labels.some((t) => t.includes("dev"))).toBe(true);
  });

  it("clicking a branch row fires onChange with that branch", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("main"),
      detail("develop"),
    ]);
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker();
    await user.click(trigger!);
    const row = await screen.findByText("develop");
    await user.click(row);
    expect(onChange).toHaveBeenCalledWith("develop");
  });

  it("fetches branches eagerly on mount and does not refetch on reopen", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([detail("main")]);
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    // Eager fetch fires on mount, not on first open. This lets the
    // auto-correct effect run before the user can submit a "+ New
    // worktree…" with the wrong default base.
    await waitFor(() => {
      expect(listBranchesDetailed).toHaveBeenCalledTimes(1);
    });
    // Open and close — should not refetch.
    await user.click(trigger!);
    await user.keyboard("{Escape}");
    await user.click(trigger!);
    await waitFor(() => {
      expect(listBranchesDetailed).toHaveBeenCalledTimes(1);
    });
  });

  it("auto-corrects the default to 'master' when seeded value ('main') doesn't exist in the repo", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("master"),
      detail("feat/x"),
    ]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith("master");
    });
  });

  it("prefers 'main' over 'master' when both are present", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("main"),
      detail("master"),
      detail("feat/x"),
    ]);
    const onChange = vi.fn();
    // Seed with a nonsense value so auto-correct triggers.
    renderPicker({ value: "__missing__", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith("main");
    });
  });

  it("falls back to the most-recent branch when neither main nor master exists", async () => {
    // listBranchesDetailed returns rows already sorted by recency, so
    // we expect the picker to fall back to branches[0].
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("feat/x", { last_commit_unix: NOW - 60 }),
      detail("other-branch", { last_commit_unix: NOW - 7200 }),
    ]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith("feat/x");
    });
  });

  it("does NOT call onChange when the seeded value already exists in the branch list", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("main"),
      detail("master"),
    ]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(listBranchesDetailed).toHaveBeenCalled();
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not call onChange when the branch list comes back empty (e.g. fresh repo)", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(listBranchesDetailed).toHaveBeenCalled();
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("handles listBranchesDetailed failure gracefully (no branches listed, no crash)", async () => {
    vi.mocked(listBranchesDetailed).mockRejectedValue(new Error("git error"));
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    // No options render, but the empty state does.
    await waitFor(() => {
      expect(screen.getByText(/No branches/i)).toBeInTheDocument();
    });
  });

  it("shows All / Worktrees tab counts and filters when Worktrees is selected", async () => {
    vi.mocked(listBranchesDetailed).mockResolvedValue([
      detail("main"),
      detail("feat/x"),
      detail("old/cleanup"),
    ]);
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    // Without a populated app store, no branches map to worktrees,
    // so the All count is 3 and the Worktrees count is 0.
    const allTab = await screen.findByRole("button", { name: /^All\s*3$/ });
    expect(allTab).toBeInTheDocument();
    const worktreesTab = screen.getByRole("button", {
      name: /^Worktrees\s*0$/,
    });
    await user.click(worktreesTab);
    expect(screen.getByText(/No active worktrees/i)).toBeInTheDocument();
  });
});
