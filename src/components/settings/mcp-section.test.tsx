/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { McpServerConfig } from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listMcpServers: vi.fn(),
    getMcpRuntimeStatus: vi.fn().mockResolvedValue([]),
    setMcpDisabledIds: vi.fn().mockResolvedValue(undefined),
    primeMcpRuntime: vi.fn().mockResolvedValue([]),
    startMcpServerCmd: vi.fn().mockResolvedValue(undefined),
    stopMcpServerCmd: vi.fn().mockResolvedValue(undefined),
    restartMcpServerCmd: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { McpSection } from "./mcp-section";
import { listMcpServers } from "@/tauri/commands";

const listMcpServersMock = listMcpServers as unknown as ReturnType<typeof vi.fn>;

function makeServer(overrides: Partial<McpServerConfig> = {}): McpServerConfig {
  return {
    id: `id-${overrides.name ?? "demo"}`,
    name: "demo",
    sources: ["codemuxUser"],
    command: "npx",
    args: ["-y", "@scope/server"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
    ...overrides,
  };
}

beforeEach(() => {
  listMcpServersMock.mockReset();
});

afterEach(() => {
  cleanup();
});

describe("McpSection", () => {
  it("renders discovered servers grouped by primary source", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "codemux-self",
        name: "codemux",
        sources: ["codemux"],
        command: "/usr/bin/codemux",
        args: ["mcp"],
      }),
      makeServer({
        id: "id-github",
        name: "github",
        sources: ["claudeUser"],
      }),
      makeServer({
        id: "id-fs",
        name: "fs",
        sources: ["cursorUser"],
        command: "mcp-fs",
        args: ["--root", "/home"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("mcp-group-codemux")).toBeInTheDocument();
    });

    expect(screen.getByText("codemux")).toBeInTheDocument();
    expect(screen.getByText("always on")).toBeInTheDocument();
    expect(screen.getByText("github")).toBeInTheDocument();
    expect(screen.getByText("fs")).toBeInTheDocument();
    expect(screen.getByTestId("mcp-group-claudeUser")).toBeInTheDocument();
    expect(screen.getByTestId("mcp-group-cursorUser")).toBeInTheDocument();
  });

  it("shows the empty state when discovery returns nothing", async () => {
    listMcpServersMock.mockResolvedValueOnce([]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("mcp-empty")).toBeInTheDocument();
    });
  });

  it("surfaces errors from the backend command", async () => {
    listMcpServersMock.mockRejectedValueOnce(new Error("boom"));

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("mcp-error")).toBeInTheDocument();
    });
    expect(screen.getByTestId("mcp-error").textContent).toContain("boom");
  });

  it("re-fetches when Refresh is clicked", async () => {
    listMcpServersMock.mockResolvedValue([
      makeServer({ id: "id-1", name: "first", sources: ["codemuxUser"] }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(listMcpServersMock).toHaveBeenCalledTimes(1);
    });

    fireEvent.click(screen.getByLabelText("Refresh MCP servers"));

    await waitFor(() => {
      expect(listMcpServersMock).toHaveBeenCalledTimes(2);
    });
  });

  it("renders an HTTP badge for http transports", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-linear",
        name: "linear",
        sources: ["claudeUser"],
        command: "https://mcp.linear.app/mcp",
        args: [],
        transport: "http",
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByText("HTTP")).toBeInTheDocument();
    });
    expect(screen.getByText("https://mcp.linear.app/mcp")).toBeInTheDocument();
  });

  it("renders the command preview with args", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-with-args",
        name: "withargs",
        sources: ["codemuxUser"],
        command: "npx",
        args: ["-y", "@scope/srv"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByText("npx -y @scope/srv")).toBeInTheDocument();
    });
  });

  it("forwards the project root to the backend", async () => {
    listMcpServersMock.mockResolvedValueOnce([]);

    render(<McpSection projectRoot="/home/me/proj" />);

    await waitFor(() => {
      expect(listMcpServersMock).toHaveBeenCalledWith("/home/me/proj");
    });
  });

  // ── Dedupe / collision behavior ─────────────────────────────────────

  it("renders an 'also:' label for deduped multi-source rows", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-omarchy",
        name: "omarchy-kb",
        sources: ["claudeUser", "cursorUser"],
        command: "docker",
        args: ["exec", "-i", "x"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(
        screen.getByTestId("mcp-row-id-omarchy-extra-sources"),
      ).toBeInTheDocument();
    });
    const label = screen.getByTestId("mcp-row-id-omarchy-extra-sources");
    expect(label.textContent).toContain("also: Cursor · User");

    // Row lives in the canonical (Claude · User) group, NOT a Cursor group.
    expect(screen.getByTestId("mcp-group-claudeUser")).toBeInTheDocument();
    expect(screen.queryByTestId("mcp-group-cursorUser")).not.toBeInTheDocument();
  });

  it("shows source disambiguators when two rows share a name", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-a",
        name: "omarchy-kb",
        sources: ["claudeUser"],
        command: "docker",
        args: ["exec", "-i", "v1"],
      }),
      makeServer({
        id: "id-b",
        name: "omarchy-kb",
        sources: ["cursorUser"],
        command: "docker",
        args: ["exec", "-i", "v2"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("mcp-row-id-a-disambig")).toBeInTheDocument();
    });
    expect(
      screen.getByTestId("mcp-row-id-a-disambig").textContent,
    ).toContain("Claude · User");
    expect(
      screen.getByTestId("mcp-row-id-b-disambig").textContent,
    ).toContain("Cursor · User");
  });

  // ── Stage 2 runtime + toggle behavior ───────────────────────────────

  it("renders 'always on' badge for the Codemux self row and hides the toggle", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "codemux-self",
        name: "codemux",
        sources: ["codemux"],
        command: "/usr/bin/codemux",
        args: ["mcp"],
      }),
      makeServer({
        id: "id-other",
        name: "other",
        sources: ["claudeUser"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByText("always on")).toBeInTheDocument();
    });
    // Self row: no toggle.
    expect(
      screen.queryByTestId("mcp-row-codemux-self-toggle"),
    ).not.toBeInTheDocument();
    // Other row: toggle present.
    expect(screen.getByTestId("mcp-row-id-other-toggle")).toBeInTheDocument();
  });

  it("clicking the toggle on a row calls the runtime stop command", async () => {
    const { stopMcpServerCmd } = (await import(
      "@/tauri/commands"
    )) as unknown as Record<string, ReturnType<typeof vi.fn>>;

    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-toggle",
        name: "toggleable",
        sources: ["claudeUser"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    const toggle = await screen.findByTestId("mcp-row-id-toggle-toggle");
    // Switch starts checked (enabled). Click → disable → stop call.
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(stopMcpServerCmd).toHaveBeenCalledWith("id-toggle");
    });
  });

  it("does NOT show a disambiguator when a name appears only once", async () => {
    listMcpServersMock.mockResolvedValueOnce([
      makeServer({
        id: "id-solo",
        name: "solo",
        sources: ["claudeUser"],
      }),
    ]);

    render(<McpSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByText("solo")).toBeInTheDocument();
    });
    expect(
      screen.queryByTestId("mcp-row-id-solo-disambig"),
    ).not.toBeInTheDocument();
  });
});
