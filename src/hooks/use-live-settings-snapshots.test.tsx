import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Event } from "@tauri-apps/api/event";
import type { McpServerRuntime } from "@/tauri/commands";
import type { PresetStoreSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  mcp: vi.fn<() => Promise<McpServerRuntime[]>>(),
  presets: vi.fn<() => Promise<PresetStoreSnapshot>>(),
  listenMcp: vi.fn<(event: string, callback: (event: Event<McpServerRuntime>) => void) => Promise<() => void>>(),
  listenPresets: vi.fn<(callback: (snapshot: PresetStoreSnapshot) => void) => Promise<() => void>>(),
}));

vi.mock("@/tauri/commands", () => ({ getMcpRuntimeStatus: mocks.mcp, getPresets: mocks.presets, MCP_STATUS_CHANGED_EVENT: "mcp-status-changed" }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listenMcp }));
vi.mock("@/tauri/events", () => ({ onPresetsChanged: mocks.listenPresets }));

import { useMcpRuntime } from "./use-mcp-runtime";
import { usePresetStore } from "./use-preset-store";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const runtime = (id: string, status: McpServerRuntime["status"]): McpServerRuntime => ({
  id, name: id, status, toolsCount: status.kind === "running" ? status.toolCount : 0,
  errorMessage: null, stderrTail: null, startedAtMs: null,
});

describe("live settings snapshot hydration", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocks.listenMcp.mockResolvedValue(() => {});
    mocks.listenPresets.mockResolvedValue(() => {});
  });
  afterEach(cleanup);

  it("merges MCP events received during hydration over the stale snapshot", async () => {
    const pending = deferred<McpServerRuntime[]>();
    mocks.mcp.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useMcpRuntime());
    const live = runtime("alpha", { kind: "running", toolCount: 5 });
    act(() => { mocks.listenMcp.mock.calls[0][1]({ event: "mcp-status-changed", id: 1, payload: live }); });
    await act(async () => { pending.resolve([runtime("alpha", { kind: "starting" }), runtime("beta", { kind: "discovered" })]); });
    expect(result.current.loaded).toBe(true);
    expect(result.current.runtimes.get("alpha")).toEqual(live);
    expect(result.current.runtimes.has("beta")).toBe(true);
  });

  it("retains a newly discovered MCP server absent from the initial snapshot", async () => {
    const pending = deferred<McpServerRuntime[]>();
    mocks.mcp.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useMcpRuntime());
    const live = runtime("new-server", { kind: "discovered" });
    act(() => { mocks.listenMcp.mock.calls[0][1]({ event: "mcp-status-changed", id: 1, payload: live }); });
    await act(async () => { pending.resolve([]); });
    expect(result.current.runtimes.get("new-server")).toEqual(live);
  });

  it("does not overwrite changed presets with the earlier mount snapshot", async () => {
    const pending = deferred<PresetStoreSnapshot>();
    mocks.presets.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => usePresetStore());
    const live: PresetStoreSnapshot = { presets: [], bar_visible: true, default_preset_id: "new" };
    act(() => { mocks.listenPresets.mock.calls[0][0](live); });
    await act(async () => { pending.resolve({ presets: [], bar_visible: false, default_preset_id: "old" }); });
    expect(result.current).toEqual(live);
  });
});
