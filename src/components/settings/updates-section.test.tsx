/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn().mockResolvedValue("0.24.0") }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@/tauri/commands", () => ({
  hostsList: vi.fn().mockResolvedValue([
    { id: 2, server_id: null, name: "buildbox", ssh_target: "dev@buildbox", created_at: "", updated_at: "", dirty: false },
  ]),
  getLocalDeviceName: vi.fn().mockResolvedValue("workstation"),
}));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn(), failure: vi.fn() } }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => false }));

import { toast } from "@/lib/toast";
import { useUpdateAll } from "@/stores/update-all-store";
import { UpdatesSection } from "./updates-section";

const report = (provider: string, patch: Record<string, unknown> = {}) => ({
  provider,
  installed_version: "1.0.0",
  latest_version: "1.1.0",
  available: true,
  manager: "npm",
  can_update: true,
  message: null,
  ...patch,
});

let installedVersion = "0.23.0";
let runningVersion = "0.23.0";
let statusAvailable = true;
let serviceAlive = true;

beforeEach(() => {
  invoke.mockReset();
  installedVersion = "0.23.0";
  runningVersion = "0.23.0";
  statusAvailable = true;
  serviceAlive = true;
  useUpdateAll.setState({ machines: {}, items: {}, running: false, summary: null });
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "provider_updates_local":
        return [
          report("claude", { manager: "Omarchy · mise" }),
          report("cursor", { can_update: false, manager: "Omarchy system package", message: "Run `omarchy update`" }),
        ];
      case "provider_updates_device":
        return { host_id: 2, helper_version: installedVersion, running_version: statusAvailable && serviceAlive ? runningVersion : null, serve_alive: statusAvailable ? serviceAlive : null, app_version: "0.24.0", providers: [report("codex")], error: null };
      case "device_helper_update":
        installedVersion = "0.24.0";
        statusAvailable = true;
        return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: "An already running headless service keeps its previous version until restarted." };
      case "provider_update_local":
        return report("claude", { installed_version: "1.1.0", available: false });
      case "provider_update_device":
        throw "npm failed (exit 1): EACCES: permission denied";
      default:
        throw new Error(`unexpected ${cmd}`);
    }
  });
});
afterEach(cleanup);

describe("UpdatesSection", () => {
  it("counts every updatable item, names the skipped ones, and reports a partial failure", async () => {
    render(<UpdatesSection />);
    // Claude here, buildbox's outdated helper and its Codex. Cursor is the
    // system package's job, so it is skipped rather than counted.
    const button = await screen.findByRole("button", { name: "Update all (3)" });
    expect(screen.getByText(/Omarchy system package/)).toBeInTheDocument();

    fireEvent.click(button);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Claude/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Codemux, Codex/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Cursor: Run `omarchy update`/)).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Update all" }));
    await waitFor(() => expect(toast.warning).toHaveBeenCalled());
    const [title, options] = vi.mocked(toast.warning).mock.calls[0];
    expect(title).toBe("1 of 3 updates failed");
    expect((options as { description: string }).description).toContain("buildbox · Codex: npm failed (exit 1)");
    expect(invoke).toHaveBeenCalledWith("provider_update_local", { provider: "claude" });
    expect(await screen.findByText(/EACCES: permission denied/)).toBeInTheDocument();
    expect(screen.getByText(/v0.24.0 installed · v0.23.0 running · Restart required/)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("2 updated, 1 failed");
  });

  it("shows a restart requirement after refreshing a device with an older running daemon", async () => {
    installedVersion = "0.24.0";
    runningVersion = "0.23.0";
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "provider_updates_local") return [];
      if (cmd === "provider_updates_device") {
        return { host_id: 2, helper_version: installedVersion, running_version: runningVersion, serve_alive: true, app_version: "0.24.0", providers: [], error: null };
      }
      throw new Error(`unexpected ${cmd}`);
    });

    render(<UpdatesSection />);
    expect(await screen.findByText("v0.24.0 installed · v0.23.0 running · Restart required")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart required" })).toBeDisabled();
  });

  it("refreshes daemon status after updating one device's helper", async () => {
    statusAvailable = false;
    render(<UpdatesSection />);
    const row = (await screen.findByText("v0.23.0 → v0.24.0")).closest("[data-settings-row]");
    if (!(row instanceof HTMLElement)) throw new Error("helper row missing");

    fireEvent.click(within(row).getByRole("button", { name: "Update" }));
    expect(await screen.findByText("v0.24.0 installed · v0.23.0 running · Restart required")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("provider_updates_device", { hostId: 2, force: true });
  });

  it("shows a stopped service after updating its helper", async () => {
    statusAvailable = false;
    serviceAlive = false;
    render(<UpdatesSection />);
    const row = (await screen.findByText("v0.23.0 → v0.24.0")).closest("[data-settings-row]");
    if (!(row instanceof HTMLElement)) throw new Error("helper row missing");

    fireEvent.click(within(row).getByRole("button", { name: "Update" }));
    expect(await screen.findByText("v0.24.0 installed · service stopped")).toBeInTheDocument();
    expect(screen.queryByText(/already running headless service keeps its previous version/)).not.toBeInTheDocument();
  });
});
