/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { MobileComposerChips } from "./mobile-composer-chips";

function seed(patch: Partial<WorkspaceSnapshot>) {
  const workspace = {
    workspace_id: "ws-1",
    git_changed_files: 0,
    git_additions: 0,
    git_deletions: 0,
    pr_number: null,
    ...patch,
  } as WorkspaceSnapshot;
  useAppStore.setState({
    appState: { workspaces: [workspace] } as unknown as AppStateSnapshot,
  });
}

beforeEach(() => useUIStore.setState({ rightPanelTabs: {} }));
afterEach(cleanup);

describe("MobileComposerChips", () => {
  it("renders nothing for a clean workspace without a PR", () => {
    seed({});
    const { container } = render(<MobileComposerChips workspaceId="ws-1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it("opens Changes from the file chip and Review from the PR chip", () => {
    seed({ git_changed_files: 1, git_additions: 9, git_deletions: 1, pr_number: 172 });
    render(<MobileComposerChips workspaceId="ws-1" />);
    const files = screen.getByRole("button", { name: "Review 1 changed file" });
    expect(files).toHaveTextContent("1 file+9−1");
    fireEvent.click(files);
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
    fireEvent.click(screen.getByRole("button", { name: "Open pull request #172" }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("review");
  });
});
