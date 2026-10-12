/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { groupPorts, SidebarPortsPopover } from "./sidebar-ports-popover";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import type {
  AppStateSnapshot,
  PortInfoSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

function port(
  p: Partial<PortInfoSnapshot> & { port: number },
): PortInfoSnapshot {
  return {
    port: p.port,
    pid: p.pid ?? 0,
    process_name: p.process_name ?? "proc",
    workspace_id: p.workspace_id ?? null,
    label: p.label ?? null,
    source: p.source ?? null,
  };
}

// groupPorts only reads `workspace_id` and `title`, so a minimal cast keeps
// the fixtures readable without building a full WorkspaceSnapshot.
const ws = (id: string, title: string) =>
  ({ workspace_id: id, title }) as unknown as WorkspaceSnapshot;

describe("groupPorts", () => {
  it("collapses all docker ports into a single 'Docker' group regardless of workspace", () => {
    const groups = groupPorts(
      [
        port({ port: 7000, source: "docker", workspace_id: "ws1", label: "a-web-1" }),
        port({ port: 8080, source: "docker", workspace_id: "ws2", label: "b-api-1" }),
      ],
      [ws("ws1", "A"), ws("ws2", "B")],
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].workspaceName).toBe("Docker");
    expect(groups[0].ports.map((p) => p.port)).toEqual([7000, 8080]);
  });

  it("orders groups workspace → docker → other", () => {
    const groups = groupPorts(
      [
        port({ port: 5000 }), // other: no workspace, no source
        port({ port: 9000, source: "docker", workspace_id: "ws1", label: "c-1" }),
        port({ port: 3000, workspace_id: "ws1" }), // workspace
      ],
      [ws("ws1", "A")],
    );
    expect(groups.map((g) => g.workspaceName)).toEqual(["A", "Docker", "Other"]);
  });

  it("labels non-docker groups by workspace title and falls back to 'Other'", () => {
    const groups = groupPorts(
      [port({ port: 3000, workspace_id: "ws1" }), port({ port: 4000 })],
      [ws("ws1", "My Project")],
    );
    const byName = Object.fromEntries(groups.map((g) => [g.workspaceName, g]));
    expect(byName["My Project"].ports[0].port).toBe(3000);
    expect(byName["Other"].ports[0].port).toBe(4000);
  });

  it("keeps a docker port out of its workspace group even when workspace_id is set", () => {
    const groups = groupPorts(
      [
        port({ port: 3000, workspace_id: "ws1" }),
        port({ port: 8099, source: "docker", workspace_id: "ws1", label: "d-1" }),
      ],
      [ws("ws1", "A")],
    );
    const a = groups.find((g) => g.workspaceName === "A")!;
    const docker = groups.find((g) => g.workspaceName === "Docker")!;
    expect(a.ports.map((p) => p.port)).toEqual([3000]);
    expect(docker.ports.map((p) => p.port)).toEqual([8099]);
  });

  it("returns no groups for an empty port list", () => {
    expect(groupPorts([], [])).toEqual([]);
  });
});

describe("SidebarPortsPopover", () => {
  afterEach(() => {
    cleanup();
    useAppStore.setState({ appState: null });
  });

  function renderWithPorts(ports: PortInfoSnapshot[]) {
    useAppStore.setState({
      appState: { detected_ports: ports, workspaces: [] } as unknown as AppStateSnapshot,
    });
    render(
      <TooltipProvider>
        <SidebarPortsPopover />
      </TooltipProvider>,
    );
  }

  it("names the button in its tooltip, not only the count", async () => {
    renderWithPorts([port({ port: 3000 })]);
    fireEvent.focus(screen.getByRole("button", { name: "Ports" }));
    expect((await screen.findAllByText("Ports · 1 active")).length).toBeGreaterThan(0);
  });

  it("shows a port's full name only when the row truncates it, never as a native title", async () => {
    renderWithPorts([port({ port: 3000, process_name: "python" })]);
    fireEvent.click(screen.getByRole("button", { name: "Ports" }));

    const name = await screen.findByTestId("port-name");
    expect(name).not.toHaveAttribute("title");

    // jsdom lays nothing out, so the name fits: no tooltip.
    fireEvent.focus(name);
    expect(screen.queryByRole("tooltip")).toBeNull();
    fireEvent.blur(name);

    Object.defineProperty(name, "scrollWidth", { configurable: true, value: 200 });
    Object.defineProperty(name, "clientWidth", { configurable: true, value: 80 });
    fireEvent.focus(name);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("python");
  });
});
