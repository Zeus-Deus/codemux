import { afterEach, beforeAll, expect, it } from "vitest";
import type { AppStateSnapshot } from "@/tauri/types";
import type { LocalChatScanResult, LocalChatImportResult, AgentChatSessionRecord } from "@/tauri/commands";
import { replayPayloads } from "@/lib/agent-chat/hydrate";
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
let invoke: Invoke;
beforeAll(async () => {
  window.history.replaceState({}, "", "?fixture=session-import");
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});
afterEach(() => { localStorage.clear(); });
it("starts the importer demo with zero workspaces", async () => {
  const state = await invoke<AppStateSnapshot>("get_app_state");
  expect(state.workspaces).toEqual([]);
});
it("copies selected synthetic sessions, hydrates read-only text, and deduplicates", async () => {
  const scan = await invoke<LocalChatScanResult>("agent_chat_scan_local_sessions");
  expect(scan.sessions).toHaveLength(3);
  const sourceIds = scan.sessions.slice(0, 2).map((s) => s.source_id);
  const result = await invoke<LocalChatImportResult>("agent_chat_import_local_sessions", { sourceIds });
  expect(result.imported).toHaveLength(2);
  const state = await invoke<AppStateSnapshot>("get_app_state");
  expect(state.workspaces).toHaveLength(1);
  const record = await invoke<AgentChatSessionRecord>("agent_chat_get_session", { threadId: result.imported[0].thread_id });
  expect(record.imported_from).toBe(sourceIds[0]);
  const payloads = await invoke<string[]>("agent_chat_list_messages", { threadId: result.imported[0].thread_id });
  const slice = replayPayloads(payloads);
  expect(slice.messages.map((m) => m.kind)).toEqual(["user_message", "assistant_message"]);
  expect(JSON.stringify(slice.messages)).toContain("synthetic");
  const again = await invoke<LocalChatImportResult>("agent_chat_import_local_sessions", { sourceIds });
  expect(again.imported).toHaveLength(0);
  expect(again.skipped).toBe(2);
  const after = await invoke<LocalChatScanResult>("agent_chat_scan_local_sessions");
  expect(after.sessions[0].already_imported).toBe(true);
  await invoke("agent_chat_open_search_result", { threadId: result.imported[1].thread_id });
  expect((await invoke<AppStateSnapshot>("get_app_state")).active_workspace_id).toBe(result.imported[1].workspace_id);
});
