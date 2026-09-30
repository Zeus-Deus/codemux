/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { EmptyState } from "./empty-state";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";
vi.mock("./window-chrome", () => ({ WindowChrome: () => null }));
vi.mock("@/hooks/use-project-actions", () => ({ useProjectActions: () => ({ openProject: vi.fn() }) }));
afterEach(cleanup);
it("offers explicit import in the legacy zero-workspace landing", () => {
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ localSessionImportOfferDismissed: false, showLocalSessionImport: false });
  render(<EmptyState />);
  fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
});
