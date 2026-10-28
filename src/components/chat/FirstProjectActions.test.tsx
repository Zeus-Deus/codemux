/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const { openProjectMock, openAddProjectMock } = vi.hoisted(() => ({
  openProjectMock: vi.fn(),
  openAddProjectMock: vi.fn(),
}));

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: openProjectMock, openAddProject: openAddProjectMock }),
  openDroppedFolder: vi.fn(),
  openProjectAtPath: vi.fn(),
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dbGetRecentProjects: vi.fn().mockResolvedValue([]),
}));

import { FirstProjectActions } from "./FirstProjectActions";

afterEach(cleanup);
beforeEach(() => {
  openProjectMock.mockReset();
  openAddProjectMock.mockReset();
});

it("shows three tiles with their hints", () => {
  render(<FirstProjectActions />);
  expect(screen.getByRole("button", { name: /Open folder/ })).toHaveTextContent(
    "or drop one anywhere",
  );
  expect(screen.getByRole("button", { name: /Clone repository/ })).toHaveTextContent(
    "GitHub or any git URL",
  );
  expect(screen.getByRole("button", { name: /New project/ })).toHaveTextContent(
    "Empty, or scaffolded by an agent",
  );
});

it("opens a folder straight from the picker and reports it", async () => {
  openProjectMock.mockResolvedValue({ success: true, path: "/work/api", name: "api" });
  const onProjectOpened = vi.fn();
  render(<FirstProjectActions onProjectOpened={onProjectOpened} />);
  fireEvent.click(screen.getByRole("button", { name: /Open folder/ }));
  await vi.waitFor(() => expect(onProjectOpened).toHaveBeenCalledWith("/work/api"));
});

it("opens the Add project sheet on the Clone and New tabs", () => {
  render(<FirstProjectActions />);
  fireEvent.click(screen.getByRole("button", { name: /Clone repository/ }));
  expect(openAddProjectMock).toHaveBeenLastCalledWith("clone");
  fireEvent.click(screen.getByRole("button", { name: /New project/ }));
  expect(openAddProjectMock).toHaveBeenLastCalledWith("new");
});
