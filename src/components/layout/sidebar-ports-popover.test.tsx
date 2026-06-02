import { describe, it, expect } from "vitest";
import { groupPorts } from "./sidebar-ports-popover";
import type { PortInfoSnapshot, WorkspaceSnapshot } from "@/tauri/types";

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
