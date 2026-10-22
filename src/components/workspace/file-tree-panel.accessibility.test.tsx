/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FileEntry, WorkspaceSnapshot } from "@/tauri/types";
import { DEFAULT_SETTINGS, useSyncedSettingsStore } from "@/stores/synced-settings-store";

const { listDirectory, openEditorTab } = vi.hoisted(() => ({
  listDirectory: vi.fn(),
  openEditorTab: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/tauri/commands", () => ({ listDirectory }));
vi.mock("@/lib/open-editor-tab", () => ({ openEditorTab }));
import { FileTreePanel } from "./file-tree-panel";

const workspace: WorkspaceSnapshot = {
  workspace_id: "ws-1", title: "Project", workspace_type: "standard", cwd: "/p",
  worktree_path: null, project_root: "/p", git_branch: "main", git_ahead: 0,
  git_behind: 0, git_additions: 0, git_deletions: 0, git_changed_files: 0,
  notification_count: 0, latest_agent_state: null, pr_number: null,
  pr_state: null, pr_url: null, linked_issue: null, notifications_muted: false,
  tabs: [], active_tab_id: "", active_surface_id: "", surfaces: [],
};
function entry(path: string, is_dir = false): FileEntry {
  return { path, name: path.slice(path.lastIndexOf("/") + 1), is_dir, size: null, is_gitignored: false };
}
const roots = [entry("/p/src", true), entry("/p/docs", true), entry("/p/README.md")];
const listings: Record<string, FileEntry[]> = {
  "/p": roots,
  "/p/src": [entry("/p/src/lib", true), entry("/p/src/main.ts")],
  "/p/src/lib": [entry("/p/src/lib/utils.ts")],
  "/p/docs": [],
};
function activeItem(tree = screen.getByRole("tree", { name: "Files" })) {
  const id = tree.getAttribute("aria-activedescendant");
  const item = id ? document.getElementById(id) : null;
  expect(item).not.toBeNull();
  expect(tree).toContainElement(item);
  return item!;
}
function setShowHidden(show_hidden_files: boolean) {
  useSyncedSettingsStore.setState({
    settings: { ...DEFAULT_SETTINGS, file_tree: { show_hidden_files } },
  });
}
const originalSettings = useSyncedSettingsStore.getState().settings;
beforeEach(() => {
  listDirectory.mockReset().mockImplementation((path: string) => Promise.resolve(listings[path] ?? []));
  openEditorTab.mockClear();
  setShowHidden(false);
});
afterEach(() => {
  cleanup();
  useSyncedSettingsStore.setState({ settings: originalSettings });
});

describe("FileTreePanel accessibility", () => {
  it("exposes named nested tree items, groups, expansion and controlled file selection", async () => {
    const user = userEvent.setup();
    render(<FileTreePanel workspace={workspace} selectedPath="/p/README.md" />);
    const folder = await screen.findByRole("treeitem", { name: "src" });
    const file = screen.getByRole("treeitem", { name: "README.md" });
    expect(folder).toHaveAttribute("aria-expanded", "false");
    expect(folder).not.toHaveAttribute("aria-selected");
    expect(file).not.toHaveAttribute("aria-expanded");
    expect(file).toHaveAttribute("aria-selected", "true");
    await user.click(within(folder).getByText("src"));
    expect(folder).toHaveAttribute("aria-expanded", "true");
    const group = within(folder).getByRole("group");
    expect(within(group).getByRole("treeitem", { name: "main.ts" })).toHaveAttribute("aria-selected", "false");
    expect(folder).toHaveAccessibleName("src");
    await user.click(within(folder).getByText("src"));
    expect(folder).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.queryByRole("treeitem", { name: "main.ts" })).toBeNull();
  });

  it("uses one tab stop, navigates visible nodes without wrapping, and keeps selection separate", async () => {
    const user = userEvent.setup();
    const onOpenFile = vi.fn();
    const view = render(<><FileTreePanel workspace={workspace} onOpenFile={onOpenFile} /><button>After tree</button></>);
    await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    const tree = screen.getByRole("tree", { name: "Files" });
    expect(tree).toHaveFocus();
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{ArrowUp}{Home}{ArrowLeft}");
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("lib");
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("utils.ts");
    await user.keyboard("{ArrowDown}");
    expect(activeItem()).toHaveAccessibleName("main.ts");
    await user.keyboard("{ArrowUp}");
    expect(activeItem()).toHaveAccessibleName("utils.ts");
    await user.keyboard("{ArrowLeft}");
    expect(activeItem()).toHaveAccessibleName("lib");
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("treeitem", { name: "lib" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: "utils.ts" })).toBeNull();
    await user.keyboard("{End}{ArrowDown}{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("README.md");
    expect(tree).toHaveFocus();
    expect(onOpenFile).not.toHaveBeenCalled();
    view.rerender(<><FileTreePanel workspace={workspace} onOpenFile={onOpenFile} selectedPath="/p/src/main.ts" /><button>After tree</button></>);
    expect(activeItem()).toHaveAccessibleName("README.md");
    expect(screen.getByRole("treeitem", { name: "main.ts" })).toHaveAttribute("aria-selected", "true");
    await user.tab();
    expect(screen.getByRole("button", { name: "After tree" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(tree).toHaveFocus();
    expect(activeItem()).toHaveAccessibleName("README.md");
  });

  it("starts at the visible selected file and skips cached descendants of collapsed folders", async () => {
    const user = userEvent.setup();
    render(<FileTreePanel workspace={workspace} selectedPath="/p/README.md" />);
    const src = await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    expect(activeItem()).toHaveAccessibleName("README.md");
    await user.click(within(src).getByText("src"));
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("utils.ts");
    // Collapsing with the mouse returns the active descendant to its parent.
    await user.click(within(src).getByText("src"));
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{ArrowDown}");
    expect(activeItem()).toHaveAccessibleName("docs");
    await user.keyboard("{Home}{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("lib");
  });

  it.each(["empty", "failure"])("keeps loading and %s folders expanded without navigating captions", async (outcome) => {
    const user = userEvent.setup();
    let resolve!: (entries: FileEntry[]) => void;
    let reject!: (error: Error) => void;
    const pending = new Promise<FileEntry[]>((res, rej) => { resolve = res; reject = rej; });
    listDirectory.mockImplementation((path: string) => path === "/p" ? Promise.resolve(roots) : pending);
    render(<FileTreePanel workspace={workspace} />);
    const src = await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(src).toHaveAttribute("aria-expanded", "true");
    expect(activeItem()).toBe(src);
    expect(listDirectory).toHaveBeenCalledTimes(2);
    await act(async () => {
      if (outcome === "empty") resolve([]);
      else reject(new Error("unreadable"));
    });
    expect(within(src).getByText("empty")).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(activeItem()).toBe(src);
    await user.keyboard("{ArrowDown}");
    expect(activeItem()).toHaveAccessibleName("docs");
  });

  it.each([false, true])("activates files once by mouse, Enter and Space (custom handler: %s)", async (custom) => {
    const user = userEvent.setup();
    const onOpenFile = vi.fn();
    render(<FileTreePanel workspace={workspace} onOpenFile={custom ? onOpenFile : undefined} />);
    const file = await screen.findByRole("treeitem", { name: "README.md" });
    await user.click(within(file).getByText("README.md"));
    await user.keyboard("{Enter} ");
    const handler = custom ? onOpenFile : openEditorTab;
    expect(handler).toHaveBeenCalledTimes(3);
    if (custom) expect(onOpenFile).toHaveBeenLastCalledWith("/p/README.md");
    else expect(openEditorTab).toHaveBeenLastCalledWith("ws-1", workspace.tabs, "/p/README.md");
    expect(custom ? openEditorTab : onOpenFile).not.toHaveBeenCalled();
  });

  it("uses Enter and Space to toggle folders once", async () => {
    const user = userEvent.setup();
    render(<FileTreePanel workspace={workspace} />);
    const folder = await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    await user.keyboard("{Enter}");
    expect(folder).toHaveAttribute("aria-expanded", "true");
    await user.keyboard(" ");
    expect(folder).toHaveAttribute("aria-expanded", "false");
  });

  it("accepts assistive item activation without toggling ancestors or repeating file activation", async () => {
    const onOpenFile = vi.fn();
    render(<FileTreePanel workspace={workspace} onOpenFile={onOpenFile} />);
    const src = await screen.findByRole("treeitem", { name: "src" });
    fireEvent.click(src);
    const file = await screen.findByRole("treeitem", { name: "main.ts" });
    fireEvent.click(file);
    expect(src).toHaveAttribute("aria-expanded", "true");
    expect(onOpenFile).toHaveBeenCalledExactlyOnceWith("/p/src/main.ts");
    fireEvent.keyDown(screen.getByRole("tree"), { key: "Enter", repeat: true });
    fireEvent.keyDown(screen.getByRole("tree"), { key: " ", repeat: true });
    expect(onOpenFile).toHaveBeenCalledTimes(1);
  });

  it("keeps late child loading from moving focus back from another control", async () => {
    const user = userEvent.setup();
    let resolve!: (entries: FileEntry[]) => void;
    const pending = new Promise<FileEntry[]>((res) => { resolve = res; });
    listDirectory.mockImplementation((path: string) => path === "/p" ? Promise.resolve(roots) : pending);
    render(<><FileTreePanel workspace={workspace} /><button>Outside</button></>);
    await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    await user.keyboard("{ArrowRight}");
    await user.click(screen.getByRole("button", { name: "Outside" }));
    await act(async () => resolve(listings["/p/src"]));
    expect(screen.getByRole("button", { name: "Outside" })).toHaveFocus();
    expect(activeItem()).toHaveAccessibleName("src");
  });

  it("does not scroll the previous active row during pointer focus before a click completes", async () => {
    const user = userEvent.setup();
    render(<><FileTreePanel workspace={workspace} /><button>Outside</button></>);
    const src = await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    await user.keyboard("{End}");
    await user.click(screen.getByRole("button", { name: "Outside" }));
    // The previous active item may now be outside the scrolled viewport.
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    try {
      await user.pointer({ target: within(src).getByText("src"), keys: "[MouseLeft>]" });
      expect(screen.getByRole("tree")).toHaveFocus();
      expect(scroll).not.toHaveBeenCalled();
      await user.pointer({ keys: "[/MouseLeft]" });
      expect(activeItem()).toBe(src);
      expect(src).toHaveAttribute("aria-expanded", "true");
      expect(scroll).toHaveBeenCalledWith({ block: "nearest" });
      expect(scroll.mock.contexts.every((node) => node === src.firstElementChild)).toBe(true);
    } finally {
      scroll.mockRestore();
    }
  });

  it("keeps focus on refresh, falls back to a surviving ancestor, then handles removal and empty results", async () => {
    const user = userEvent.setup();
    const view = render(<FileTreePanel workspace={workspace} />);
    await screen.findByRole("treeitem", { name: "src" });
    await user.tab();
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowDown}");
    expect(activeItem()).toHaveAccessibleName("main.ts");
    const tree = screen.getByRole("tree");
    view.rerender(<FileTreePanel workspace={workspace} refreshKey={1} />);
    expect(tree).toHaveFocus();
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{ArrowRight}");
    expect(activeItem()).toHaveAccessibleName("src");
    listDirectory.mockResolvedValue([entry("/p/replacement.ts")]);
    view.rerender(<FileTreePanel workspace={workspace} refreshKey={2} />);
    await screen.findByRole("treeitem", { name: "replacement.ts" });
    expect(activeItem()).toHaveAccessibleName("replacement.ts");
    expect(tree).toHaveFocus();
    listDirectory.mockResolvedValue([]);
    view.rerender(<FileTreePanel workspace={workspace} refreshKey={3} />);
    await screen.findByText("No files");
    expect(tree).toHaveFocus();
    expect(tree).not.toHaveAttribute("aria-activedescendant");
    await user.keyboard("{Home}{End}{ArrowDown}{Enter} ");
    expect(openEditorTab).not.toHaveBeenCalled();
  });

  it("preserves visible focus through hidden-file reloads and never steals focus from controls", async () => {
    const user = userEvent.setup();
    setShowHidden(true);
    listDirectory.mockImplementation((_path: string, hidden: boolean) =>
      Promise.resolve(hidden ? [entry("/p/.env"), ...roots] : roots));
    const view = render(<><button>Outside</button><FileTreePanel workspace={workspace} /></>);
    const hidden = await screen.findByRole("treeitem", { name: ".env" });
    await user.click(within(hidden).getByText(".env"));
    act(() => setShowHidden(false));
    await screen.findByRole("treeitem", { name: "src" });
    expect(screen.queryByRole("treeitem", { name: ".env" })).toBeNull();
    expect(screen.getByRole("tree")).toHaveFocus();
    expect(activeItem()).toHaveAccessibleName("src");
    await user.keyboard("{End}");
    act(() => setShowHidden(true));
    await screen.findByRole("treeitem", { name: ".env" });
    expect(activeItem()).toHaveAccessibleName("README.md");
    await user.click(screen.getByRole("button", { name: "Outside" }));
    listDirectory.mockResolvedValue([entry("/p/new.ts")]);
    view.rerender(<><button>Outside</button><FileTreePanel workspace={workspace} refreshKey={1} /></>);
    await screen.findByRole("treeitem", { name: "new.ts" });
    expect(screen.getByRole("button", { name: "Outside" })).toHaveFocus();
  });
});
