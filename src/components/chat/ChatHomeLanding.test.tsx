/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const { openProjectMock, openCloneDialogMock, openProjectAtPathMock, recentProjectsMock, listDirectoryMock, toastErrorMock, mockProbe } = vi.hoisted(() => ({
  mockProbe: vi.fn(),
  openProjectMock: vi.fn(),
  listDirectoryMock: vi.fn(),
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
  agentChatProviderHealth: mockProbe,
  dbGetRecentProjects: recentProjectsMock,
  listDirectory: listDirectoryMock,
  dbGetUiState: vi.fn().mockResolvedValue(null),
}));

import { ChatHomeLanding } from "./ChatHomeLanding";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { useProjectPickerHintStore } from "@/stores/project-picker-hint-store";
import { emptyHealthSlot, HEALTH_REPROBE_MS, useProviderHealth } from "@/stores/provider-health-store";
import type { AgentChatProviderKind, ProviderHealthReport, AppStateSnapshot } from "@/tauri/types";
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
beforeEach(() => {
  mockProbe.mockReset();
  useProviderHealth.setState({
    slots: {
      claude: emptyHealthSlot(), codex: emptyHealthSlot(), cursor: emptyHealthSlot(),
      grok: emptyHealthSlot(), hermes: emptyHealthSlot(), acp: emptyHealthSlot(),
      opencode: emptyHealthSlot(),
    },
  });
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot, homeDir: null });
  toastErrorMock.mockReset();
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: false, localSessionImportOfferDismissed: false });
  useProjectPickerHintStore.setState({ seen: false });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  openProjectMock.mockReset().mockResolvedValue({ success: false });
  openCloneDialogMock.mockReset();
  openProjectAtPathMock.mockReset().mockResolvedValue({ success: true, path: "/work/parser", name: "parser" });
  recentProjectsMock.mockReset().mockResolvedValue([]);
  listDirectoryMock.mockReset().mockResolvedValue([]);
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

  it("asks what to work on when the thread has no project", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} projectName={null} />,
    );
    expect(container.querySelector("h1")?.textContent).toBe(
      "What should we work on?",
    );
    expect(
      screen.queryByRole("button", { name: "or start without a project" }),
    ).toBeNull();
  });

  it("offers the project picker as 'No project' under the no-project headline", () => {
    render(
      <ChatHomeLanding
        composer={<div />}
        projectName={null}
        projectPicker={(trigger) => <span data-testid="picker">{trigger}</span>}
      />,
    );
    const trigger = screen.getByRole("button", { name: "No project" });
    expect(screen.getByTestId("picker")).toContainElement(trigger);
    // Under the heading, not inside it.
    expect(trigger.closest("h1")).toBeNull();
  });

  it("reserves the line under a project-scoped headline so it doesn't jump", () => {
    const view = render(
      <ChatHomeLanding
        composer={<div />}
        projectName="codemux"
        projectPicker={(trigger) => trigger}
        onStartWithoutProject={() => {}}
      />,
    );
    const withProject = screen.getByTestId("landing-subline").className;
    view.rerender(
      <ChatHomeLanding
        composer={<div />}
        projectName={null}
        projectPicker={(trigger) => trigger}
        onStartWithoutProject={() => {}}
      />,
    );
    expect(screen.getByTestId("landing-subline").className).toBe(withProject);
    expect(withProject).toContain("h-8");
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByTestId("landing-subline")).toBeNull();
  });

  it("moves focus to the 'No project' picker after starting without a project", () => {
    function Harness() {
      const [projectName, setProjectName] = useState<string | null>("codemux");
      return (
        <ChatHomeLanding
          composer={<div />}
          projectName={projectName}
          projectPicker={(trigger) => trigger}
          onStartWithoutProject={() => setProjectName(null)}
        />
      );
    }
    render(<Harness />);
    fireEvent.click(
      screen.getByRole("button", { name: "or start without a project" }),
    );
    expect(screen.getByRole("button", { name: "No project" })).toHaveFocus();
  });

  it("names the project through the picker and offers to drop it", () => {
    useProjectPickerHintStore.setState({ seen: true });
    const onStartWithoutProject = vi.fn();
    const { container } = render(
      <ChatHomeLanding
        composer={<div />}
        projectName="codemux"
        projectPicker={(trigger) => <span data-testid="picker">{trigger}</span>}
        onStartWithoutProject={onStartWithoutProject}
      />,
    );
    expect(container.querySelector("h1")?.textContent).toBe(
      "What should we build in codemux?",
    );
    const name = screen.getByRole("button", { name: "codemux" });
    expect(screen.getByTestId("picker")).toContainElement(name);
    fireEvent.click(
      screen.getByRole("button", { name: "or start without a project" }),
    );
    expect(onStartWithoutProject).toHaveBeenCalledTimes(1);
  });

  it("points at the picker until it has been opened once", () => {
    const view = render(
      <ChatHomeLanding
        composer={<div />}
        projectName="codemux"
        projectPicker={(trigger) => trigger}
      />,
    );
    expect(screen.getByTestId("project-picker-hint")).toHaveAttribute("aria-hidden");
    fireEvent.click(screen.getByRole("button", { name: "codemux" }));
    expect(screen.queryByTestId("project-picker-hint")).toBeNull();
    expect(useProjectPickerHintStore.getState().seen).toBe(true);
    // Stays gone on the no-project picker too.
    view.rerender(
      <ChatHomeLanding
        composer={<div />}
        projectName={null}
        projectPicker={(trigger) => trigger}
      />,
    );
    expect(screen.queryByTestId("project-picker-hint")).toBeNull();
  });

  it("shows no hint where there is no picker", () => {
    render(<ChatHomeLanding composer={<div />} projectName="codemux" />);
    expect(screen.queryByTestId("project-picker-hint")).toBeNull();
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

  it.each(["codemux", null])(
    "uses only neutral foreground/muted color tokens — no accents (project: %s)",
    (projectName) => {
      const { container } = render(
        <ChatHomeLanding
          composer={<div />}
          projectName={projectName}
          projectPicker={(trigger) => trigger}
          onStartWithoutProject={() => {}}
        />,
      );
      const html = container.innerHTML;
      expect(html).not.toMatch(/\btext-primary\b/);
      expect(html).not.toMatch(/\bbg-primary\b/);
      expect(html).not.toMatch(/\btext-success\b/);
      expect(html).not.toMatch(/\btext-warning\b/);
      expect(html).not.toMatch(/\btext-danger\b/);
    },
  );

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

    it("refuses a recent project whose folder is gone and drops its row", async () => {
      recentProjectsMock.mockResolvedValue([
        { path: "/work/parser", name: "parser", last_opened_at: "2026-10-01T10:00:00Z" },
        { path: "/work/docs", name: "docs", last_opened_at: "2026-09-30T10:00:00Z" },
      ]);
      listDirectoryMock.mockImplementation(async (path: string) => {
        if (path === "/work/parser") throw new Error("No such file or directory");
        return [];
      });
      const onProjectOpened = vi.fn();
      render(<ChatHomeLanding composer={<div />} onProjectOpened={onProjectOpened} />);
      fireEvent.click(await screen.findByRole("button", { name: /parser/ }));
      await vi.waitFor(() =>
        expect(toastErrorMock).toHaveBeenCalledWith(
          "Couldn't open parser at /work/parser: No such file or directory",
        ),
      );
      expect(openProjectAtPathMock).not.toHaveBeenCalled();
      expect(onProjectOpened).not.toHaveBeenCalled();
      const list = screen.getByRole("region", { name: "Recent projects" });
      expect(list).not.toHaveTextContent("parser");
      expect(list).toHaveTextContent("docs");
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

  describe("first-run agent strip", () => {
    const HEALTH: Partial<Record<AgentChatProviderKind, Partial<ProviderHealthReport>>> = {
      claude: { status: "ready", installed: true },
      codex: {
        status: "error",
        installed: true,
        message: "Codex CLI is not authenticated. Run `codex login` and retry.",
      },
      cursor: { status: "error", installed: false, message: "Not on PATH." },
      grok: { status: "error", installed: false, message: "Not on PATH." },
      opencode: { status: "ready", installed: true },
    };

    beforeEach(() => {
      mockProbe.mockImplementation(async (provider: AgentChatProviderKind) => ({
        provider,
        message: null,
        version: null,
        ...HEALTH[provider],
      }));
    });

    async function renderWithAgents(onSelect = vi.fn()) {
      const view = render(
        <ChatHomeLanding
          composer={<div />}
          agents={{ selected: "claude", onSelect }}
        />,
      );
      await act(async () => {
        await new Promise((r) => setTimeout(r, 0));
      });
      return { view, onSelect };
    }

    it("shows each agent's readiness once every probe has answered", async () => {
      const { onSelect } = await renderWithAgents();
      expect(mockProbe).toHaveBeenCalledTimes(5);
      expect(mockProbe).not.toHaveBeenCalledWith("hermes");
      expect(screen.getByTestId("agent-chip-claude")).toHaveAttribute(
        "data-readiness",
        "ready",
      );
      expect(screen.getByTestId("agent-chip-claude")).toHaveAttribute(
        "aria-pressed",
        "true",
      );
      const codex = screen.getByTestId("agent-chip-codex");
      expect(codex).toHaveAttribute("data-readiness", "not_ready");
      expect(codex).toHaveAttribute("title", expect.stringContaining("codex login"));
      expect(screen.getByTestId("agent-chip-cursor")).toBeDisabled();

      fireEvent.click(screen.getByTestId("agent-chip-opencode"));
      expect(onSelect).toHaveBeenCalledWith("opencode");
    });

    it("reserves its row but stays empty while a probe is still running", async () => {
      mockProbe.mockImplementation(() => new Promise(() => {}));
      await renderWithAgents();
      expect(screen.getByTestId("agent-availability-strip")).toBeEmptyDOMElement();
    });

    it("shows its chips after a cache expires while another opening probe is outstanding", async () => {
      vi.useFakeTimers({ toFake: ["Date"] });
      const openedAt = Date.now();
      const missingClaude: ProviderHealthReport = {
        provider: "claude", status: "error", installed: false,
        message: "Not on PATH.", version: null,
      };
      useProviderHealth.setState((state) => ({
        slots: {
          ...state.slots,
          claude: {
            ...emptyHealthSlot(),
            report: missingClaude,
            fetchedAt: openedAt - HEALTH_REPROBE_MS + 1,
          },
        },
      }));
      let resolveClaude!: (report: ProviderHealthReport) => void;
      let resolveCodex!: (report: ProviderHealthReport) => void;
      const claude = new Promise<ProviderHealthReport>((resolve) => { resolveClaude = resolve; });
      const codex = new Promise<ProviderHealthReport>((resolve) => { resolveCodex = resolve; });
      mockProbe.mockImplementation((provider: AgentChatProviderKind) =>
        provider === "claude" ? claude : provider === "codex" ? codex :
          Promise.resolve({ provider, status: "ready", installed: true, message: null, version: null }),
      );
      await renderWithAgents();
      expect(mockProbe).not.toHaveBeenCalledWith("claude");

      vi.setSystemTime(openedAt + 2);
      await act(async () => {
        resolveCodex({ provider: "codex", status: "ready", installed: true, message: null, version: null });
        await codex;
      });
      expect(mockProbe).toHaveBeenCalledWith("claude");
      expect(screen.getByTestId("agent-availability-strip")).toBeEmptyDOMElement();

      await act(async () => {
        resolveClaude(missingClaude);
        await claude;
      });
      expect(screen.getByTestId("agent-chip-codex")).toHaveAttribute("data-readiness", "ready");
      expect(screen.getByTestId("agent-chip-claude")).toBeDisabled();
    });

    it("only appears on a first run, and only where the composer can switch", async () => {
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      const { view } = await renderWithAgents();
      expect(screen.queryByTestId("agent-availability-strip")).toBeNull();
      expect(mockProbe).not.toHaveBeenCalled();
      view.unmount();

      useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.queryByTestId("agent-availability-strip")).toBeNull();
    });
  });
