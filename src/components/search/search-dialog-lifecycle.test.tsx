/**
 * The search dialogs stay mounted after their first open (app-shell's
 * `useMountedOnceOpen`) so Radix can play the exit animation. That moves
 * their state across opens, which these tests pin:
 *
 * - closing must not clear the results while the exit animation runs, or the
 *   list collapses to "No matching files" mid-fade;
 * - reopening must start from an empty query on the very first render;
 * - a search that resolves after a close must not repopulate the list.
 *
 * jsdom has no CSS animations, so Radix would unmount the content the instant
 * it closes. `getComputedStyle` is patched to report an exit animation on
 * `data-state="closed"`, which makes Presence keep the content mounted until
 * `animationend`, as it does in the app.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import { useUIStore } from "@/stores/ui-store";
import { searchFileNames, searchInFiles } from "@/tauri/commands";
import type { SearchResult } from "@/tauri/types";

vi.mock("@/tauri/commands", () => ({
  searchFileNames: vi.fn(),
  searchInFiles: vi.fn(),
}));
vi.mock("@/stores/app-store", () => ({
  selectActiveWorkspaceId: () => null,
  useActiveWorkspaceCwd: () => "/repo",
  useAppStore: Object.assign(
    vi.fn(() => null),
    { getState: () => ({ appState: null }) },
  ),
}));

import { FileSearchDialog } from "./file-search-dialog";
import { ContentSearchDialog } from "./content-search-dialog";

const realGetComputedStyle = window.getComputedStyle.bind(window);

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(searchFileNames).mockReset();
  vi.mocked(searchInFiles).mockReset();
  useUIStore.setState({ showFileSearch: false, showContentSearch: false });
  vi.spyOn(window, "getComputedStyle").mockImplementation((el, pseudo) => {
    const styles = realGetComputedStyle(el, pseudo);
    return new Proxy(styles, {
      get(target, prop) {
        if (prop === "animationName") {
          const state = (el as Element).getAttribute("data-state");
          return state === "closed" ? "exit" : state === "open" ? "enter" : "none";
        }
        const value = Reflect.get(target, prop, target);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
  });
  if (!globalThis.CSS?.escape) {
    vi.stubGlobal("CSS", { ...globalThis.CSS, escape: (s: string) => s });
  }
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function flushSearch(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

function searchInput(): HTMLInputElement {
  return screen.getByRole("textbox") as HTMLInputElement;
}

describe("file search across opens", () => {
  it("keeps results through the exit and reopens empty", async () => {
    vi.mocked(searchFileNames).mockResolvedValue(["src/app.tsx", "src/main.tsx"]);
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);

    fireEvent.change(searchInput(), { target: { value: "app" } });
    await flushSearch(250);
    expect(screen.getByText("app.tsx")).toBeInTheDocument();

    act(() => useUIStore.setState({ showFileSearch: false }));
    // Still mounted for the exit animation, with the list intact.
    expect(screen.getByRole("dialog", { hidden: true })).toHaveAttribute("data-state", "closed");
    expect(screen.getByText("app.tsx")).toBeInTheDocument();
    expect(screen.queryByText("No matching files")).toBeNull();

    act(() => useUIStore.setState({ showFileSearch: true }));
    expect(searchInput().value).toBe("");
    expect(screen.queryByText("app.tsx")).toBeNull();
    expect(screen.getByText("Type a file name to search")).toBeInTheDocument();
  });

  it("ignores a search that resolves after the dialog closed", async () => {
    let resolve: (files: string[]) => void = () => {};
    vi.mocked(searchFileNames).mockImplementation(
      () => new Promise<string[]>((r) => (resolve = r)),
    );
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);

    fireEvent.change(searchInput(), { target: { value: "late" } });
    await flushSearch(250);
    expect(searchFileNames).toHaveBeenCalledTimes(1);

    act(() => useUIStore.setState({ showFileSearch: false }));
    act(() => useUIStore.setState({ showFileSearch: true }));
    await act(async () => resolve(["src/late.tsx"]));

    expect(screen.queryByText("late.tsx")).toBeNull();
    expect(searchInput().value).toBe("");
  });
});

describe("content search across opens", () => {
  const match: SearchResult = {
    file_path: "src/app.tsx",
    line_number: 3,
    line_content: "const needle = 1;",
    match_start: 6,
    match_end: 12,
  };

  it("keeps results through the exit and reopens with fresh options", async () => {
    vi.mocked(searchInFiles).mockResolvedValue([match]);
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);

    fireEvent.change(searchInput(), { target: { value: "needle" } });
    await flushSearch(350);
    expect(screen.getAllByText(/app\.tsx/).length).toBeGreaterThan(0);
    const before = screen.getByRole("dialog").textContent;

    act(() => useUIStore.setState({ showContentSearch: false }));
    expect(screen.getByRole("dialog", { hidden: true })).toHaveAttribute("data-state", "closed");
    expect(screen.getByRole("dialog", { hidden: true }).textContent).toBe(before);

    act(() => useUIStore.setState({ showContentSearch: true }));
    expect(searchInput().value).toBe("");
    expect(screen.queryByText(/app\.tsx/)).toBeNull();
  });
});
