/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("./WorkspaceStatusCluster", () => ({
  WorkspaceStatusCluster: () => <span data-testid="status-cluster-stub" />,
}));

import {
  ComposerContextRow,
  CONTEXT_ROW_STEPS,
  contextRowLadder,
} from "./ComposerContextRow";

/** jsdom has no layout: give the row and its gap fixed widths. */
function mockWidths(row: number, gap: number) {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function (this: HTMLElement) {
      const width = this.dataset.testid === "composer-context-row"
        ? row
        : this.getAttribute("aria-hidden") === "true" && this.classList.contains("flex-1")
          ? gap
          : 0;
      return { width, height: 0, top: 0, left: 0, right: width, bottom: 0, x: 0, y: 0, toJSON: () => ({}) };
    },
  );
}

function renderRow() {
  return render(
    <ComposerContextRow
      projectRoot="/repo/codemux"
      isHomeWorkspace={false}
      branch="fix-composer"
      settings={<button type="button">Opus 5.5</button>}
    />,
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("contextRowLadder", () => {
  it("gives up branch, then secondary labels, git counts, project, effort", () => {
    expect(contextRowLadder(0)).toEqual({
      hideBranch: false,
      hideSecondary: false,
      hideGitCounts: false,
      hideProject: false,
      hideEffort: false,
    });
    expect(contextRowLadder(1).hideBranch).toBe(true);
    expect(contextRowLadder(1).hideSecondary).toBe(false);
    expect(contextRowLadder(3)).toMatchObject({ hideSecondary: true, hideGitCounts: true, hideProject: false });
    expect(Object.values(contextRowLadder(CONTEXT_ROW_STEPS)).every(Boolean)).toBe(true);
  });
});

describe("ComposerContextRow", () => {
  it("shows project, branch, the composer's settings and the status cluster", () => {
    renderRow();
    expect(screen.getByText("codemux")).toBeInTheDocument();
    expect(screen.getByText("fix-composer")).toBeInTheDocument();
    expect(screen.getByTestId("context-row-settings")).toContainElement(
      screen.getByRole("button", { name: "Opus 5.5" }),
    );
    expect(screen.getByTestId("status-cluster-stub")).toBeInTheDocument();
  });

  it("keeps everything while there is room", () => {
    mockWidths(700, 120);
    renderRow();
    const row = screen.getByTestId("composer-context-row");
    expect(row).toHaveAttribute("data-fit-level", "0");
    expect(row).not.toHaveAttribute("data-hide-branch");
  });

  it("steps through the ladder while the row is full, never past its last step", () => {
    mockWidths(320, 0);
    renderRow();
    const row = screen.getByTestId("composer-context-row");
    expect(row).toHaveAttribute("data-fit-level", String(CONTEXT_ROW_STEPS));
    for (const attr of [
      "data-hide-branch",
      "data-hide-secondary",
      "data-hide-git-counts",
      "data-hide-project",
      "data-hide-effort",
    ]) {
      expect(row).toHaveAttribute(attr);
    }
  });
});
