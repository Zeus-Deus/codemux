/**
 * The two search overlays' chrome — the part that isn't about searching.
 *
 * Both regressions these lock were reported together: the dialog rendered
 * *blurry*, and its close button sat on the search input's top-right corner.
 * One cause each, both in the shared `DialogContent` defaults rather than in
 * the dialogs themselves, which is exactly why they need pinning here.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { DIALOG_CRISP_POSITION } from "@/components/ui/dialog";
import { useUIStore } from "@/stores/ui-store";

vi.mock("@/tauri/commands", () => ({
  searchFileNames: vi.fn().mockResolvedValue([]),
  searchFileContents: vi.fn().mockResolvedValue([]),
}));
vi.mock("@/stores/app-store", () => ({
  useActiveWorkspaceCwd: () => "/repo",
  useAppStore: Object.assign(
    vi.fn(() => null),
    { getState: () => ({ appState: null }) },
  ),
}));

import { FileSearchDialog } from "./file-search-dialog";
import { ContentSearchDialog } from "./content-search-dialog";

beforeEach(() => {
  useUIStore.setState({ showFileSearch: false, showContentSearch: false });
});
afterEach(cleanup);

function contentEl(): HTMLElement {
  return screen.getByRole("dialog");
}

describe("search dialogs — crisp positioning", () => {
  // A translate puts the dialog on its own composited layer, and `top-1/2` /
  // `left-1/2` / `-translate-*-1/2` are all fractional the moment the window
  // (or the dialog) has an odd dimension. The layer then gets resampled onto
  // a half pixel and every glyph in it softens on WebKitGTK. Position has to
  // come from layout, which paints snapped to the pixel grid.
  it("positions the file search without any transform", () => {
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);
    const el = contentEl();

    for (const cls of DIALOG_CRISP_POSITION.split(" ")) {
      expect(el).toHaveClass(cls);
    }
    // The defaults that would re-introduce the composited half-pixel.
    expect(el).not.toHaveClass("-translate-x-1/2");
    expect(el).not.toHaveClass("-translate-y-1/2");
    expect(el).not.toHaveClass("top-1/2");
  });

  it("positions the content search the same way", () => {
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    const el = contentEl();
    for (const cls of DIALOG_CRISP_POSITION.split(" ")) {
      expect(el).toHaveClass(cls);
    }
  });

  // `DIALOG_CRISP_POSITION` is only correct if every offset in it is a whole
  // number of pixels at any window size — a percentage would put us straight
  // back on a half pixel.
  it("uses no percentage offsets", () => {
    expect(DIALOG_CRISP_POSITION).not.toMatch(/\d\/\d/);
    expect(DIALOG_CRISP_POSITION).not.toMatch(/%/);
  });
});

describe("search dialogs — close affordance", () => {
  // Every other overlay in the app passes `showCloseButton={false}` and
  // closes on Escape or a backdrop click. These two never opted out, so they
  // inherited the shared `absolute top-2 right-2` button — which, over their
  // `p-0` content, landed on the search input's top-right corner instead of
  // the padded header it assumes.
  it("shows no × in the file search", () => {
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);
    expect(screen.queryByRole("button", { name: /close/i })).toBeNull();
  });

  it("shows no × in the content search", () => {
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    expect(screen.queryByRole("button", { name: /close/i })).toBeNull();
  });
});
