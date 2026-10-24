/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PEEK_AREA, type Host } from "@codemux/sdk";
import { registry } from "@/contrib/registry";
import { ComputerUseController, type ComputerUseServices } from "./controller";
import { ComputerChatChip, ComputerIndicator, ComputerPreview, ComputerSettings, DoctorDiagnostics, computerChipVisible, setupStatus } from "./views";

vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() } }));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));

afterEach(cleanup);

const healthy = { ok: true, probes: [{ label: "X11 connection", status: "ok" as const, message: "Connected to the desktop session." }] };

async function activeController() {
  let release = () => {};
  const exit = new Promise<void>((resolve) => { release = resolve; });
  const services: ComputerUseServices = {
    process: {
      which: async () => "/demo/cua-driver",
      run: async ({ args }) => ({ code: 0, stdout: args?.[0] === "--version" ? "cua-driver 0.21.0" : JSON.stringify(healthy), stderr: "", timedOut: false, truncated: false, pipesHeldOpen: false, cancelled: false }),
    },
    os: { info: async () => ({ platform: "linux", arch: "x86_64", homeDir: "/home/demo", configDir: null, dataDir: null, localDataDir: null }) },
    tools: { status: async () => ({ id: "cua-driver", version: "0.21.0", supported: true, unsupportedReason: null, executable: null }), install: async () => "/demo/managed" },
    fs: { dataDir: async () => "/demo", listDir: async () => [], writeDataFile: async (name) => `/demo/${name}`, removeDataFile: async () => {} },
    registerMcpServer: async (options) => async () => { if (options.scope !== "plugin") await exit; },
    mcp: {
      listTools: async (id) => (id.startsWith("cua-discovery-") ? ["list_windows", "get_screen_size"] : id.startsWith("cua-view-") ? ["get_window_state"] : ["get_window_state", "click", "type_text", "scroll"]).map((name) => ({ name, prefixedName: name, description: null, inputSchema: {}, serverId: id })),
      callTool: async (_id, tool) => ({ content: [], ...(tool === "list_windows" ? { structuredContent: { windows: [{ pid: 42, window_id: 123, title: "Counter", app_name: "Counter" }] } } : {}) }),
      getStatus: async (id) => ({ id, name: "Computer use", status: { kind: "running", toolCount: 1 }, toolsCount: 1, errorMessage: null, stderrTail: null, startedAtMs: 0 }),
      onStatus: () => () => {}, onActivity: () => () => {},
    },
  };
  const controller = new ComputerUseController(services);
  await controller.setWorkspace({ id: "demo", title: "Demo", local: true }, true);
  await controller.detect();
  await controller.discover();
  await controller.enable(controller.getState().windows[0]);
  const peek: Pick<Host["peek"], "open"> = { open: () => true };
  return { controller, peek, release };
}

describe("computer-use status controls", () => {
  it("shows the resolved executable in the advanced diagnostics", async () => {
    const { controller, peek, release } = await activeController();
    render(<ComputerSettings controller={controller} peek={peek} showWorkspace={() => true} initialCommand="" onCommand={() => {}} onOptIn={() => {}} />);
    await userEvent.setup().click(screen.getByText(/^Advanced/));
    expect(screen.getByLabelText("Resolved cua-driver executable")).toBeVisible();
    expect(screen.getByLabelText("Resolved cua-driver executable")).toHaveTextContent("/demo/cua-driver");
    release();
    await controller.dispose();
  });

  it.each(["settings", "title bar"])("returns to the workspace before opening Live view from %s", async (surface) => {
    const { controller, release } = await activeController();
    const order: string[] = [];
    let available = true;
    const showWorkspace = () => { order.push("workspace"); return available; };
    const peek = { open: vi.fn(() => { order.push("peek"); return true; }) };
    render(surface === "settings"
      ? <ComputerSettings controller={controller} peek={peek} showWorkspace={showWorkspace} initialCommand="" onCommand={() => {}} onOptIn={() => {}} />
      : <ComputerIndicator controller={controller} peek={peek} showWorkspace={showWorkspace} />);
    const user = userEvent.setup();
    const button = screen.getByRole("button", { name: surface === "settings" ? "Live view" : /^View computer use/ });
    await user.click(button);
    expect(order).toEqual(["workspace", "peek"]);
    expect(peek.open).toHaveBeenCalledWith("computer-use:live", { workspaceId: "demo" });
    available = false;
    order.length = 0;
    await user.click(button);
    expect(order).toEqual(["workspace"]);
    expect(peek.open).toHaveBeenCalledTimes(1);
    release();
    await controller.dispose();
  });

  it("keeps healthy diagnostics collapsed and warnings immediately visible", () => {
    const { rerender, container } = render(<DoctorDiagnostics report={healthy} />);
    expect(screen.getByText("All diagnostics passed")).toBeVisible();
    expect(container.querySelector("details")).not.toHaveAttribute("open");
    expect(screen.getByText("Connected to the desktop session.")).not.toBeVisible();
    rerender(<DoctorDiagnostics report={{ ok: true, probes: [{ label: "X11 connection", status: "warn", message: "Connection refused (os error 111)", detail: "DISPLAY=:0" }] }} />);
    expect(screen.getByLabelText("Warning")).toBeVisible();
    expect(screen.getByText("Connection refused (os error 111)")).toBeVisible();
    expect(screen.getByText("DISPLAY=:0")).toBeVisible();
    expect(container.querySelector("details")).toBeNull();
  });

  it("lets the keyboard reach Stop and waits for confirmed exit", async () => {
    const { controller, peek, release } = await activeController();
    const user = userEvent.setup();
    render(<ComputerIndicator controller={controller} peek={peek} showWorkspace={() => true} />);
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Stop computer use" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(screen.getByText("Stopping…")).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop computer use" })).toBeDisabled();
    await act(async () => { release(); await controller.stop(); });
    expect(screen.queryByRole("button", { name: "Stop computer use" })).toBeNull();
    await controller.dispose();
  });

  it.each(["starting", "error", "stopping"] as const)("keeps live-view Stop available in %s unless shutdown is pending", async (phase) => {
    const { controller, release } = await activeController();
    const state = { ...controller.getState(), phase };
    vi.spyOn(controller, "getState").mockReturnValue(state);
    render(<ComputerPreview controller={controller} workspaceId="demo" size={{ width: 400, height: 300 }} close={() => {}} onStatusChange={() => {}} />);
    const button = screen.getByRole("button", { name: "Stop computer use in live view" });
    if (phase === "stopping") expect(button).toBeDisabled();
    else expect(button).toBeEnabled();
    release();
    await controller.dispose();
  });

  it("announces only the state text instead of wrapping the Stop buttons in a live region", async () => {
    const { controller, peek, release } = await activeController();
    const { container } = render(<ComputerIndicator controller={controller} peek={peek} showWorkspace={() => true} />);
    expect(container.querySelector('[data-computer-use="indicator"]')).not.toHaveAttribute("role");
    const live = container.querySelector('[aria-live="polite"]');
    expect(live).toHaveTextContent("Computer · Counter");
    expect(live?.querySelector("button")).toBeNull();
    expect(screen.getByRole("button", { name: "Stop computer use" }).closest('[aria-live]')).toBeNull();
    release();
    await controller.dispose();
  });

  it("shows the live-view chip only in the opted-in workspace", async () => {
    const { controller, release } = await activeController();
    const unregister = registry.register({ id: "computer-use:live", source: "test", area: PEEK_AREA, render: () => null });
    const { rerender } = render(<ComputerChatChip controller={controller} workspaceId="other" />);
    expect(screen.queryByRole("button", { name: "View Computer preview" })).toBeNull();
    // The status area's visibility data must agree, or Live titles nothing.
    expect(computerChipVisible(controller.getState(), "other")).toBe(false);
    rerender(<ComputerChatChip controller={controller} workspaceId="demo" />);
    expect(screen.getByRole("button", { name: "View Computer preview" })).toBeVisible();
    expect(computerChipVisible(controller.getState(), "demo")).toBe(true);
    unregister();
    release();
    await controller.dispose();
  });
});

function setupServices(overrides: { which?: (command: string) => Promise<string | null>; version?: string; doctor?: object; supported?: boolean; install?: ComputerUseServices["tools"]["install"] } = {}): ComputerUseServices {
  return {
    process: {
      which: overrides.which ?? (async () => null),
      run: async ({ args }) => ({ code: 0, stdout: args?.[0] === "--version" ? `cua-driver ${overrides.version ?? "0.21.0"}` : JSON.stringify(overrides.doctor ?? healthy), stderr: "", timedOut: false, truncated: false, pipesHeldOpen: false, cancelled: false }),
    },
    os: { info: async () => ({ platform: "linux", arch: "x86_64", homeDir: "/home/demo", configDir: null, dataDir: null, localDataDir: null }) },
    tools: {
      status: async () => ({ id: "cua-driver", version: "0.21.0", supported: overrides.supported ?? true,
        unsupportedReason: overrides.supported === false ? "Codemux can’t install the driver on macOS." : null, executable: null }),
      install: overrides.install ?? (async () => "/demo/managed"),
    },
    fs: { dataDir: async () => "/demo", listDir: async () => [], writeDataFile: async (name) => `/demo/${name}`, removeDataFile: async () => {} },
    registerMcpServer: async () => async () => {},
    mcp: { listTools: async () => [], callTool: async () => ({ content: [] }), getStatus: async () => null, onStatus: () => () => {}, onActivity: () => () => {} },
  };
}

function renderSettings(controller: ComputerUseController, props: Partial<{ initialCommand: string; onCommand: (command: string) => void }> = {}) {
  return render(<ComputerSettings controller={controller} peek={{ open: () => true }} showWorkspace={() => true}
    initialCommand={props.initialCommand ?? ""} onCommand={props.onCommand ?? (() => {})} onOptIn={() => {}} />);
}

describe("computer-use driver setup page", () => {
  it("checks automatically on open and offers one install action when the driver is missing", async () => {
    let finish = () => {};
    const install = vi.fn((_tool: string, onProgress?: (progress: { stage: "downloading"; received: number; total: number }) => void) => new Promise<string>((resolve) => {
      onProgress?.({ stage: "downloading", received: 5_000_000, total: 20_000_000 });
      finish = () => resolve("/demo/managed");
    }));
    const services = setupServices({ install });
    const controller = new ComputerUseController(services);
    await controller.setWorkspace({ id: "demo", title: "Demo", local: true }, false);
    renderSettings(controller);
    expect(await screen.findByText("Driver not installed")).toBeVisible();
    expect(screen.getByRole("switch", { name: "Allow computer use in this workspace" })).toBeDisabled();
    expect(screen.getByText("Finish driver setup to choose a window.")).toBeVisible();
    // The custom path is optional and tucked away.
    expect(screen.getByLabelText("cua-driver executable")).not.toBeVisible();
    await userEvent.setup().click(screen.getByRole("button", { name: "Install driver" }));
    expect(install).toHaveBeenCalledOnce();
    expect(await screen.findByText("Downloading… 5.0 of 20.0 MB")).toBeVisible();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "25");
    services.process.which = async (command) => command === "/demo/managed" ? "/demo/managed" : null;
    services.tools.status = async () => ({ id: "cua-driver", version: "0.21.0", supported: true, unsupportedReason: null, executable: "/demo/managed" });
    await act(async () => { finish(); await vi.waitFor(() => expect(controller.getState().driver.status).toBe("ready")); });
    expect(screen.getByText("Driver ready")).toBeVisible();
    expect(screen.getByText("Installed by Codemux")).toBeVisible();
    expect(screen.getByRole("switch", { name: "Allow computer use in this workspace" })).toBeEnabled();
    expect(controller.getState().optedIn).toBe(false);
    await controller.dispose();
  });

  it("shows a retry after a failed install", async () => {
    const controller = new ComputerUseController(setupServices({ install: async () => { throw new Error("The download failed: HTTP 503. Check your connection and try again."); } }));
    renderSettings(controller);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Install driver" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("HTTP 503");
    expect(screen.getByRole("button", { name: "Retry installation" })).toBeEnabled();
    await controller.dispose();
  });

  it("keeps an incompatible install in place and offers a compatible one", async () => {
    const controller = new ComputerUseController(setupServices({ which: async (command) => command === "cua-driver" ? "/usr/bin/cua-driver" : null, version: "0.20.0" }));
    renderSettings(controller);
    expect(await screen.findByText("Compatible driver needed")).toBeVisible();
    expect(screen.getByText(/Found cua-driver 0\.20\.0/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Install compatible driver" })).toBeVisible();
    expect(screen.getByText("Your existing installation stays in place.")).toBeVisible();
    await controller.dispose();
  });

  it("gives an honest manual fallback where Codemux can’t install", async () => {
    const controller = new ComputerUseController(setupServices({ supported: false }));
    renderSettings(controller);
    expect(await screen.findByText("Driver not installed")).toBeVisible();
    expect(screen.queryByRole("button", { name: /Install/ })).toBeNull();
    expect(screen.getByRole("link", { name: "Open installation guide" })).toHaveAttribute("href", "https://cua.ai/docs/cua-driver/quickstart");
    expect(screen.getByText(/can’t install the driver on macOS/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
    expect(setupStatus(controller.getState())).toEqual({ label: "Setup needed · driver not installed", tone: "attention" });
    await controller.dispose();
  });

  it("explains operating-system setup and re-checks", async () => {
    const doctor = { ok: false, probes: [{ label: "screen recording", status: "error", message: "Screen Recording permission is not granted." }] };
    const services = setupServices({ which: async (command) => command === "cua-driver" ? "/usr/bin/cua-driver" : null, doctor });
    const controller = new ComputerUseController(services);
    renderSettings(controller);
    expect(await screen.findByText("Driver setup needed")).toBeVisible();
    expect(screen.getByText("Screen Recording permission is not granted.")).toBeVisible();
    expect(screen.getByText(/operating system’s settings/)).toBeVisible();
    const run = vi.spyOn(services.process, "run");
    await userEvent.setup().click(screen.getByRole("button", { name: "Check again" }));
    await vi.waitFor(() => expect(run).toHaveBeenCalled());
    await controller.dispose();
  });

  it("shows doctor warnings on a ready driver without opening Advanced", async () => {
    const doctor = { ok: true, probes: [
      { label: "X11 connection", status: "ok", message: "Connected to the desktop session." },
      { label: "Accessibility bus", status: "warn", message: "AT-SPI bus is not running." },
    ] };
    const controller = new ComputerUseController(setupServices({ which: async (command) => command === "cua-driver" ? "/usr/bin/cua-driver" : null, doctor }));
    renderSettings(controller);
    expect(await screen.findByText("Driver ready")).toBeVisible();
    expect(screen.getByText("AT-SPI bus is not running.")).toBeVisible();
    expect(screen.getByLabelText("Warning")).toBeVisible();
    // Listed once, outside the collapsed Advanced section.
    expect(screen.getAllByLabelText("Driver diagnostics")).toHaveLength(1);
    expect(screen.getByLabelText("cua-driver executable")).not.toBeVisible();
    await controller.dispose();
  });

  it("keeps an all-clear report collapsed under Advanced on a ready driver", async () => {
    const controller = new ComputerUseController(setupServices({ which: async (command) => command === "cua-driver" ? "/usr/bin/cua-driver" : null }));
    renderSettings(controller);
    expect(await screen.findByText("Driver ready")).toBeVisible();
    expect(screen.getByText("All diagnostics passed")).not.toBeVisible();
    await userEvent.setup().click(screen.getByText(/^Advanced/));
    expect(screen.getByText("All diagnostics passed")).toBeVisible();
    await controller.dispose();
  });

  it("checks a custom path under Advanced and restores automatic detection", async () => {
    const onCommand = vi.fn();
    const services = setupServices({ which: async (command) => command === "/opt/cua/cua-driver" || command === "cua-driver" ? command === "cua-driver" ? "/usr/bin/cua-driver" : command : null });
    const controller = new ComputerUseController(services);
    renderSettings(controller, { onCommand });
    expect(await screen.findByText("Driver ready")).toBeVisible();
    const user = userEvent.setup();
    await user.click(screen.getByText(/^Advanced/));
    await user.type(screen.getByLabelText("cua-driver executable"), "/opt/cua/cua-driver");
    await user.click(screen.getByRole("button", { name: "Use this path" }));
    expect(onCommand).toHaveBeenLastCalledWith("/opt/cua/cua-driver");
    await vi.waitFor(() => expect(controller.getState().driver.source).toBe("custom"));
    expect(screen.getByText("Custom path")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Use automatic detection" }));
    expect(onCommand).toHaveBeenLastCalledWith("");
    await vi.waitFor(() => expect(controller.getState().driver.source).toBe("system"));
    expect(screen.getByLabelText("cua-driver executable")).toHaveValue("");
    await controller.dispose();
  });

  it("summarises readiness for the Extensions row separately from on/off", () => {
    const base = { phase: "idle" as const, driver: { status: "ready" as const, message: null, source: "system" as const } };
    expect(setupStatus(base)).toEqual({ label: "Ready to use", tone: "ready" });
    expect(setupStatus({ ...base, phase: "installing" })).toEqual({ label: "Installing driver…", tone: "neutral" });
    expect(setupStatus({ ...base, driver: { ...base.driver, status: "incompatible" } }).tone).toBe("attention");
    expect(setupStatus({ ...base, driver: { ...base.driver, status: "unchecked" } }).tone).toBe("neutral");
  });
});
