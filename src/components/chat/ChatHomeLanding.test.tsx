/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const { openProjectMock, openCloneDialogMock, openProjectAtPathMock, recentProjectsMock, toastErrorMock } = vi.hoisted(() => ({
  openProjectMock: vi.fn(),
  openCloneDialogMock: vi.fn(),
  openProjectAtPathMock: vi.fn(),
  recentProjectsMock: vi.fn(),
  toastErrorMock: vi.fn(),
}));
vi.mock("@/lib/toast", () => ({ toast: { error: toastErrorMock, success: vi.fn() } }));
vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: openProjectMock, openCloneDialog: openCloneDialogMock }),
  openProjectAtPath: openProjectAtPathMock,
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dbGetRecentProjects: recentProjectsMock,
  dbGetUiState: vi.fn().mockResolvedValue(null),
}));

import { ChatHomeLanding } from "./ChatHomeLanding";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";
afterEach(cleanup);
beforeEach(() => {
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot, homeDir: null });
  toastErrorMock.mockReset();
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: false, localSessionImportOfferDismissed: false });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  openProjectMock.mockReset().mockResolvedValue({ success: false });
  openCloneDialogMock.mockReset();
  openProjectAtPathMock.mockReset().mockResolvedValue({ success: true, path: "/work/parser", name: "parser" });
  recentProjectsMock.mockReset().mockResolvedValue([]);
});
describe("ChatHomeLanding", () => {
  it("offers opt-in import only for first-run local enabled profiles", () => {
    const view = render(<ChatHomeLanding composer={<div />} />);
    fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
    expect(useUIStore.getState().showLocalSessionImport).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    useUIStore.setState({ localSessionImportOfferDismissed: false });
    useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("hides import for remote clients and disabled chat", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    const view = render(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    useFeatureFlags.setState({ enableAgentChat: false });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("renders the marquee headline", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div data-testid="composer-slot" />} />,
    );
    const heading = container.querySelector("h1");
    expect(heading?.textContent).toBe("What should we do today?");
  });

  it("renders the composer passed via props", () => {
    const { container } = render(
      <ChatHomeLanding
        composer={<div data-testid="composer-slot-a">slotA</div>}
      />,
    );
    expect(
      container.querySelector('[data-testid="composer-slot-a"]'),
    ).not.toBeNull();
  });

  it("uses only neutral foreground/muted color tokens — no accents", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const html = container.innerHTML;
    expect(html).not.toMatch(/\btext-primary\b/);
    expect(html).not.toMatch(/\bbg-primary\b/);
    expect(html).not.toMatch(/\btext-success\b/);
    expect(html).not.toMatch(/\btext-warning\b/);
    expect(html).not.toMatch(/\btext-danger\b/);
  });

  it("centers content vertically and horizontally", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("items-center");
    // Vertical centring via auto margins, so a short pane scrolls instead
    // of clipping the headline.
    expect(root.className).toContain("overflow-y-auto");
    expect((root.firstElementChild as HTMLElement).className).toContain("my-auto");
  });

  describe("with no project open", () => {
    it("offers Open project and Clone repository under the composer", async () => {
      openProjectMock.mockResolvedValue({ success: true, path: "/work/api", name: "api" });
      const onProjectOpened = vi.fn();
      render(<ChatHomeLanding composer={<div />} onProjectOpened={onProjectOpened} />);
      fireEvent.click(screen.getByRole("button", { name: /Open project/ }));
      await vi.waitFor(() => expect(onProjectOpened).toHaveBeenCalledWith("/work/api"));
      fireEvent.click(screen.getByRole("button", { name: "Clone repository" }));
      expect(openCloneDialogMock).toHaveBeenCalledOnce();
    });

    it("reopens a recent project without the folder picker", async () => {
      recentProjectsMock.mockResolvedValue([
        { path: "/work/parser", name: "parser", last_opened_at: "2026-10-01T10:00:00Z" },
      ]);
      const onProjectOpened = vi.fn();
      render(<ChatHomeLanding composer={<div />} onProjectOpened={onProjectOpened} />);
      const row = await screen.findByRole("button", { name: /parser/ });
      fireEvent.click(row);
      await vi.waitFor(() => expect(onProjectOpened).toHaveBeenCalledWith("/work/parser"));
      expect(openProjectAtPathMock).toHaveBeenCalledWith("/work/parser");
      expect(openProjectMock).not.toHaveBeenCalled();
    });

    it("says so when opening the picked folder fails", async () => {
      openProjectMock.mockRejectedValue(new Error("workspace limit reached"));
      const onProjectOpened = vi.fn();
      render(<ChatHomeLanding composer={<div />} onProjectOpened={onProjectOpened} />);
      fireEvent.click(screen.getByRole("button", { name: /Open project/ }));
      await vi.waitFor(() =>
        expect(toastErrorMock).toHaveBeenCalledWith(
          "Couldn't open the project: workspace limit reached",
        ),
      );
      expect(onProjectOpened).not.toHaveBeenCalled();
    });

    it("still lists five recent projects when Home is among them", async () => {
      useAppStore.setState({ homeDir: "/home/me" });
      recentProjectsMock.mockResolvedValue(
        ["/home/me", "/w/a", "/w/b", "/w/c", "/w/d", "/w/e"].map((path) => ({
          path,
          name: path.split("/").pop(),
          last_opened_at: "2026-10-01T10:00:00Z",
        })),
      );
      render(<ChatHomeLanding composer={<div />} />);
      const list = await screen.findByRole("region", { name: "Recent projects" });
      expect(recentProjectsMock).toHaveBeenCalledWith(6);
      expect(list.querySelectorAll("li")).toHaveLength(5);
      expect(list).not.toHaveTextContent("/home/me");
    });

    it("hides the project actions once a workspace exists", () => {
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.queryByRole("button", { name: /Open project/ })).toBeNull();
      expect(screen.queryByRole("button", { name: "Clone repository" })).toBeNull();
      expect(recentProjectsMock).not.toHaveBeenCalled();
    });
  });

  it("renders the notice slot above the composer", () => {
    const { container } = render(
      <ChatHomeLanding
        composer={<div data-testid="composer-slot" />}
        notice={<div data-testid="notice-slot" />}
      />,
    );
    const notice = container.querySelector('[data-testid="notice-slot"]')!;
    const composer = container.querySelector('[data-testid="composer-slot"]')!;
    expect(notice.compareDocumentPosition(composer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
