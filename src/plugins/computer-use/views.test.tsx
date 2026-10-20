/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PEEK_AREA, type Host } from "@codemux/sdk";
import { registry } from "@/contrib/registry";
import { ComputerUseController, type ComputerUseServices } from "./controller";
import { ComputerChatChip, ComputerIndicator, ComputerPreview, ComputerSettings, DoctorDiagnostics } from "./views";

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
  it("shows the resolved executable in the installation diagnostics", async () => {
    const { controller, peek, release } = await activeController();
    render(<ComputerSettings controller={controller} peek={peek} showWorkspace={() => true} initialCommand="" onCommand={() => {}} onOptIn={() => {}} />);
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
    expect(screen.getByText("Driver ready")).toBeVisible();
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
    rerender(<ComputerChatChip controller={controller} workspaceId="demo" />);
    expect(screen.getByRole("button", { name: "View Computer preview" })).toBeVisible();
    unregister();
    release();
    await controller.dispose();
  });
});
