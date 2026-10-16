import { afterEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  calls.push(args?.phase ? `${cmd}:${args.phase}` : cmd);
  if (cmd === "plugins_list") return [];
  if (cmd === "plugins_get_decisions") return {};
  return undefined;
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() } }));

import { resetBootForTests, resumePlugins } from "./boot";
import { resetPluginsStoreForTests, usePluginsStore } from "./plugins-store";

afterEach(() => {
  resetBootForTests();
  resetPluginsStoreForTests();
  calls.length = 0;
  vi.useRealTimers();
});

describe("Resume plugins", () => {
  it("re-arms the crash marker before lifting the pause, then clears it after 10 s", async () => {
    vi.useFakeTimers();
    usePluginsStore.setState({ paused: true, pausedReason: "crashed", decisionsLoaded: true });
    await resumePlugins();
    expect(calls.indexOf("plugins_startup_checkpoint:loading")).toBeGreaterThanOrEqual(0);
    expect(calls.indexOf("plugins_startup_checkpoint:loading")).toBeLessThan(calls.indexOf("plugins_resume"));
    expect(usePluginsStore.getState().paused).toBe(false);
    expect(calls).not.toContain("plugins_startup_checkpoint:ok");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls).toContain("plugins_startup_checkpoint:ok");
  });

  it("keeps the pause when the marker can't be written", async () => {
    invoke.mockImplementationOnce(async () => {
      throw new Error("read-only disk");
    });
    usePluginsStore.setState({ paused: true, decisionsLoaded: true });
    await expect(resumePlugins()).rejects.toThrow("read-only disk");
    expect(usePluginsStore.getState().paused).toBe(true);
  });
});
