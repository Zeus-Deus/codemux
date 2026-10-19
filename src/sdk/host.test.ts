/**
 * The host API members a Claude compaction plugin needs: `host.chat.getStatus`
 * and `host.chat.requestCompaction` (shape and IPC), and reduced motion that
 * follows both the app setting and the OS, with a subscription removed when
 * the plugin turns off.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9" }));
vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));

const settings = vi.hoisted(() => {
  const listeners = new Set<() => void>();
  const state = { settings: {} as Record<string, string>, set: () => {} };
  return {
    state,
    listeners,
    put(key: string, value: string) {
      state.settings = { ...state.settings, [key]: value };
      for (const fn of [...listeners]) fn();
    },
  };
});
vi.mock("@/stores/settings-store", () => ({
  useSettingsStore: {
    getState: () => settings.state,
    setState: vi.fn(),
    subscribe: (fn: () => void) => {
      settings.listeners.add(fn);
      return () => settings.listeners.delete(fn);
    },
  },
  selectReduceMotion: (s: { settings: Record<string, string> }) =>
    s.settings["appearance.reduce_motion"] === "true",
  SETTINGS_DEFAULTS: {},
}));
vi.mock("@/lib/themes", () => ({
  getActiveTheme: () => ({ id: "t" }),
  subscribeActiveTheme: () => () => {},
  THEME_ROLE_VARIABLES: {},
}));

const workspaces = vi.hoisted(() => {
  const listeners = new Set<() => void>();
  const state = {
    appState: {
      workspaces: [] as {
        workspace_id: string;
        title: string;
        cwd: string;
        host_id?: number;
        remote_cwd?: string;
      }[],
    },
  };
  return {
    state,
    listeners,
    notify: () => {
      for (const fn of [...listeners]) fn();
    },
  };
});
vi.mock("@/stores/app-store", () => ({
  selectActiveWorkspaceId: () => null,
  useAppStore: {
    getState: () => workspaces.state,
    subscribe: (fn: () => void) => {
      workspaces.listeners.add(fn);
      return () => workspaces.listeners.delete(fn);
    },
  },
}));

import { activatePlugin, resetActivationForTests, unloadPlugin } from "@/contrib/activation";
import { resetLifetimeForTests } from "@/contrib/lifetime";
import { resetPluginsStoreForTests } from "@/contrib/plugins-store";
import type { CodemuxPlugin } from "@/contrib/plugin";
import { host } from "./host";

/** A controllable `prefers-reduced-motion` media query. */
function installMediaQuery(initial: boolean) {
  const listeners = new Set<() => void>();
  const media = {
    matches: initial,
    media: "(prefers-reduced-motion: reduce)",
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  };
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => media),
  });
  return {
    listeners,
    set(on: boolean) {
      media.matches = on;
      for (const fn of [...listeners]) fn();
    },
  };
}

const record = { id: "demo", name: "Demo", kind: "disk" as const, tier: "local" as const };

beforeEach(() => {
  settings.state.settings = {};
  settings.listeners.clear();
});

afterEach(() => {
  resetActivationForTests();
  resetPluginsStoreForTests();
  resetLifetimeForTests();
  invoke.mockReset();
  invoke.mockImplementation(async () => undefined);
});

describe("host.chat status and compaction", () => {
  it("getStatus reads the native status command for one thread", async () => {
    const status = { provider: "claude", busy: false, queued: 0, compacting: false, status: "ready" };
    invoke.mockImplementation(async (cmd) => (cmd === "plugins_chat_status" ? status : undefined));
    await expect(host.chat.getStatus("t-1")).resolves.toEqual(status);
    expect(invoke).toHaveBeenCalledWith("plugins_chat_status", { threadId: "t-1" });
  });

  it("getStatus passes through null for a thread with no live session", async () => {
    invoke.mockImplementation(async () => null);
    await expect(host.chat.getStatus("gone")).resolves.toBeNull();
  });

  it("requestCompaction returns the app's started/refused answer as is", async () => {
    invoke.mockImplementationOnce(async () => ({ started: true }));
    await expect(host.chat.requestCompaction("t-1")).resolves.toEqual({ started: true });
    invoke.mockImplementationOnce(async () => ({ started: false, reason: "busy" }));
    await expect(host.chat.requestCompaction("t-1")).resolves.toEqual({ started: false, reason: "busy" });
    expect(invoke).toHaveBeenCalledWith("plugins_chat_request_compaction", { threadId: "t-1" });
  });

  it("ctx.host exposes the same chat members", () => {
    let seen: string[] = [];
    activatePlugin(
      {
        id: "demo",
        register(ctx) {
          seen = Object.keys(ctx.host.chat).sort();
        },
      },
      record,
    );
    expect(seen).toEqual(
      ["getStatus", "getThread", "listThreads", "onEvent", "requestCompaction", "sendTurn"].sort(),
    );
  });
});

describe("host reduced motion", () => {
  it("follows the app setting or the OS preference", () => {
    const os = installMediaQuery(false);
    expect(host.reducedMotion()).toBe(false);
    os.set(true);
    expect(host.reducedMotion()).toBe(true);
    os.set(false);
    settings.put("appearance.reduce_motion", "true");
    expect(host.reducedMotion()).toBe(true);
  });

  it("onReducedMotionChange reports each flip from either source, once", () => {
    const os = installMediaQuery(false);
    const seen: boolean[] = [];
    const off = host.onReducedMotionChange((value) => seen.push(value));
    os.set(true);
    settings.put("appearance.reduce_motion", "true"); // still reduced: no call
    os.set(false); // the setting keeps it reduced: no call
    settings.put("appearance.reduce_motion", "false");
    expect(seen).toEqual([true, false]);
    off();
    os.set(true);
    expect(seen).toEqual([true, false]);
    expect(os.listeners.size).toBe(0);
    expect(settings.listeners.size).toBe(0);
  });

  it("a plugin's reduced-motion subscription is removed when it turns off", () => {
    const os = installMediaQuery(false);
    const heard = vi.fn();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        host.onReducedMotionChange(heard);
        ctx.host.onReducedMotionChange(heard);
      },
    };
    activatePlugin(plugin, record);
    expect(os.listeners.size).toBe(2);
    os.set(true);
    expect(heard).toHaveBeenCalledTimes(2);
    unloadPlugin("demo");
    expect(os.listeners.size).toBe(0);
    expect(settings.listeners.size).toBe(0);
    os.set(false);
    expect(heard).toHaveBeenCalledTimes(2);
  });

  it("works without matchMedia", () => {
    Object.defineProperty(window, "matchMedia", { configurable: true, value: undefined });
    expect(host.reducedMotion()).toBe(false);
    const off = host.onReducedMotionChange(() => {});
    off();
  });
});

describe("host.workspace list", () => {
  it("lists local and remote paths and only notifies on list changes, until unload", () => {
    workspaces.state.appState.workspaces = [
      { workspace_id: "local", title: "Project", cwd: "/project" },
      {
        workspace_id: "ssh",
        title: "Remote",
        cwd: "/mirror",
        remote_cwd: "/remote/project",
        host_id: 42,
      },
    ];
    const callback = vi.fn();
    activatePlugin(
      {
        id: "demo",
        register(ctx) {
          ctx.host.workspace.onListChange(callback);
        },
      },
      record,
    );
    expect(host.workspace.list()).toEqual([
      { id: "local", name: "Project", path: "/project", remote: false },
      { id: "ssh", name: "Remote", path: "/remote/project", remote: true },
    ]);
    workspaces.notify();
    expect(callback).not.toHaveBeenCalled();
    workspaces.state.appState.workspaces =
      workspaces.state.appState.workspaces.slice(0, 1);
    workspaces.notify();
    expect(callback).toHaveBeenCalledWith([
      { id: "local", name: "Project", path: "/project", remote: false },
    ]);
    unloadPlugin("demo");
    expect(workspaces.listeners.size).toBe(0);
    workspaces.state.appState.workspaces = [];
    workspaces.notify();
    expect(callback).toHaveBeenCalledOnce();
  });
});
