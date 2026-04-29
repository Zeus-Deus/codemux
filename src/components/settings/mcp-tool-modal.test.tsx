/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";

import type {
  McpServerConfig,
  McpServerRuntime,
  McpTool,
} from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listMcpToolsForServer: vi.fn(),
  };
});

import { McpToolModal } from "./mcp-tool-modal";
import { listMcpToolsForServer } from "@/tauri/commands";

const listMcpToolsForServerMock =
  listMcpToolsForServer as unknown as ReturnType<typeof vi.fn>;

function makeServer(overrides: Partial<McpServerConfig> = {}): McpServerConfig {
  return {
    id: "id-codemux",
    name: "codemux",
    sources: ["codemux"],
    command: "/usr/bin/codemux",
    args: ["mcp"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
    ...overrides,
  };
}

function makeRuntime(overrides: Partial<McpServerRuntime> = {}): McpServerRuntime {
  return {
    id: "id-codemux",
    name: "codemux",
    status: { kind: "running", toolCount: 2 },
    toolsCount: 2,
    errorMessage: null,
    stderrTail: null,
    startedAtMs: 1,
    ...overrides,
  };
}

function makeTool(name: string, description: string | null = null): McpTool {
  return {
    name,
    prefixedName: `mcp__codemux__${name}`,
    description,
    inputSchema: {},
    serverId: "id-codemux",
  };
}

beforeEach(() => {
  listMcpToolsForServerMock.mockReset();
});

afterEach(() => {
  cleanup();
});

describe("McpToolModal", () => {
  it("renders nothing when server is null", () => {
    render(
      <McpToolModal server={null} runtime={null} onClose={() => {}} />,
    );
    expect(screen.queryByTestId("mcp-tool-modal-list")).not.toBeInTheDocument();
  });

  it("renders the tool list with prefixed names + descriptions", async () => {
    listMcpToolsForServerMock.mockResolvedValueOnce([
      makeTool("browser_navigate", "Navigate the browser"),
      makeTool("browser_screenshot"),
    ]);
    render(
      <McpToolModal
        server={makeServer()}
        runtime={makeRuntime()}
        onClose={() => {}}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("mcp-tool-modal-list")).toBeInTheDocument();
    });
    expect(screen.getByText("mcp__codemux__browser_navigate")).toBeInTheDocument();
    expect(screen.getByText("Navigate the browser")).toBeInTheDocument();
    expect(screen.getByText("mcp__codemux__browser_screenshot")).toBeInTheDocument();
  });

  it("shows the empty state when server has no tools", async () => {
    listMcpToolsForServerMock.mockResolvedValueOnce([]);
    render(
      <McpToolModal
        server={makeServer()}
        runtime={makeRuntime()}
        onClose={() => {}}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("mcp-tool-modal-empty")).toBeInTheDocument();
    });
    expect(screen.getByTestId("mcp-tool-modal-empty").textContent).toContain(
      "No tools exposed",
    );
  });

  it("shows the not-running message for non-running servers", async () => {
    listMcpToolsForServerMock.mockResolvedValueOnce([]);
    render(
      <McpToolModal
        server={makeServer()}
        runtime={makeRuntime({ status: { kind: "errored", message: "boom" } })}
        onClose={() => {}}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("mcp-tool-modal-empty")).toBeInTheDocument();
    });
    expect(screen.getByTestId("mcp-tool-modal-empty").textContent).toContain(
      "Server not running",
    );
  });

  it("surfaces backend errors", async () => {
    listMcpToolsForServerMock.mockRejectedValueOnce(new Error("io"));
    render(
      <McpToolModal
        server={makeServer()}
        runtime={makeRuntime()}
        onClose={() => {}}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("mcp-tool-modal-error")).toBeInTheDocument();
    });
    expect(screen.getByTestId("mcp-tool-modal-error").textContent).toContain("io");
  });
});
