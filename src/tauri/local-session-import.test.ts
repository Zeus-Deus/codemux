import { afterEach, beforeEach, expect, it, vi } from "vitest";
import * as commands from "./commands";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: vi.fn() }));
it("imports only the explicit source IDs", async () => {
  invoke.mockResolvedValue({ imported: [], skipped: 0, warnings: [] });
  await commands.agentChatImportLocalSessions(["claude:fixture"]);
  expect(invoke).toHaveBeenCalledWith("agent_chat_import_local_sessions", { sourceIds: ["claude:fixture"] });
});
it("refuses discovery and import on remote clients before IPC", async () => {
  (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
  await expect(commands.agentChatScanLocalSessions()).rejects.toThrow("local desktop");
  await expect(commands.agentChatImportLocalSessions(["fixture"])).rejects.toThrow("local desktop");
  expect(invoke).not.toHaveBeenCalled();
});
beforeEach(() => invoke.mockReset());
afterEach(() => { delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__; });
it("scans local sessions through the explicit native command", async () => {
  invoke.mockResolvedValue({ sessions: [], warnings: [] });
  expect(await commands.agentChatScanLocalSessions()).toEqual({ sessions: [], warnings: [] });
  expect(invoke).toHaveBeenCalledWith("agent_chat_scan_local_sessions");
});
