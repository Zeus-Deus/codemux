/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { FileEntry, WorkspaceSnapshot } from "@/tauri/types";

const listDirectory = vi.hoisted(() => vi.fn());
vi.mock("@/tauri/commands", () => ({
  listDirectory: (...args: unknown[]) => listDirectory(...args),
}));

import { FileTreePanel } from "./file-tree-panel";

function entry(overrides: Partial<FileEntry> & { name: string }): FileEntry {
  return {
    path: `/p/${overrides.name}`,
    is_dir: false,
    size: 290,
    is_gitignored: false,
    ...overrides,
  } as FileEntry;
}

function workspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    cwd: "/p",
    worktree_path: null,
    tabs: [],
  } as unknown as WorkspaceSnapshot;
}

beforeEach(() => {
  listDirectory.mockReset();
});

afterEach(cleanup);

describe("FileTreePanel", () => {
  // The right-aligned `290b / 17K / 4K` column was the noisiest thing on a
  // surface whose whole job is navigation. It is gone, and no code path may
  // quietly bring it back off `FileEntry.size`.
  it("shows no file-size column", async () => {
    listDirectory.mockResolvedValue([
      entry({ name: "README.md", size: 290 }),
      entry({ name: "bundle.js", size: 17408 }),
    ]);
    render(<FileTreePanel workspace={workspace()} />);

    expect(await screen.findByText("README.md")).toBeInTheDocument();
    expect(screen.queryByText(/^\d+(B|K)$/)).toBeNull();
    expect(screen.queryByText("290B")).toBeNull();
    expect(screen.queryByText("17K")).toBeNull();
  });

  it("keeps rows on the tight 22px / 5px-radius geometry", async () => {
    listDirectory.mockResolvedValue([entry({ name: "README.md" })]);
    render(<FileTreePanel workspace={workspace()} />);

    const row = await screen.findByTestId("file-tree-row");
    expect(row).toHaveClass("h-[22px]", "rounded-[5px]");
  });

  // 11px per depth level. Asserted through the rendered inline style
  // because the indent is computed, not a class.
  it("indents 11px per depth level", async () => {
    listDirectory.mockImplementation((path: string) =>
      Promise.resolve(
        path === "/p"
          ? [entry({ name: "src", path: "/p/src", is_dir: true, size: null })]
          : [entry({ name: "utils.ts", path: "/p/src/utils.ts" })],
      ),
    );
    render(<FileTreePanel workspace={workspace()} />);

    await userEvent.click(await screen.findByText("src"));
    const child = await screen.findByTestId("file-tree-row");
    expect(child).toHaveStyle({ paddingLeft: "17px" }); // 6 + 1 × 11
  });

  // With no header and no size column, the selected row is the only thing
  // tying the tree to the pane it opened.
  it("fills only the selected row", async () => {
    listDirectory.mockResolvedValue([
      entry({ name: "README.md", path: "/p/README.md" }),
      entry({ name: "AGENTS.md", path: "/p/AGENTS.md" }),
    ]);
    render(<FileTreePanel workspace={workspace()} selectedPath="/p/AGENTS.md" />);

    await screen.findByText("AGENTS.md");
    const selected = screen
      .getAllByTestId("file-tree-row")
      .filter((row) => row.dataset.selected === "true");
    expect(selected).toHaveLength(1);
    expect(selected[0]).toHaveTextContent("AGENTS.md");
    expect(selected[0]).toHaveClass("bg-foreground/8");
  });
});
