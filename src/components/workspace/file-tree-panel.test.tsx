/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StrictMode } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { FileEntry, WorkspaceSnapshot } from "@/tauri/types";

const listDirectory = vi.hoisted(() => vi.fn());
vi.mock("@/tauri/commands", () => ({
  listDirectory: (...args: unknown[]) => listDirectory(...args),
}));

import { FileTreePanel } from "./file-tree-panel";
import { DEFAULT_SETTINGS, useSyncedSettingsStore } from "@/stores/synced-settings-store";

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

function deferredDirectory() {
  let resolve!: (entries: FileEntry[]) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<FileEntry[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function setShowHidden(showHidden: boolean) {
  useSyncedSettingsStore.setState({
    settings: { ...DEFAULT_SETTINGS, file_tree: { show_hidden_files: showHidden } },
  });
}

beforeEach(() => {
  listDirectory.mockReset();
  setShowHidden(false);
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

  it("keeps rows on the tight 22px geometry with a rounded row", async () => {
    listDirectory.mockResolvedValue([entry({ name: "README.md" })]);
    render(<FileTreePanel workspace={workspace()} />);

    const row = await screen.findByTestId("file-tree-row");
    expect(row).toHaveClass("h-[22px]");
    // The radius value belongs to the token ladder; that the row is
    // rounded at all is this test's business.
    expect(row.className).toMatch(/\brounded/);
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
  });

  it.each(["Refresh", "show-hidden"])("keeps current-root rows while %s is pending", async (reload) => {
    const pending = deferredDirectory();
    listDirectory.mockResolvedValueOnce([entry({ name: "README.md" })])
      .mockReturnValueOnce(pending.promise);
    const view = render(<FileTreePanel workspace={workspace()} />);
    await screen.findByText("README.md");

    if (reload === "Refresh") {
      view.rerender(<FileTreePanel workspace={workspace()} refreshKey={1} />);
    } else {
      act(() => setShowHidden(true));
    }

    expect(listDirectory).toHaveBeenLastCalledWith("/p", reload === "show-hidden");
    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.queryByText("No files")).toBeNull();

    await act(async () => pending.resolve([entry({ name: "current.ts" })]));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByText("README.md")).toBeNull();
  });

  it.each(["/next", ""])("clears old-root rows when cwd becomes %s", async (cwd) => {
    const pending = deferredDirectory();
    listDirectory.mockResolvedValueOnce([entry({ name: "previous.ts" })])
      .mockReturnValueOnce(pending.promise);
    const view = render(<FileTreePanel workspace={workspace()} />);
    await screen.findByText("previous.ts");

    view.rerender(<FileTreePanel workspace={{ ...workspace(), cwd }} />);
    expect(screen.queryByText("previous.ts")).toBeNull();
    expect(listDirectory).toHaveBeenCalledTimes(cwd ? 2 : 1);
    if (cwd) {
      await act(async () => pending.resolve([entry({ name: "current.ts", path: `${cwd}/current.ts` })]));
      expect(screen.getByText("current.ts")).toBeInTheDocument();
    }
  });

  it("ignores a pending root read after cwd is cleared", async () => {
    const pending = deferredDirectory();
    listDirectory.mockReturnValueOnce(pending.promise);
    const view = render(<FileTreePanel workspace={workspace()} />);
    view.rerender(<FileTreePanel workspace={{ ...workspace(), cwd: "" }} />);

    await act(async () => pending.resolve([entry({ name: "previous.ts" })]));
    expect(screen.queryByText("previous.ts")).toBeNull();
  });

  it.each(["Refresh", "show-hidden"])("ignores a successful root read superseded by %s", async (reload) => {
    const previous = deferredDirectory();
    listDirectory.mockReturnValueOnce(previous.promise)
      .mockResolvedValueOnce([entry({ name: "current.ts" })]);
    const view = render(<FileTreePanel workspace={workspace()} />);
    if (reload === "Refresh") {
      view.rerender(<FileTreePanel workspace={workspace()} refreshKey={1} />);
    } else {
      act(() => setShowHidden(true));
    }
    await screen.findByText("current.ts");

    await act(async () => previous.resolve([entry({ name: "previous.ts" })]));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByText("previous.ts")).toBeNull();
  });

  it("ignores the root read from a cleaned-up Strict Mode effect", async () => {
    const previous = deferredDirectory();
    listDirectory.mockReturnValueOnce(previous.promise)
      .mockResolvedValueOnce([entry({ name: "current.ts" })]);
    render(<StrictMode><FileTreePanel workspace={workspace()} /></StrictMode>);
    await screen.findByText("current.ts");

    await act(async () => previous.resolve([entry({ name: "previous.ts" })]));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByText("previous.ts")).toBeNull();
  });

  it("ignores a previous workspace's slow root read", async () => {
    const previous = deferredDirectory();
    listDirectory.mockReturnValueOnce(previous.promise)
      .mockResolvedValueOnce([entry({ name: "current.ts", path: "/next/current.ts" })]);
    const view = render(<FileTreePanel workspace={workspace()} />);
    view.rerender(<FileTreePanel workspace={{ ...workspace(), cwd: "/next" }} />);
    await screen.findByText("current.ts");

    await act(async () => previous.resolve([entry({ name: "previous.ts" })]));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByText("previous.ts")).toBeNull();
  });

  it("ignores an earlier root read after returning to the same worktree", async () => {
    const previous = deferredDirectory();
    const next = deferredDirectory();
    listDirectory.mockReturnValueOnce(previous.promise)
      .mockReturnValueOnce(next.promise)
      .mockResolvedValueOnce([entry({ name: "current.ts" })]);
    const view = render(<FileTreePanel workspace={workspace()} />);
    view.rerender(<FileTreePanel workspace={{ ...workspace(), worktree_path: "/next" }} />);
    view.rerender(<FileTreePanel workspace={workspace()} />);
    await screen.findByText("current.ts");

    await act(async () => {
      previous.resolve([entry({ name: "previous.ts" })]);
      next.reject(new Error("intermediate worktree read failed"));
    });
    expect(listDirectory.mock.calls.map(([path]) => path)).toEqual(["/p", "/next", "/p"]);
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByText("previous.ts")).toBeNull();
  });

  it.each(["Refresh", "show-hidden"])("ignores a failed root read superseded by %s", async (reload) => {
    const previous = deferredDirectory();
    listDirectory.mockReturnValueOnce(previous.promise)
      .mockResolvedValueOnce([entry({ name: "current.ts" })]);
    const view = render(<FileTreePanel workspace={workspace()} />);
    if (reload === "Refresh") {
      view.rerender(<FileTreePanel workspace={workspace()} refreshKey={1} />);
    } else {
      act(() => setShowHidden(true));
    }
    await screen.findByText("current.ts");

    await act(async () => previous.reject(new Error("old read failed")));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
  });

  it("shares a pending directory read across collapse and re-expansion", async () => {
    const children = deferredDirectory();
    listDirectory.mockResolvedValueOnce([entry({ name: "src", is_dir: true })])
      .mockReturnValue(children.promise);
    render(<FileTreePanel workspace={workspace()} />);
    const folder = await screen.findByRole("button", { name: /^src/ });

    await userEvent.click(folder);
    await userEvent.click(folder);
    await userEvent.click(folder);
    expect(listDirectory).toHaveBeenCalledTimes(2);

    await act(async () => children.resolve([entry({ name: "child.ts", path: "/p/src/child.ts" })]));
    expect(screen.getByText("child.ts")).toBeInTheDocument();
    await userEvent.click(folder);
    await userEvent.click(folder);
    expect(listDirectory).toHaveBeenCalledTimes(2);
  });

  it.each(["success", "failure"])("does not let an old child read's %s populate or unlock the refreshed tree", async (outcome) => {
    const previous = deferredDirectory();
    const current = deferredDirectory();
    const root = [entry({ name: "src", is_dir: true })];
    listDirectory.mockResolvedValueOnce(root).mockReturnValueOnce(previous.promise)
      .mockResolvedValueOnce(root).mockReturnValueOnce(current.promise);
    const view = render(<FileTreePanel workspace={workspace()} />);
    await userEvent.click(await screen.findByRole("button", { name: /^src/ }));
    view.rerender(<FileTreePanel workspace={workspace()} refreshKey={1} />);
    const folder = await screen.findByRole("button", { name: /^src/ });
    await userEvent.click(folder);

    await act(async () => {
      if (outcome === "success") previous.resolve([entry({ name: "stale.ts" })]);
      else previous.reject(new Error("old child read failed"));
    });
    expect(screen.queryByText("stale.ts")).toBeNull();
    expect(screen.queryByText("empty")).toBeNull();
    expect(screen.getByRole("status", { name: "Loading" })).toBeInTheDocument();
    await userEvent.click(folder);
    await userEvent.click(folder);
    expect(listDirectory).toHaveBeenCalledTimes(4);

    await act(async () => current.resolve([entry({ name: "current.ts" })]));
    expect(screen.getByText("current.ts")).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Loading" })).toBeNull();
  });
});
