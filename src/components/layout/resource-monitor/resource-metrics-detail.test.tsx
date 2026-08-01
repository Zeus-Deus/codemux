/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ResourceMetricsSnapshot } from "@/tauri/types";

// Which detail level the monitor asks the backend for. Closed, only the
// host share and the severity dot are on screen, so the poll must not pay
// for a full process-table walk + `smaps_rollup` read per PID.

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: vi.fn(() => true),
  selectShowResourceMonitor: vi.fn(),
}));

const getResourceMetricsMock = vi.fn();

vi.mock("@/tauri/commands", () => ({
  // Unlike the sibling suite's mock, this one forwards the argument.
  getResourceMetrics: (detail?: boolean) => getResourceMetricsMock(detail),
  activateWorkspace: vi.fn(),
  activateTerminalSession: vi.fn(),
}));

import { ResourceMonitor } from "./ResourceMonitor";

function makeSnapshot(): ResourceMetricsSnapshot {
  return {
    app: {
      cpu: 1,
      memory: 100,
      main: { cpu: 1, memory: 100 },
      web_view: { cpu: 0, memory: 0 },
      other: { cpu: 0, memory: 0 },
    },
    workspaces: [],
    host: {
      total_memory: 16 * 1024 * 1024 * 1024,
      free_memory: 8 * 1024 * 1024 * 1024,
      used_memory: 8 * 1024 * 1024 * 1024,
      memory_usage_percent: 50,
      cpu_core_count: 8,
      load_average_1m: 1.5,
    },
    total_cpu: 1,
    total_memory: 100,
    collected_at: 1_700_000_000_000,
  };
}

function renderMonitor() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: 0, refetchInterval: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <ResourceMonitor />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("ResourceMonitor detail level", () => {
  beforeEach(() => {
    getResourceMetricsMock.mockReset();
    getResourceMetricsMock.mockResolvedValue(makeSnapshot());
  });

  afterEach(() => {
    cleanup();
  });

  it("asks for a summary while the popover is closed", async () => {
    renderMonitor();
    await waitFor(() => expect(getResourceMetricsMock).toHaveBeenCalled());
    expect(getResourceMetricsMock).toHaveBeenCalledWith(false);
    expect(getResourceMetricsMock).not.toHaveBeenCalledWith(true);
  });

  it("asks for detail as soon as the popover opens", async () => {
    renderMonitor();
    await waitFor(() => expect(getResourceMetricsMock).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Resource monitor" }));
    await waitFor(() =>
      expect(getResourceMetricsMock).toHaveBeenCalledWith(true),
    );
  });
});
