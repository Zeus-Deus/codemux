import { describe, expect, it } from "vitest";
import { connectionFailure, controlFiles, isLinuxWayland, parseDoctor, parseVersion, readFrame, readWindows } from "./model";

const target = { pid: 42, windowId: 123, title: "Demo", appName: "Demo App" };

describe("computer-use driver results", () => {
  it("limits Wayland-specific copy to positively identified Linux Wayland sessions", () => {
    const report = (binary: string, display: string) => ({ ok: true, probes: [
      { label: "binary", status: "ok" as const, message: binary },
      { label: "display server", status: "ok" as const, message: display },
    ] });
    expect(isLinuxWayland(report("cua-driver 0.21.0 (x86_64-linux)", "Wayland+XWayland (WAYLAND_DISPLAY=wayland-1, DISPLAY=:0)"))).toBe(true);
    expect(isLinuxWayland(report("cua-driver 0.21.0 (x86_64-linux)", "X11 (DISPLAY=:0)"))).toBe(false);
    expect(isLinuxWayland(report("cua-driver 0.21.0 (aarch64-macos)", "WindowServer"))).toBe(false);
    expect(isLinuxWayland(report("cua-driver 0.21.0 (x86_64-windows)", "Win32"))).toBe(false);
    expect(isLinuxWayland(null)).toBe(false);
  });
  it("keeps a doctor warning even when the driver's overall ok flag is true", () => {
    const probes = [{ label: "X11 connection", status: "warn", message: "no top-level windows returned" }];
    expect(parseDoctor(JSON.stringify({ ok: true, probes }))).toEqual({ ok: true, probes });
    expect(parseDoctor('{"ok":true,"probes":[{}]}')).toBeNull();
    expect(parseVersion("cua-driver 0.21.0\n")).toBe("0.21.0");
    expect(parseVersion("command not found")).toBeNull();
  });

  it("rejects ambiguous or invalid native window identities", () => {
    expect(readWindows({ structuredContent: { windows: [
      { pid: 42, window_id: 123, title: "Demo", app_name: "Demo App" },
      { pid: 42, window_id: Number.MAX_SAFE_INTEGER + 1, title: "Unsafe integer" },
      { pid: -1, window_id: 10, title: "Invalid process" },
      { pid: "42", window_id: 10, title: "Wrong type" },
    ] } })).toEqual([target]);
    expect(readWindows({ isError: true, structuredContent: { windows: [target] } })).toEqual([]);
  });

  it("does not display stale pixels when exact-window capture was refused", () => {
    const content = [{ type: "image", mimeType: "image/png", data: "aGVsbG8=" }];
    expect(readFrame({ content })).toEqual({ kind: "image", url: "data:image/png;base64,aGVsbG8=" });
    expect(readFrame({ content, structuredContent: {
      screenshot_frame_valid: false,
      screenshot_error: { code: "surface_identity_unproven", reason: "no compositor-attested window geometry" },
    } })).toEqual({ kind: "unavailable", message: "no compositor-attested window geometry" });
    expect(readFrame({ content, isError: true }).kind).toBe("unavailable");
    expect(readFrame({ content: [{ type: "image", mimeType: "image/svg+xml", data: "aGVsbG8=" }] }).kind).toBe("unavailable");
  });

  it("preserves the actual X11 connection error for diagnostics", () => {
    expect(connectionFailure({ isError: true, content: [
      { type: "text", text: "Connection refused (os error 111)" },
    ] })).toBe("cua-driver could not connect to the desktop: Connection refused (os error 111)");
    expect(connectionFailure({ structuredContent: { width: 1024, height: 768 } })).toBeNull();
  });
});

describe("computer-use policy and resource boundaries", () => {
  it("admits only scoped background mutations and never grants display control", () => {
    const files = controlFiles(target);
    const policy = JSON.parse(files.policy);
    const manifest = JSON.parse(files.manifest);
    const admitted = [...policy.allow.tools, ...policy.allow.rules.map((rule: { tool: string }) => rule.tool)];
    for (const tool of ["drag", "mouse_button_down", "mouse_button_up", "mouse_drag", "parallel_mouse_drag", "replay_trajectory", "press_key", "hotkey", "page", "browser_pointer", "clipboard_read", "launch_app", "set_config"]) {
      expect(admitted).not.toContain(tool);
    }
    for (const rule of policy.allow.rules) {
      expect(rule.constraints.pid.allowed).toEqual([42]);
      expect(rule.constraints.window_id.allowed).toEqual([123]);
      expect(rule.constraints.delivery_mode.allowed).toEqual(["background"]);
    }
    expect(policy.allow.rules.find((rule: { tool: string }) => rule.tool === "click").constraints.modifier.allowed).toEqual([[]]);
    expect(manifest.resources.desktop).toEqual({
      applications: [42], windows: [{ pid: 42, window_id: 123 }], display: false,
    });
    expect(manifest.allow.tools.sort()).toEqual(admitted.sort());
  });

  it("requires a new valid window selection instead of inventing an identity", () => {
    expect(() => controlFiles({ ...target, pid: 0 })).toThrow("Choose a current application window");
    expect(() => controlFiles({ ...target, windowId: NaN })).toThrow("Choose a current application window");
  });
});
