/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SidebarProjectFilter } from "./sidebar-project-filter";

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: vi.fn() }),
}));

vi.mock("./use-project-appearance", () => ({
  useProjectAppearance: () => ({}),
}));

afterEach(cleanup);

describe("SidebarProjectFilter", () => {
  it("marks only the current filter, retaining a check slot on every filter row", async () => {
    const setFilter = vi.fn();
    const props = {
      projectGroups: [{ projectName: "Alpha", projectPath: "/alpha", workspaces: [] }],
      counts: { map: new Map([["/alpha", 2]]), total: 2 },
      filter: null,
      filterName: null,
      setFilter,
    };
    const user = userEvent.setup();
    const { rerender } = render(<SidebarProjectFilter {...props} />);
    await user.click(screen.getByRole("button", { name: "Filter by project" }));
    const all = screen.getByRole("menuitemradio", { name: "All projects" });
    const alpha = screen.getByRole("menuitemradio", { name: "Alpha" });
    expect(all).toHaveAttribute("aria-checked", "true");
    expect(all).toHaveClass("font-medium");
    expect(all).not.toHaveClass("bg-surface-3");
    expect(alpha).toHaveAttribute("aria-checked", "false");
    for (const row of [all, alpha]) {
      expect(row.lastElementChild).toHaveClass("size-4", "shrink-0");
    }
    expect(all.lastElementChild?.querySelector("svg")).not.toBeNull();
    expect(alpha.lastElementChild).toBeEmptyDOMElement();

    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(setFilter).toHaveBeenCalledWith("/alpha");
    rerender(<SidebarProjectFilter {...props} filter="/alpha" filterName="Alpha" />);
    await user.click(screen.getByRole("button", { name: "Filter by project" }));
    expect(screen.getByRole("menuitemradio", { name: "Alpha" }))
      .toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "All projects" }))
      .toHaveAttribute("aria-checked", "false");
  });

  it("prints a project's count only when it has workspaces", async () => {
    const user = userEvent.setup();
    render(
      <SidebarProjectFilter
        projectGroups={[
          { projectName: "Alpha", projectPath: "/alpha", workspaces: [] },
          { projectName: "Beta", projectPath: "/beta", workspaces: [] },
        ]}
        counts={{ map: new Map([["/alpha", 3], ["/beta", 0]]), total: 3 }}
        filter={null}
        filterName={null}
        setFilter={vi.fn()}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Filter by project" }));
    expect(screen.getByRole("menuitemradio", { name: "Alpha" })).toHaveTextContent(/^AAlpha3$/);
    expect(screen.getByRole("menuitemradio", { name: "Beta" })).toHaveTextContent(/^BBeta$/);
    expect(screen.getByRole("menuitemradio", { name: "All projects" })).toHaveTextContent(/^All projects3$/);
  });
});
