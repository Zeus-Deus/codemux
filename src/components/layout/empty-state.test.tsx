/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";

const { openProjectMock, openCloneDialogMock, openProjectAtPathMock, recentProjectsMock } = vi.hoisted(() => ({
  openProjectMock: vi.fn(),
  openCloneDialogMock: vi.fn(),
  openProjectAtPathMock: vi.fn(),
  recentProjectsMock: vi.fn(),
}));
vi.mock("./window-chrome", () => ({ WindowChrome: () => null }));
vi.mock("@/components/overlays/clone-dialog", () => ({ CloneDialog: () => null }));
vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: openProjectMock, openCloneDialog: openCloneDialogMock }),
  openProjectAtPath: openProjectAtPathMock,
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dbGetRecentProjects: recentProjectsMock,
  dbGetUiState: vi.fn().mockResolvedValue(null),
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
  openCloneDialogMock.mockReset();
  openProjectAtPathMock.mockReset().mockResolvedValue({ success: true });
  recentProjectsMock.mockReset().mockResolvedValue([]);
});

it("offers explicit import in the legacy zero-workspace landing", () => {
  render(<EmptyState />);
  fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
});

it("opens the clone dialog directly instead of behind New Project", () => {
  render(<EmptyState />);
  fireEvent.click(screen.getByRole("button", { name: "Clone repository" }));
  expect(openCloneDialogMock).toHaveBeenCalledOnce();
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
