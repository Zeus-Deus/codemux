/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";

const { openProjectMock, openAddProjectMock, openProjectAtPathMock, recentProjectsMock, listDirectoryMock, toastErrorMock, mockProbe, signInMock, installMock, invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  mockProbe: vi.fn(),
  signInMock: vi.fn(),
  installMock: vi.fn(),
  openProjectMock: vi.fn(),
  listDirectoryMock: vi.fn(),
  openAddProjectMock: vi.fn(),
  openProjectAtPathMock: vi.fn(),
  recentProjectsMock: vi.fn(),
  toastErrorMock: vi.fn(),
}));
// Hermes profiles and custom agents are listed straight through `invoke`.
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  invoke: invokeMock,
}));
vi.mock("@/lib/toast", () => ({ toast: { error: toastErrorMock, success: vi.fn() } }));
vi.mock("@/lib/agent-chat/provider-sign-in", () => ({
  signInToProvider: signInMock,
  installProvider: installMock,
}));
vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: openProjectMock, openAddProject: openAddProjectMock }),
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
import { PAST_PROJECTS_RETRY_MS, useFirstRunSession } from "./first-run-agents";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useHermes, type HermesProfile } from "@/stores/hermes-store";
import { useProjectPickerHintStore } from "@/stores/project-picker-hint-store";
import { emptyHealthSlot, HEALTH_REPROBE_MS, useProviderHealth } from "@/stores/provider-health-store";
import type { AgentChatProviderKind, ProviderHealthReport, AppStateSnapshot } from "@/tauri/types";
/** What `hermes_profiles` rejects with when Hermes isn't installed. */
const HERMES_NOT_INSTALLED = "setup_required: install official Hermes with ACP dependencies";
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
  // A fresh window session on a device that has never had a project open.
  useFirstRunSession.setState({ pastProjectsChecked: true, listFailed: {}, connectDismissed: false });
  useUIStore.setState({ firstRunDone: false });
  // A fresh machine: Hermes is enabled but not installed, so listing its
  // profiles fails the way the backend does, and no custom agent is set up.
  invokeMock.mockReset().mockImplementation(async (cmd: string) => {
    if (cmd === "hermes_profiles") throw HERMES_NOT_INSTALLED;
    if (cmd === "acp_agents") return [];
    throw new Error(`unmocked command ${cmd}`);
  });
  useHermes.setState({ profiles: null });
  useCustomAcp.setState({ agents: null });
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot, homeDir: null });
  toastErrorMock.mockReset();
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: false, localSessionImportOfferDismissed: false });
  useProjectPickerHintStore.setState({ seen: false });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  openProjectMock.mockReset().mockResolvedValue({ success: false });
  openAddProjectMock.mockReset();
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
    it("offers Open folder, Clone repository and New project under the composer", async () => {
      openProjectMock.mockResolvedValue({ success: true, path: "/work/api", name: "api" });
      const onProjectOpened = vi.fn();
      render(<ChatHomeLanding composer={<div />} onProjectOpened={onProjectOpened} />);
      fireEvent.click(screen.getByRole("button", { name: /Open folder/ }));
      await vi.waitFor(() => expect(onProjectOpened).toHaveBeenCalledWith("/work/api"));
      fireEvent.click(screen.getByRole("button", { name: /Clone repository/ }));
      expect(openAddProjectMock).toHaveBeenCalledWith("clone");
      fireEvent.click(screen.getByRole("button", { name: /New project/ }));
      expect(openAddProjectMock).toHaveBeenCalledWith("new");
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
      fireEvent.click(screen.getByRole("button", { name: /Open folder/ }));
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

    it("offers the project actions with nothing open or only Home open", () => {
      const view = render(<ChatHomeLanding composer={<div />} />);
      expect(screen.getByRole("button", { name: /Open folder/ })).toBeInTheDocument();
      view.unmount();

      // A returning user who closed every project, leaving Home.
      useUIStore.setState({ firstRunDone: true });
      useAppStore.setState({
        appState: { workspaces: [{ workspace_type: "home" }] } as unknown as AppStateSnapshot,
      });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.getByRole("button", { name: /Open folder/ })).toBeInTheDocument();
    });

    it("gives a returning user with every workspace closed the tiles, not onboarding", async () => {
      // Nothing could run here, which would ask a first run to connect one.
      mockProbe.mockImplementation(async (provider: AgentChatProviderKind) => ({
        provider, status: "error", installed: false, message: "Not on PATH.", version: null,
      }));
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
      expect(useUIStore.getState().firstRunDone).toBe(true);
      render(<ChatHomeLanding composer={<div data-testid="composer" />} agents={{ selected: "claude" }} />);
      await act(async () => {
        await new Promise((r) => setTimeout(r, 0));
      });
      expect(screen.getByRole("button", { name: /Open folder/ })).toBeInTheDocument();
      expect(screen.getByTestId("composer")).toBeInTheDocument();
      expect(screen.queryByTestId("connect-agent")).toBeNull();
      expect(screen.queryByTestId("agent-status-line")).toBeNull();
      expect(mockProbe).not.toHaveBeenCalled();
      expect(invokeMock).not.toHaveBeenCalledWith("hermes_profiles");
      expect(invokeMock).not.toHaveBeenCalledWith("acp_agents");
    });

    describe("on an app start before the first-run flag is known", () => {
      async function start(workspaces: unknown[]) {
        useFirstRunSession.setState({ pastProjectsChecked: false });
        useAppStore.setState({ appState: null });
        useAppStore.setState({ appState: { workspaces } as unknown as AppStateSnapshot });
        render(<ChatHomeLanding composer={<div />} agents={{ selected: "claude" }} />);
        await act(async () => {
          await new Promise((r) => setTimeout(r, 0));
        });
      }
      beforeEach(() => {
        mockProbe.mockImplementation(async (provider: AgentChatProviderKind) => ({
          provider, status: "ready", installed: true, message: null, version: null,
        }));
      });
      /** The first-run check's lookups; the landing lists its own six. */
      const pastProjectsLookups = () =>
        recentProjectsMock.mock.calls.filter(([limit]) => limit === 10).length;

      it("keeps onboarding after a restart mid-setup left only Home open", async () => {
        await start([{ workspace_type: "home", cwd: "/home/me" }]);
        expect(useUIStore.getState().firstRunDone).toBe(false);
        expect(screen.getByTestId("agent-status-line")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /Open folder/ })).toBeInTheDocument();
      });

      it("ignores Home in recent projects", async () => {
        useAppStore.setState({ homeDir: "/home/me" });
        recentProjectsMock.mockResolvedValue([
          { path: "/home/me", name: "me", last_opened_at: "2026-10-01T10:00:00Z" },
        ]);
        await start([{ workspace_type: "home", cwd: "/home/me" }]);
        expect(useUIStore.getState().firstRunDone).toBe(false);
        expect(screen.getByTestId("agent-status-line")).toBeInTheDocument();
      });

      it("treats someone with recent projects as returning", async () => {
        recentProjectsMock.mockResolvedValue([
          { path: "/w/parser", name: "parser", last_opened_at: "2026-10-01T10:00:00Z" },
        ]);
        await start([]);
        expect(useUIStore.getState().firstRunDone).toBe(true);
        expect(screen.queryByTestId("agent-status-line")).toBeNull();
        expect(screen.getByRole("button", { name: /Open folder/ })).toBeInTheDocument();
      });

      it("retries a failed lookup before onboarding and finds a returning user", async () => {
        vi.useFakeTimers();
        let lookups = 0;
        recentProjectsMock.mockImplementation(async (limit: number) => {
          if (limit === 10 && lookups++ === 0) throw new Error("ipc unavailable");
          return [{ path: "/w/parser", name: "parser", last_opened_at: "2026-10-01T10:00:00Z" }];
        });
        useFirstRunSession.setState({ pastProjectsChecked: false });
        useAppStore.setState({ appState: null });
        useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
        await vi.advanceTimersByTimeAsync(0);
        // The failure is not taken as "no projects": onboarding waits.
        expect(useFirstRunSession.getState().pastProjectsChecked).toBe(false);
        render(<ChatHomeLanding composer={<div />} agents={{ selected: "claude" }} />);
        expect(screen.queryByTestId("agent-status-line")).toBeNull();

        await vi.advanceTimersByTimeAsync(PAST_PROJECTS_RETRY_MS[0]);
        expect(pastProjectsLookups()).toBe(2);
        expect(useUIStore.getState().firstRunDone).toBe(true);
        expect(useFirstRunSession.getState().pastProjectsChecked).toBe(true);
        expect(screen.queryByTestId("agent-status-line")).toBeNull();
      });

      it("offers onboarding once every retry of the lookup has failed", async () => {
        vi.useFakeTimers();
        recentProjectsMock.mockRejectedValue(new Error("ipc unavailable"));
        useFirstRunSession.setState({ pastProjectsChecked: false });
        useAppStore.setState({ appState: null });
        useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
        for (const delay of PAST_PROJECTS_RETRY_MS) {
          await vi.advanceTimersByTimeAsync(0);
          expect(useFirstRunSession.getState().pastProjectsChecked).toBe(false);
          await vi.advanceTimersByTimeAsync(delay);
        }
        expect(pastProjectsLookups()).toBe(PAST_PROJECTS_RETRY_MS.length + 1);
        expect(useFirstRunSession.getState().pastProjectsChecked).toBe(true);
        expect(useUIStore.getState().firstRunDone).toBe(false);

        vi.useRealTimers();
        render(<ChatHomeLanding composer={<div />} agents={{ selected: "claude" }} />);
        await act(async () => {
          await new Promise((r) => setTimeout(r, 0));
        });
        expect(screen.getByTestId("agent-status-line")).toBeInTheDocument();
      });

      it("shows no onboarding until recent projects answer", async () => {
        recentProjectsMock.mockReturnValue(new Promise(() => {}));
        await start([]);
        expect(screen.queryByTestId("agent-status-line")).toBeNull();
        expect(mockProbe).not.toHaveBeenCalled();
      });
    });

    it("ends the first run for good once a project opens", () => {
      useAppStore.setState({
        appState: { workspaces: [{ workspace_type: "home" }] } as unknown as AppStateSnapshot,
      });
      expect(useUIStore.getState().firstRunDone).toBe(false);
      useAppStore.setState({
        appState: {
          workspaces: [{ workspace_type: "home" }, { workspace_type: "project" }],
        } as unknown as AppStateSnapshot,
      });
      expect(useUIStore.getState().firstRunDone).toBe(true);
    });

    it("hides the project actions once a workspace exists", () => {
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.queryByRole("button", { name: /Open folder/ })).toBeNull();
      expect(screen.queryByRole("button", { name: /Clone repository/ })).toBeNull();
      expect(recentProjectsMock).not.toHaveBeenCalled();
    });
  });
});

  describe("first-run agents", () => {
    type Answer = Partial<ProviderHealthReport>;
    const READY: Answer = { status: "ready", installed: true };
    const SIGNED_OUT = (provider: string): Answer => ({
      status: "error",
      installed: true,
      message: `${provider} is not signed in.`,
      login_command: `${provider} login`,
    });
    const MISSING: Answer = { status: "error", installed: false, message: "Not on PATH." };
    // Installed, but the probe couldn't check the sign-in.
    const UNVERIFIED: Answer = {
      status: "warning",
      installed: true,
      message: "Couldn't verify sign-in.",
    };
    let health: Partial<Record<AgentChatProviderKind, Answer>>;

    beforeEach(() => {
      health = {
        claude: READY,
        codex: SIGNED_OUT("codex"),
        cursor: MISSING,
        grok: MISSING,
        opencode: READY,
      };
      mockProbe.mockImplementation(async (provider: AgentChatProviderKind) => ({
        provider,
        message: null,
        version: null,
        ...health[provider],
      }));
      signInMock.mockReset().mockResolvedValue(undefined);
      installMock.mockReset().mockResolvedValue(undefined);
      useUIStore.setState({ showSettings: false, settingsSection: null });
    });

    async function flush() {
      await act(async () => {
        await new Promise((r) => setTimeout(r, 0));
      });
    }

    async function renderWithAgents(selected: AgentChatProviderKind = "claude") {
      const view = render(
        <ChatHomeLanding
          composer={<div data-testid="composer" />}
          agents={{ selected }}
        />,
      );
      await flush();
      return view;
    }

    it("sums up what can run in one line once every probe has answered", async () => {
      await renderWithAgents("opencode");
      expect(mockProbe).toHaveBeenCalledTimes(5);
      expect(mockProbe).not.toHaveBeenCalledWith("hermes");
      expect(screen.getByTestId("agent-status-line")).toHaveTextContent(
        "OpenCode and Claude Code are ready·Codex needs sign-in·Set up",
      );
      fireEvent.click(screen.getByRole("button", { name: "Set up" }));
      expect(useUIStore.getState()).toMatchObject({ showSettings: true, settingsSection: "providers" });
    });

    it("counts several agents that need setup", async () => {
      health = { ...health, opencode: MISSING, cursor: SIGNED_OUT("cursor") };
      await renderWithAgents();
      expect(screen.getByTestId("agent-status-line")).toHaveTextContent(
        "Claude Code is ready·2 more need setup·Set up",
      );
    });

    it("reserves its line, and keeps the composer, while a probe is still running", async () => {
      mockProbe.mockImplementation(() => new Promise(() => {}));
      await renderWithAgents();
      expect(screen.getByTestId("agent-status-line")).toBeEmptyDOMElement();
      expect(screen.getByTestId("composer")).toBeInTheDocument();
      expect(screen.queryByTestId("connect-agent")).toBeNull();
    });

    it("waits for a cache that expired while another opening probe is outstanding", async () => {
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
      expect(screen.getByTestId("agent-status-line")).toBeEmptyDOMElement();

      await act(async () => {
        resolveClaude(missingClaude);
        await claude;
      });
      expect(screen.getByTestId("agent-status-line")).toHaveTextContent(
        "Codex and 3 more are ready",
      );
    });

    it("calls an agent it can't verify installed, and keeps the composer", async () => {
      health = { claude: UNVERIFIED, codex: MISSING, cursor: MISSING, grok: MISSING, opencode: MISSING };
      await renderWithAgents();
      expect(screen.queryByTestId("connect-agent")).toBeNull();
      expect(screen.getByTestId("composer")).toBeInTheDocument();
      expect(screen.getByTestId("agent-status-line")).toHaveTextContent("Claude Code is installed");
      expect(screen.getByTestId("agent-status-line")).not.toHaveTextContent("ready");
    });

    it("names only ready agents when some are ready and others unverified", async () => {
      health = { claude: UNVERIFIED, codex: READY, cursor: MISSING, grok: MISSING, opencode: MISSING };
      await renderWithAgents();
      expect(screen.getByTestId("agent-status-line")).toHaveTextContent("Codex is ready·Set up");
    });

    describe("when no agent can run", () => {
      beforeEach(() => {
        health = {
          claude: SIGNED_OUT("claude"),
          codex: MISSING,
          cursor: MISSING,
          grok: MISSING,
          opencode: MISSING,
        };
      });

      it("asks to connect one in place of the headline and composer", async () => {
        await renderWithAgents();
        expect(screen.getByRole("heading", { name: "Connect an agent to start" })).toBeInTheDocument();
        expect(screen.queryByTestId("composer")).toBeNull();
        expect(screen.queryByRole("button", { name: /Open folder/ })).toBeNull();
        expect(screen.getByTestId("connect-agent-claude")).toHaveTextContent("Installed · not signed in");
        expect(screen.getByTestId("connect-agent-codex")).toHaveTextContent("Not installed");
        expect(screen.getByTestId("connect-agent-opencode")).toHaveTextContent(
          "Not installed · has free models",
        );
      });

      it("signs in, installs, or opens the agent list", async () => {
        await renderWithAgents();
        fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
        expect(signInMock).toHaveBeenCalledWith("claude", "claude login", null);
        const opencode = screen.getByTestId("connect-agent-opencode");
        fireEvent.click(within(opencode).getByRole("button", { name: "Install" }));
        expect(installMock).toHaveBeenCalledWith("opencode", "OpenCode");
        fireEvent.click(screen.getByRole("button", { name: "Browse agents" }));
        expect(useUIStore.getState()).toMatchObject({ showSettings: true, settingsSection: "providers" });
      });

      it("offers a ChatGPT plan for Codex", async () => {
        const start = vi.fn().mockResolvedValue(undefined);
        useChatGptStore.setState({ start, status: null, error: null });
        await renderWithAgents();
        const codex = screen.getByTestId("connect-agent-codex");
        fireEvent.click(within(codex).getByRole("button", { name: "Use ChatGPT plan" }));
        expect(start).toHaveBeenCalledWith(null);
      });

      it("re-checks when the window regains focus and returns to the composer once one is ready", async () => {
        vi.useFakeTimers({ toFake: ["Date"] });
        await renderWithAgents();
        expect(screen.getByTestId("connect-agent")).toBeInTheDocument();
        expect(mockProbe).toHaveBeenCalledTimes(5);

        // Too soon after the last answer: no CLI spawned.
        await act(async () => {
          window.dispatchEvent(new Event("focus"));
        });
        expect(mockProbe).toHaveBeenCalledTimes(5);

        health = { ...health, claude: READY };
        vi.setSystemTime(Date.now() + 11_000);
        await act(async () => {
          window.dispatchEvent(new Event("focus"));
        });
        await flush();
        expect(mockProbe).toHaveBeenCalledTimes(10);
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        expect(screen.getByTestId("composer")).toBeInTheDocument();
        expect(screen.getByTestId("agent-status-line")).toHaveTextContent("Claude Code is ready");
      });

      it("re-checks straight away when opened again over a no-agent answer", async () => {
        const first = await renderWithAgents();
        expect(mockProbe).toHaveBeenCalledTimes(5);
        first.unmount();
        health = { ...health, opencode: READY };
        await renderWithAgents();
        expect(mockProbe).toHaveBeenCalledTimes(10);
        expect(screen.queryByTestId("connect-agent")).toBeNull();
      });

      it("can be continued past for the session when detection is wrong", async () => {
        const first = await renderWithAgents();
        fireEvent.click(screen.getByRole("button", { name: "Continue anyway" }));
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        expect(screen.getByTestId("composer")).toBeInTheDocument();
        first.unmount();
        await renderWithAgents();
        expect(screen.queryByTestId("connect-agent")).toBeNull();
      });

      it("stays away while a Hermes or custom agent listing failed, and retries it later", async () => {
        vi.useFakeTimers({ toFake: ["Date"] });
        invokeMock.mockImplementation(async (cmd: string) => {
          if (cmd === "hermes_profiles") throw new Error("hermes is busy");
          if (cmd === "acp_agents") return [];
          throw new Error(`unmocked command ${cmd}`);
        });
        const first = await renderWithAgents();
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        expect(screen.getByTestId("composer")).toBeInTheDocument();
        expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "hermes_profiles")).toHaveLength(1);
        first.unmount();

        // Opened again soon after: the failure is remembered, not retried.
        const second = await renderWithAgents();
        expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "hermes_profiles")).toHaveLength(1);
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        second.unmount();

        // Half a minute later it is listed again; an empty answer is conclusive.
        invokeMock.mockImplementation(async (cmd: string) =>
          cmd === "hermes_profiles" || cmd === "acp_agents" ? [] : Promise.reject(new Error(cmd)),
        );
        vi.setSystemTime(Date.now() + 31_000);
        await renderWithAgents();
        expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "hermes_profiles")).toHaveLength(2);
        expect(screen.getByTestId("connect-agent")).toBeInTheDocument();
      });

      it("stays away while the custom agent listing failed", async () => {
        invokeMock.mockImplementation(async (cmd: string) => {
          if (cmd === "hermes_profiles") return [];
          throw new Error("acp listing failed");
        });
        await renderWithAgents();
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        expect(screen.getByTestId("composer")).toBeInTheDocument();
      });

      it("stays away for someone with a Hermes profile", async () => {
        useHermes.setState({ profiles: [{ id: "work" } as HermesProfile] });
        await renderWithAgents();
        expect(screen.queryByTestId("connect-agent")).toBeNull();
        expect(screen.getByTestId("composer")).toBeInTheDocument();
      });

      it("stays away for someone with an enabled custom agent, not a disabled one", async () => {
        const agent = {
          id: "a", name: "Mine", executable: "mine", args: [], environment: {},
          enabled: false, auth_method: null, revision: "1",
        };
        useCustomAcp.setState({ agents: [agent] });
        const view = await renderWithAgents();
        expect(screen.getByTestId("connect-agent")).toBeInTheDocument();
        view.unmount();
        useCustomAcp.setState({ agents: [{ ...agent, enabled: true }] });
        await renderWithAgents();
        expect(screen.queryByTestId("connect-agent")).toBeNull();
      });

      it("stays a first run while the only workspace is the Home one sign-in opened", async () => {
        const view = await renderWithAgents();
        useAppStore.setState({
          appState: { workspaces: [{ workspace_type: "home" }] } as unknown as AppStateSnapshot,
        });
        view.rerender(
          <ChatHomeLanding composer={<div />} agents={{ selected: "claude" }} />,
        );
        expect(screen.getByTestId("connect-agent")).toBeInTheDocument();
      });
    });

    it("only appears on a first run, and only where the composer can switch", async () => {
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      const view = await renderWithAgents();
      expect(screen.queryByTestId("agent-status-line")).toBeNull();
      expect(mockProbe).not.toHaveBeenCalled();
      view.unmount();

      useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.queryByTestId("agent-status-line")).toBeNull();
      expect(mockProbe).not.toHaveBeenCalled();
    });
  });
