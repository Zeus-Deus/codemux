/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri command surface BEFORE importing the store so the
// store's module-level capture of `setMcpDisabledIds` resolves to the
// mock, not the real Tauri invoke (which would throw under jsdom).
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    setMcpDisabledIds: vi.fn().mockResolvedValue(undefined),
  };
});

import { useMcpStore } from "./mcp-store";
import { setMcpDisabledIds, MCP_CODEMUX_SELF_ID } from "@/tauri/commands";

const setMcpDisabledIdsMock =
  setMcpDisabledIds as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => {
  // Reset persisted state between tests.
  useMcpStore.setState({ disabledIds: [] });
  setMcpDisabledIdsMock.mockClear();
  localStorage.clear();
});

afterEach(() => {
  useMcpStore.setState({ disabledIds: [] });
});

describe("useMcpStore", () => {
  it("toggle adds an id and stores sorted", () => {
    useMcpStore.getState().toggleDisabled("zeta");
    useMcpStore.getState().toggleDisabled("alpha");
    expect(useMcpStore.getState().disabledIds).toEqual(["alpha", "zeta"]);
  });

  it("toggle removes an already-present id", () => {
    useMcpStore.getState().toggleDisabled("github");
    useMcpStore.getState().toggleDisabled("github");
    expect(useMcpStore.getState().disabledIds).toEqual([]);
  });

  it("isDisabled returns true for disabled ids and false otherwise", () => {
    useMcpStore.getState().toggleDisabled("github");
    expect(useMcpStore.getState().isDisabled("github")).toBe(true);
    expect(useMcpStore.getState().isDisabled("other")).toBe(false);
  });

  it("Codemux self can never be disabled via the action", () => {
    useMcpStore.getState().toggleDisabled(MCP_CODEMUX_SELF_ID);
    expect(useMcpStore.getState().disabledIds).toEqual([]);
    expect(useMcpStore.getState().isDisabled(MCP_CODEMUX_SELF_ID)).toBe(false);
  });

  it("Codemux self is never reported disabled even if it sneaks into state", () => {
    // Backdoor: bypass the action so we can prove the read-side guard.
    useMcpStore.setState({ disabledIds: [MCP_CODEMUX_SELF_ID] });
    expect(useMcpStore.getState().isDisabled(MCP_CODEMUX_SELF_ID)).toBe(false);
  });

  it("toggle syncs to backend with the new disabledIds", async () => {
    useMcpStore.getState().toggleDisabled("github");
    await Promise.resolve(); // let the fire-and-forget promise settle
    expect(setMcpDisabledIdsMock).toHaveBeenCalledWith(["github"]);
  });

  it("syncToBackend pushes the current set to the runtime", async () => {
    useMcpStore.setState({ disabledIds: ["a", "b"] });
    await useMcpStore.getState().syncToBackend();
    expect(setMcpDisabledIdsMock).toHaveBeenCalledWith(["a", "b"]);
  });
});
