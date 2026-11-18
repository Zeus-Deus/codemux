import { beforeAll, expect, it } from "vitest";
import type { AppStateSnapshot } from "@/tauri/types";
import { openEditorTab } from "@/lib/open-editor-tab";
import { useEditorStore } from "@/stores/editor-store";
import { closeTab } from "@/tauri/commands";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;

beforeAll(async () => {
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});

it("opens a real selectable editor tab through the same helper as file search", async () => {
  const before = await invoke("get_app_state") as AppStateSnapshot;
  const workspace = before.workspaces.find((candidate) => candidate.workspace_id === "ws-codemux-chat")!;
  const tabId = await openEditorTab(workspace.workspace_id, workspace.tabs, `${workspace.cwd}/src/stores/ui-store.ts`);
  try {
    const opened = await invoke("get_app_state") as AppStateSnapshot;
    const current = opened.workspaces.find((candidate) => candidate.workspace_id === workspace.workspace_id)!;
    expect(current.active_tab_id).toBe(tabId);
    expect(current.tabs.find((tab) => tab.tab_id === tabId)).toMatchObject({ kind: "editor", title: "ui-store.ts", surface_id: null });
    expect(opened.terminal_sessions).toHaveLength(before.terminal_sessions.length);
    expect(await openEditorTab(current.workspace_id, current.tabs, `${workspace.cwd}/src/stores/ui-store.ts`)).toBe(tabId);
    expect((await invoke("get_app_state") as AppStateSnapshot).workspaces.find((candidate) => candidate.workspace_id === workspace.workspace_id)!.tabs).toHaveLength(workspace.tabs.length + 1);
  } finally {
    await closeTab(workspace.workspace_id, tabId);
  }
  expect(useEditorStore.getState().getTab(tabId)).toBeUndefined();
});

it("persists mock editor writes for subsequent reads and changes the disk signature", async () => {
  const path = "/mock/editor-smoke.ts";
  const signature = await invoke("file_signature", { path });
  await invoke("write_file", { path, content: "export const audited = true;\n" });
  expect(await invoke("read_file", { path })).toBe("export const audited = true;\n");
  expect(await invoke("file_signature", { path })).not.toBe(signature);
});

it("leaves the editor open when the save it is waiting for fails", async () => {
  const snapshot = await invoke("get_app_state") as AppStateSnapshot;
  const workspace = snapshot.workspaces.find((candidate) => candidate.workspace_id === "ws-codemux-chat")!;
  const tabId = await openEditorTab(workspace.workspace_id, workspace.tabs, `${workspace.cwd}/notes.md`);
  const editor = useEditorStore.getState();
  const save = editor.beginSave(tabId, "unsaved edit");
  const closing = closeTab(workspace.workspace_id, tabId);
  save.finish();

  await expect(closing).rejects.toThrow(/unsaved changes/i);
  const after = await invoke("get_app_state") as AppStateSnapshot;
  const remaining = after.workspaces.find((ws) => ws.workspace_id === workspace.workspace_id)!.tabs;
  expect(remaining.map((tab) => tab.tab_id)).toContain(tabId);
  expect(editor.getTab(tabId)).toMatchObject({ suspendedDraft: "unsaved edit", isDirty: true });
  await closeTab(workspace.workspace_id, tabId);
});
