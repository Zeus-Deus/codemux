import { beforeAll, expect, it } from "vitest";
import type { AppStateSnapshot } from "@/tauri/types";
import type { AgentChatMessageRow } from "@/tauri/commands";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;
beforeAll(async () => {
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});

it("seeds two independent chats and switches the active tab and surface through IPC", async () => {
  const state = await invoke("get_app_state") as AppStateSnapshot;
  const workspace = state.workspaces.find((w) => w.workspace_id === "ws-codemux-chat")!;
  expect(workspace.tabs).toHaveLength(2);
  const threads = workspace.surfaces.map((s) => s.root.kind === "agent_chat" ? s.root.thread_id : null);
  expect(new Set(threads).size).toBe(2);
  for (const threadId of threads) {
    const rows = await invoke("agent_chat_list_messages_after", { threadId, afterId: null }) as AgentChatMessageRow[];
    expect(rows.length).toBeGreaterThan(0);
    expect([...new Set(rows.map((row) => JSON.parse(row.payload).thread_id))]).toEqual([threadId]);
  }
  const original = workspace.tabs[0];
  const other = workspace.tabs[1];
  await invoke("activate_tab", { workspaceId: workspace.workspace_id, tabId: other.tab_id });
  const switched = await invoke("get_app_state") as AppStateSnapshot;
  const active = switched.workspaces.find((w) => w.workspace_id === workspace.workspace_id)!;
  expect(active.active_tab_id).toBe(other.tab_id);
  expect(active.active_surface_id).toBe(other.surface_id);
  await invoke("activate_tab", { workspaceId: workspace.workspace_id, tabId: original.tab_id });
  const restored = await invoke("get_app_state") as AppStateSnapshot;
  expect(restored.workspaces.find((w) => w.workspace_id === workspace.workspace_id)!.active_tab_id).toBe(original.tab_id);
});

it("does not corrupt tab selection for an unknown tab", async () => {
  const before = await invoke("get_app_state") as AppStateSnapshot;
  const workspace = before.workspaces.find((w) => w.workspace_id === "ws-codemux-chat")!;
  const selected = [workspace.active_tab_id, workspace.active_surface_id];
  await invoke("activate_tab", { workspaceId: workspace.workspace_id, tabId: "missing-tab" });
  const after = await invoke("get_app_state") as AppStateSnapshot;
  const unchanged = after.workspaces.find((w) => w.workspace_id === workspace.workspace_id)!;
  expect([unchanged.active_tab_id, unchanged.active_surface_id]).toEqual(selected);
});
