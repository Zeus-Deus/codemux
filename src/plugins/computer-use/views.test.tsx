/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PEEK_AREA, type Host } from "@codemux/sdk";
import { registry } from "@/contrib/registry";
import { ComputerUseController, type ComputerUseServices } from "./controller";
import { ComputerChatChip, ComputerIndicator, ComputerSettings, DoctorDiagnostics } from "./views";

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
    fs: { writeDataFile: async (name) => `/demo/${name}`, removeDataFile: async () => {} },
    registerMcpServer: async (options) => async () => { if (options.scope !== "plugin") await exit; },
    mcp: {
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
    render(<ComputerSettings controller={controller} peek={peek} initialCommand="" onCommand={() => {}} onOptIn={() => {}} />);
    expect(screen.getByLabelText("Resolved cua-driver executable")).toBeVisible();
    expect(screen.getByLabelText("Resolved cua-driver executable")).toHaveTextContent("/demo/cua-driver");
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
    render(<ComputerIndicator controller={controller} peek={peek} />);
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
