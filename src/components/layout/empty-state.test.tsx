/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, fireEvent } from "@testing-library/react";

const {
  openProjectMock,
  openAddProjectMock,
  openProjectAtPathMock,
  recentProjectsMock,
  listDirectoryMock,
  createEmptyWorkspaceMock,
  toastErrorMock,
  dragDrop,
} = vi.hoisted(() => ({
  openProjectMock: vi.fn(),
  openAddProjectMock: vi.fn(),
  openProjectAtPathMock: vi.fn(),
  recentProjectsMock: vi.fn(),
  listDirectoryMock: vi.fn(),
  createEmptyWorkspaceMock: vi.fn(),
  toastErrorMock: vi.fn(),
  dragDrop: { handler: null as null | ((e: { payload: unknown }) => void) },
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (handler: (e: { payload: unknown }) => void) => {
      dragDrop.handler = handler;
      return Promise.resolve(() => {
        dragDrop.handler = null;
      });
    },
  }),
}));
vi.mock("@/lib/toast", () => ({ toast: { error: toastErrorMock, success: vi.fn() } }));
vi.mock("./window-chrome", () => ({ WindowChrome: () => null }));
vi.mock("@/hooks/use-project-actions", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/use-project-actions")>()),
  useProjectActions: () => ({ openProject: openProjectMock, openAddProject: openAddProjectMock }),
  openProjectAtPath: openProjectAtPathMock,
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dbGetRecentProjects: recentProjectsMock,
  listDirectory: listDirectoryMock,
  dbGetUiState: vi.fn().mockResolvedValue(null),
  // A dropped folder goes through the real open flow down to these.
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: createEmptyWorkspaceMock,
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
}));

import { EmptyState } from "./empty-state";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";

afterEach(cleanup);
beforeEach(() => {
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ localSessionImportOfferDismissed: false, showLocalSessionImport: false });
  openAddProjectMock.mockReset();
  useUIStore.setState({ addProjectSheet: null });
  openProjectAtPathMock.mockReset().mockResolvedValue({ success: true });
  recentProjectsMock.mockReset().mockResolvedValue([]);
  listDirectoryMock.mockReset().mockResolvedValue([]);
  createEmptyWorkspaceMock.mockReset().mockResolvedValue("ws-dropped");
  toastErrorMock.mockReset();
});

function emitDragDrop(payload: unknown) {
  act(() => dragDrop.handler?.({ payload }));
}

it("offers explicit import in the legacy zero-workspace landing", () => {
  render(<EmptyState />);
  fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
});

it("opens the Add project sheet on the Clone and New tabs", () => {
  render(<EmptyState />);
  fireEvent.click(screen.getByRole("button", { name: "Clone repository" }));
  expect(openAddProjectMock).toHaveBeenLastCalledWith("clone");
  fireEvent.click(screen.getByRole("button", { name: "New project" }));
  expect(openAddProjectMock).toHaveBeenLastCalledWith("new");
});

it("lists recent projects and reopens one by path", async () => {
  recentProjectsMock.mockResolvedValue([
    { path: "/work/parser", name: "parser", last_opened_at: "2026-10-01T10:00:00Z" },
    { path: "/work/docs", name: "docs", last_opened_at: "2026-09-30T10:00:00Z" },
  ]);
  render(<EmptyState />);
  const list = await screen.findByRole("region", { name: "Recent projects" });
  expect(list).toHaveTextContent("parser");
  expect(list).toHaveTextContent("docs");
  fireEvent.click(screen.getByRole("button", { name: /docs/ }));
  await vi.waitFor(() => expect(openProjectAtPathMock).toHaveBeenCalledWith("/work/docs"));
});

it("leaves the recent list out when there is nothing to reopen", async () => {
  render(<EmptyState />);
  await vi.waitFor(() => expect(recentProjectsMock).toHaveBeenCalled());
  expect(screen.queryByRole("region", { name: "Recent projects" })).toBeNull();
});

it("opens a folder dropped from the file manager", async () => {
  render(<EmptyState />);
  await vi.waitFor(() => expect(dragDrop.handler).not.toBeNull());

  emitDragDrop({ type: "enter", paths: ["/work/parser"], position: { x: 0, y: 0 } });
  expect(screen.getByText("Drop the folder to open it")).toBeInTheDocument();

  emitDragDrop({ type: "drop", paths: ["/work/parser"], position: { x: 0, y: 0 } });
  expect(screen.queryByText("Drop the folder to open it")).toBeNull();
  await vi.waitFor(() => expect(createEmptyWorkspaceMock).toHaveBeenCalledWith("/work/parser"));
  expect(listDirectoryMock).toHaveBeenCalledWith("/work/parser");
});

it("leaves drops to the Add project sheet while it is open", async () => {
  render(<EmptyState />);
  await vi.waitFor(() => expect(dragDrop.handler).not.toBeNull());
  useUIStore.setState({ addProjectSheet: "open" });
  emitDragDrop({ type: "drop", paths: ["/work/parser"], position: { x: 0, y: 0 } });
  await Promise.resolve();
  expect(listDirectoryMock).not.toHaveBeenCalled();
  expect(createEmptyWorkspaceMock).not.toHaveBeenCalled();
});

it("refuses a dropped file instead of opening it as a project", async () => {
  listDirectoryMock.mockRejectedValue("Not a directory: /work/notes.md");
  render(<EmptyState />);
  await vi.waitFor(() => expect(dragDrop.handler).not.toBeNull());

  emitDragDrop({ type: "drop", paths: ["/work/notes.md"], position: { x: 0, y: 0 } });
  await vi.waitFor(() => expect(toastErrorMock).toHaveBeenCalledWith(
    "notes.md is not a folder. Drop a project folder to open it.",
  ));
  expect(createEmptyWorkspaceMock).not.toHaveBeenCalled();
});

it("stops listening for drops once the page is gone", async () => {
  const view = render(<EmptyState />);
  await vi.waitFor(() => expect(dragDrop.handler).not.toBeNull());
  view.unmount();
  expect(dragDrop.handler).toBeNull();
});
