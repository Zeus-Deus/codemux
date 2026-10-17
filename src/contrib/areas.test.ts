import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
const unlisten = vi.fn();
const listen = vi.fn(async (..._args: unknown[]) => unlisten);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a: unknown[]) => listen(...a) }));
vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() } }));

import { registry } from "./registry";
import { allKeybindEntries } from "@/lib/keybind-registry";
import { KEYBIND_REGISTRY } from "@/lib/keybind-registry";
import { canonicalCombo, pluginKeybindEntries, runPluginKeybind } from "./keybinds";
import { pluginThemes } from "./themes";
import { findPluginToolRenderer } from "./tool-renderers";
import { chatListenerCount, subscribeChatEvents } from "./chat-events";
import { appendText, focusedComposer, markComposerFocused, registerComposer } from "./composer-registry";
import { KEYBINDS_AREA, THEMES_AREA, TOOL_RENDERER_AREA } from "@/sdk/areas";

afterEach(() => {
  invoke.mockClear();
  listen.mockClear();
  unlisten.mockClear();
});

describe("plugin keybinds", () => {
  it("keeps the core registry array when no plugin adds keybinds", () => {
    expect(pluginKeybindEntries()).toEqual([]);
    expect(allKeybindEntries()).toBe(KEYBIND_REGISTRY);
  });

  it("appends plugin entries after core, then removes them", () => {
    const run = vi.fn();
    const off = registry.register({
      id: "demo:key",
      area: KEYBINDS_AREA,
      data: { label: "Demo", defaultKeys: "Ctrl+Alt+D", run },
    });
    const all = allKeybindEntries();
    expect(all.length).toBe(KEYBIND_REGISTRY.length + 1);
    // Written "Ctrl+Alt+D"; stored in the canonical order keydown produces.
    expect(all[all.length - 1]).toMatchObject({ id: "plugin:demo:key", defaultKeys: "Alt+Ctrl+D", category: "general" });
    expect(allKeybindEntries()).toBe(all);
    expect(runPluginKeybind("plugin:demo:key")).toBe(true);
    expect(run).toHaveBeenCalledOnce();
    expect(runPluginKeybind("toggle-sidebar")).toBe(false);
    off();
    expect(allKeybindEntries()).toBe(KEYBIND_REGISTRY);
    expect(runPluginKeybind("plugin:demo:key")).toBe(false);
  });
});

describe("canonicalCombo", () => {
  it("orders modifiers like keydown and normalises the key", () => {
    expect(canonicalCombo("Ctrl+Alt+e")).toBe("Alt+Ctrl+E");
    expect(canonicalCombo("shift+ctrl+k")).toBe("Ctrl+Shift+K");
    expect(canonicalCombo("Ctrl++")).toBe("Ctrl++");
    expect(canonicalCombo("F5")).toBe("F5");
    expect(canonicalCombo("")).toBe("");
  });
});

describe("plugin themes", () => {
  it("validates, namespaces and drops invalid themes", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(pluginThemes()).toEqual([]);
    const off = registry.registerMany([
      {
        id: "demo:dusk",
        area: THEMES_AREA,
        data: { label: "Dusk", scheme: "dark", roles: { background: "#101010", foreground: "#f0f0f0" } },
      },
      { id: "demo:bad", area: THEMES_AREA, data: { nope: true } },
    ]);
    const themes = pluginThemes();
    expect(themes.map((t) => t.id)).toEqual(["plugin:demo:dusk"]);
    expect(pluginThemes()).toBe(themes);
    off();
    expect(pluginThemes()).toEqual([]);
    warn.mockRestore();
  });
});

describe("plugin tool renderers", () => {
  it("first match wins and nothing matches without a renderer", () => {
    expect(findPluginToolRenderer("Bash")).toBeNull();
    const a = () => "a";
    const b = () => "b";
    const off = registry.registerMany([
      { id: "one:bash", area: TOOL_RENDERER_AREA, data: { tool: "Bash", render: a } },
      { id: "two:bash", area: TOOL_RENDERER_AREA, data: { tool: "Bash", render: b } },
    ]);
    expect(findPluginToolRenderer("Bash")).toMatchObject({ id: "one:bash", render: a });
    expect(findPluginToolRenderer("Read")).toBeNull();
    off();
    expect(findPluginToolRenderer("Bash")).toBeNull();
  });
});

describe("chat event tap", () => {
  it("turns the native tap on for the first listener and off after the last", async () => {
    const offA = subscribeChatEvents(() => {});
    const offB = subscribeChatEvents(() => {});
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("plugins_set_event_tap", { on: true });
    expect(listen).toHaveBeenCalledTimes(1);
    offA();
    expect(invoke).toHaveBeenCalledTimes(1);
    offB();
    offB();
    expect(chatListenerCount()).toBe(0);
    expect(invoke).toHaveBeenLastCalledWith("plugins_set_event_tap", { on: false });
    expect(invoke).toHaveBeenCalledTimes(2);
    await Promise.resolve();
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

describe("composer registry", () => {
  it("targets the last focused composer, else the only one", () => {
    const make = () => {
      let draft = "";
      return {
        workspaceId: null,
        threadId: null,
        getDraft: () => draft,
        insertText: (t: string) => {
          draft = appendText(draft, t);
        },
        focus: vi.fn(),
      };
    };
    expect(focusedComposer()).toBeNull();
    const a = make();
    const offA = registerComposer("a", a);
    expect(focusedComposer()).toBe(a);
    const b = make();
    const offB = registerComposer("b", b);
    expect(focusedComposer()).toBeNull();
    markComposerFocused("b");
    expect(focusedComposer()).toBe(b);
    offB();
    expect(focusedComposer()).toBe(a);
    offA();
    expect(appendText("x", "y")).toBe("x\ny");
    expect(appendText("", "y")).toBe("y");
    expect(appendText("x\n", "y")).toBe("x\ny");
  });
});
