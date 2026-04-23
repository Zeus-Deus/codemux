/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  listBranches: vi.fn(),
}));

import { DerivativeBranchPicker } from "./DerivativeBranchPicker";
import { listBranches } from "@/tauri/commands";

afterEach(() => cleanup());

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
    vi.mocked(listBranches).mockReset();
  });

  it("renders the current value on the trigger pill", () => {
    // Pre-populate with the current value so the auto-correct effect
    // no-ops (we're testing display, not defaulting).
    vi.mocked(listBranches).mockResolvedValue(["develop"]);
    const { trigger } = renderPicker({ value: "develop" });
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("develop");
    expect(trigger!.textContent).toContain("from");
  });

  it("disabled prop disables the trigger", () => {
    vi.mocked(listBranches).mockResolvedValue(["main"]);
    const { trigger } = renderPicker({ disabled: true });
    expect(trigger!.disabled).toBe(true);
  });

  it("opens the menu and lists branches (local + remote deduped, origin/ stripped)", async () => {
    // First call → local; second → remote. The picker dedupes the two
    // sets and strips the `origin/` prefix from remotes.
    vi.mocked(listBranches).mockImplementation(async (_path, remote) =>
      remote ? ["origin/main", "origin/dev"] : ["main", "feat/x"],
    );
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/projects/foo", false);
    });
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/projects/foo", true);
    });
    const options = await screen.findAllByRole("option");
    const labels = options.map((el) => el.textContent ?? "");
    // 3 unique: main, feat/x, dev (origin/main de-duped with local main)
    expect(labels.some((t) => t.includes("main"))).toBe(true);
    expect(labels.some((t) => t.includes("feat/x"))).toBe(true);
    expect(labels.some((t) => t.includes("dev"))).toBe(true);
    // No `origin/` prefix leaking through.
    expect(labels.some((t) => t.includes("origin/"))).toBe(false);
  });

  it("clicking a branch row fires onChange with that branch", async () => {
    vi.mocked(listBranches).mockResolvedValue(["main", "develop"]);
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker();
    await user.click(trigger!);
    const row = await screen.findByText("develop");
    await user.click(row);
    expect(onChange).toHaveBeenCalledWith("develop");
  });

  it("fetches branches eagerly on mount and does not refetch on reopen", async () => {
    vi.mocked(listBranches).mockResolvedValue(["main"]);
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    // Eager fetch fires on mount, not on first open. This lets the
    // auto-correct effect run before the user can submit a "+ New
    // worktree…" with the wrong default base.
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2);
    });
    // Open and close — should not refetch.
    await user.click(trigger!);
    await user.keyboard("{Escape}");
    await user.click(trigger!);
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2);
    });
  });

  it("auto-corrects the default to 'master' when seeded value ('main') doesn't exist in the repo", async () => {
    vi.mocked(listBranches).mockImplementation(async (_path, remote) =>
      remote ? [] : ["master", "feat/x"],
    );
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith("master");
    });
  });

  it("prefers 'main' over 'master' when both are present", async () => {
    vi.mocked(listBranches).mockResolvedValue(["main", "master", "feat/x"]);
    const onChange = vi.fn();
    // Seed with a nonsense value so auto-correct triggers.
    renderPicker({ value: "__missing__", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith("main");
    });
  });

  it("falls back to the first available branch when neither main nor master exists", async () => {
    vi.mocked(listBranches).mockResolvedValue(["feat/x", "other-branch"]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(onChange).toHaveBeenCalled();
    });
    // Sorted list → "feat/x" is alphabetically first.
    expect(onChange).toHaveBeenCalledWith("feat/x");
  });

  it("does NOT call onChange when the seeded value already exists in the branch list", async () => {
    vi.mocked(listBranches).mockResolvedValue(["main", "master"]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    // Wait long enough for a spurious call to land.
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalled();
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not call onChange when the branch list comes back empty (e.g. fresh repo)", async () => {
    vi.mocked(listBranches).mockResolvedValue([]);
    const onChange = vi.fn();
    renderPicker({ value: "main", onChange });
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalled();
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("handles listBranches failure gracefully (no branches listed, no crash)", async () => {
    vi.mocked(listBranches).mockRejectedValue(new Error("git error"));
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    // No options render, but the empty state does.
    await waitFor(() => {
      expect(screen.getByText(/No branches/i)).toBeInTheDocument();
    });
  });
});
