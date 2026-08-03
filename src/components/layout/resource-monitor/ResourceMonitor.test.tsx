/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ResourceMetricsSnapshot } from "@/tauri/types";

// ── Mocks ──

let showResourceMonitor = true;

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: vi.fn(() => showResourceMonitor),
  selectShowResourceMonitor: vi.fn(),
}));

const getResourceMetricsMock = vi.fn();
const activateWorkspaceMock = vi.fn();
const activateTerminalSessionMock = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getResourceMetrics: () => getResourceMetricsMock(),
  activateWorkspace: (id: string) => activateWorkspaceMock(id),
  activateTerminalSession: (id: string) => activateTerminalSessionMock(id),
}));

import { ResourceMonitor } from "./ResourceMonitor";

// ── Fixtures ──

function makeSnapshot(): ResourceMetricsSnapshot {
  return {
    app: {
      cpu: 12.5,
      memory: 480 * 1024 * 1024,
      main: { cpu: 8, memory: 300 * 1024 * 1024 },
      web_view: { cpu: 4, memory: 160 * 1024 * 1024 },
      other: { cpu: 0.5, memory: 20 * 1024 * 1024 },
    },
    workspaces: [
      {
        workspace_id: "ws-1",
        project_id: "/home/dev/codemux",
        project_name: "codemux",
        workspace_name: "feature-branch",
        cpu: 30,
        memory: 600 * 1024 * 1024,
        sessions: [
          {
            session_id: "sess-abc12345",
            pane_id: "pane-1",
            pid: 4242,
            title: "dev server",
            cpu: 30,
            memory: 600 * 1024 * 1024,
          },
        ],
      },
    ],
    host: {
      total_memory: 16 * 1024 * 1024 * 1024,
      free_memory: 8 * 1024 * 1024 * 1024,
      used_memory: 8 * 1024 * 1024 * 1024,
      memory_usage_percent: 50,
      cpu_core_count: 8,
      load_average_1m: 1.5,
    },
    total_cpu: 42.5,
    total_memory: 1080 * 1024 * 1024,
    collected_at: 1_700_000_000_000,
  };
}

function renderMonitor(variant?: "ghost" | "outline" | "toolbar") {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity, refetchInterval: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <ResourceMonitor variant={variant} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Tests ──

describe("ResourceMonitor", () => {
  beforeEach(() => {
    showResourceMonitor = true;
    getResourceMetricsMock.mockReset();
    activateWorkspaceMock.mockReset();
    activateTerminalSessionMock.mockReset();
    getResourceMetricsMock.mockResolvedValue(makeSnapshot());
    activateWorkspaceMock.mockResolvedValue(undefined);
    activateTerminalSessionMock.mockResolvedValue(undefined);
  });

  // No `globals: true` in vite.config, so RTL's auto-cleanup never
  // registers — unmount explicitly so portaled popover content from
  // one test doesn't leak into the next.
  afterEach(() => {
    cleanup();
  });

  it("renders nothing when the setting is disabled", () => {
    showResourceMonitor = false;
    const { container } = renderMonitor();
    expect(container).toBeEmptyDOMElement();
    expect(getResourceMetricsMock).not.toHaveBeenCalled();
  });

  it("renders the title-bar trigger button when enabled", () => {
    renderMonitor();
    const trigger = screen.getByRole("button", { name: "Resource monitor" });
    expect(trigger).toBeInTheDocument();
    expect(trigger).toHaveAttribute("data-variant", "ghost");
  });

  it("renders a toolbar trigger when requested", () => {
    renderMonitor("toolbar");
    expect(
      screen.getByRole("button", { name: "Resource monitor" }),
    ).toHaveAttribute("data-variant", "toolbar");
  });

  it("opens the popover and shows the app + workspace breakdown", async () => {
    renderMonitor();
    fireEvent.click(screen.getByRole("button", { name: "Resource monitor" }));

    expect(await screen.findByText("Resources")).toBeInTheDocument();
    // App + workspace breakdown appears once the async query resolves.
    expect(await screen.findByText("Codemux App")).toBeInTheDocument();
    // Project group, workspace, and session rows from the snapshot.
    expect(screen.getByText("codemux")).toBeInTheDocument();
    expect(screen.getByText("feature-branch")).toBeInTheDocument();
    expect(screen.getByText("dev server")).toBeInTheDocument();
  });

  it("activates the terminal session when a session row is clicked", async () => {
    renderMonitor();
    fireEvent.click(screen.getByRole("button", { name: "Resource monitor" }));

    const sessionRow = await screen.findByText("dev server");
    fireEvent.click(sessionRow);
    expect(activateTerminalSessionMock).toHaveBeenCalledWith("sess-abc12345");
  });

  it("shows an empty state when there are no terminal sessions", async () => {
    const snapshot = makeSnapshot();
    snapshot.workspaces = [];
    getResourceMetricsMock.mockResolvedValue(snapshot);

    renderMonitor();
    fireEvent.click(screen.getByRole("button", { name: "Resource monitor" }));

    expect(
      await screen.findByText("No active terminal sessions"),
    ).toBeInTheDocument();
  });
});
