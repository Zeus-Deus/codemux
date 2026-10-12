import { beforeAll, expect, it } from "vitest";
import { listen } from "@tauri-apps/api/event";
import type { AppStateSnapshot } from "@/tauri/types";
import type { HostStatusView, HostTestResult, HostView } from "@/tauri/commands";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;
beforeAll(async () => {
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});

it("walks a new device through add, install and remove", async () => {
  const host = await invoke("hosts_add", { name: "homelab", sshTarget: "homelab" }) as HostView;
  expect((await invoke("hosts_list") as HostView[]).map((h) => h.name)).toContain("homelab");

  const probe = await invoke("hosts_test_connection", { id: host.id }) as HostTestResult;
  expect(probe).toMatchObject({ ok: false, needs_install: true, uname: "Linux x86_64" });

  const pushed: HostStatusView[][] = [];
  const unlisten = await listen<HostStatusView[]>("hosts-status-changed", (e) => pushed.push(e.payload));
  await invoke("hosts_bootstrap_install", { id: host.id, uname: probe.uname });
  unlisten();
  expect(pushed[pushed.length - 1]?.find((s) => s.host_id === host.id)?.last_error).toBeNull();
  expect(await invoke("hosts_test_connection", { id: host.id })).toMatchObject({ ok: true, needs_install: false });

  const renamed = await invoke("hosts_update", { id: host.id, name: "lab", sshTarget: "homelab" }) as HostView;
  expect(renamed.name).toBe("lab");

  await invoke("hosts_delete", { id: host.id });
  expect((await invoke("hosts_list") as HostView[]).some((h) => h.id === host.id)).toBe(false);
  expect((await invoke("hosts_status_list") as HostStatusView[]).some((s) => s.host_id === host.id)).toBe(false);
});

it("names this machine and suggests ssh config hosts", async () => {
  expect(await invoke("get_local_device_name")).toBe("ai-node");
  expect(await invoke("hosts_ssh_config_hosts")).toContain("deus@zeus");
});

it("creates an attach-only workspace on a device with the chat bound", async () => {
  const result = await invoke("create_workspace_on_host", {
    hostId: 2,
    projectPath: "/home/dev/projects/codemux",
    branch: "fix-login",
    newBranch: true,
    baseBranch: null,
    initialChat: { provider: "codex", thread_id: "thread-remote-1" },
  }) as { workspace_id: string; cwd: string; adopted: boolean };
  expect(result).toMatchObject({
    cwd: "/home/deus/.codemux/worktrees/codemux/fix-login",
    adopted: false,
  });

  const state = await invoke("get_app_state") as AppStateSnapshot;
  const ws = state.workspaces.find((w) => w.workspace_id === result.workspace_id)!;
  expect(ws).toMatchObject({
    host_id: 2,
    attach_only: true,
    remote_cwd: result.cwd,
    cwd: result.cwd,
    project_root: "/home/dev/projects/codemux",
  });
  const root = ws.surfaces[0].root;
  expect(root.kind === "agent_chat" && root.thread_id).toBe("thread-remote-1");
});
