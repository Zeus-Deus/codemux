/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { HostStatusView, HostView } from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    hostsList: vi.fn(),
    hostsStatusList: vi.fn(),
    hostsAdd: vi.fn(),
    hostsUpdate: vi.fn(),
    hostsDelete: vi.fn(),
    hostsTestConnection: vi.fn(),
    hostsBootstrapInstall: vi.fn(),
    hostsReinstallRemote: vi.fn(),
    hostsSshConfigHosts: vi.fn(),
    getLocalDeviceName: vi.fn(),
  };
});

// The status store subscribes to the poller's event; there is no Tauri
// runtime in jsdom.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
    dismiss: vi.fn(),
    custom: vi.fn(),
  },
}));

import { HostsSection, defaultDeviceName } from "./hosts-section";
import {
  getLocalDeviceName,
  hostsAdd,
  hostsBootstrapInstall,
  hostsDelete,
  hostsList,
  hostsReinstallRemote,
  hostsSshConfigHosts,
  hostsStatusList,
  hostsTestConnection,
  hostsUpdate,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { __resetHostsStoreForTests } from "@/stores/hosts-store";
import { __resetHostStatusStoreForTests } from "@/stores/host-status-store";
import { __resetLocalDeviceNameForTests } from "@/stores/local-device-store";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";

const mocked = {
  hostsList: vi.mocked(hostsList),
  hostsStatusList: vi.mocked(hostsStatusList),
  hostsAdd: vi.mocked(hostsAdd),
  hostsUpdate: vi.mocked(hostsUpdate),
  hostsDelete: vi.mocked(hostsDelete),
  hostsTestConnection: vi.mocked(hostsTestConnection),
  hostsBootstrapInstall: vi.mocked(hostsBootstrapInstall),
  hostsReinstallRemote: vi.mocked(hostsReinstallRemote),
  hostsSshConfigHosts: vi.mocked(hostsSshConfigHosts),
  getLocalDeviceName: vi.mocked(getLocalDeviceName),
};

function makeHost(overrides: Partial<HostView> = {}): HostView {
  return {
    id: 1,
    server_id: "srv-zeus",
    name: "zeus",
    ssh_target: "deus@zeus",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
    ...overrides,
  };
}

function makeStatus(overrides: Partial<HostStatusView> = {}): HostStatusView {
  return {
    host_id: 1,
    probed: true,
    reachable: true,
    last_seen_at: "2026-01-01T00:00:00Z",
    last_error: null,
    disk_bytes: null,
    remote_control_serving: false,
    ...overrides,
  };
}

const READY = { ok: true, message: "Connected.", needs_install: false, uname: null };
const NEEDS_INSTALL = {
  ok: false,
  message: "Reachable, but codemux-remote isn't installed yet (Linux x86_64)",
  needs_install: true,
  uname: "Linux x86_64",
};

/** Hosts the backend holds; hostsAdd/hostsDelete keep it in sync so the
 *  store refresh after each mutation reads the change back. */
let backendHosts: HostView[] = [];

const initialDialogState = useAddDeviceDialogStore.getState();

beforeEach(() => {
  // Drops queued once-values a failed test may have left behind.
  for (const fn of Object.values(mocked)) fn.mockReset();
  backendHosts = [];
  mocked.hostsList.mockImplementation(() => Promise.resolve([...backendHosts]));
  mocked.hostsStatusList.mockResolvedValue([]);
  mocked.hostsSshConfigHosts.mockResolvedValue([]);
  mocked.getLocalDeviceName.mockResolvedValue("ai-node");
  mocked.hostsAdd.mockImplementation((name, sshTarget) => {
    const host = makeHost({ id: 7, server_id: null, name, ssh_target: sshTarget });
    backendHosts.push(host);
    return Promise.resolve(host);
  });
  mocked.hostsDelete.mockImplementation((id) => {
    backendHosts = backendHosts.filter((h) => h.id !== id);
    return Promise.resolve();
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  __resetHostsStoreForTests();
  __resetHostStatusStoreForTests();
  __resetLocalDeviceNameForTests();
  useAddDeviceDialogStore.setState(initialDialogState, true);
});

function stepState(label: string): string | null {
  return screen.getByText(label).closest("li")?.getAttribute("data-state") ?? null;
}

async function openAddDialog() {
  render(<HostsSection />);
  fireEvent.click(await screen.findByRole("button", { name: /add device/i }));
  return screen.findByRole("dialog", { name: "Add device" });
}

describe("HostsSection — list", () => {
  it("shows this machine and each device with its live status", async () => {
    backendHosts = [
      makeHost(),
      makeHost({ id: 2, name: "nas", ssh_target: "deus@nas" }),
      makeHost({ id: 3, name: "pandora", ssh_target: "deus@pandora" }),
      makeHost({ id: 4, name: "fresh", ssh_target: "me@fresh" }),
    ];
    mocked.hostsStatusList.mockResolvedValue([
      makeStatus(),
      makeStatus({ host_id: 2, last_error: "codemux-remote is not installed" }),
      makeStatus({ host_id: 3, reachable: false, last_error: "timed out" }),
      makeStatus({ host_id: 4, probed: false }),
    ]);

    render(<HostsSection />);

    expect(await screen.findByText("ai-node")).toBeInTheDocument();
    expect(screen.getByText("This computer · always available")).toBeInTheDocument();
    expect(await screen.findByText("SSH deus@zeus")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("Connected")).toBeInTheDocument());
    expect(screen.getByText("Needs setup")).toBeInTheDocument();
    expect(screen.getByText("Offline")).toBeInTheDocument();
    expect(screen.getByText("Checking…")).toBeInTheDocument();
  });

  it("shows the empty state when no devices are configured", async () => {
    render(<HostsSection />);
    expect(await screen.findByText(/No devices yet/)).toBeInTheDocument();
  });

  it("opens from the shared dialog store", async () => {
    render(<HostsSection />);
    act(() => useAddDeviceDialogStore.getState().setOpen(true));
    expect(await screen.findByRole("dialog", { name: "Add device" })).toBeInTheDocument();
  });
});

describe("HostsSection — Add device", () => {
  it("saves, connects, installs the helper and closes when ready", async () => {
    mocked.hostsTestConnection
      .mockResolvedValueOnce(NEEDS_INSTALL)
      .mockResolvedValueOnce(READY);
    mocked.hostsBootstrapInstall.mockResolvedValue({ ok: true, message: "installed" });

    const dialog = await openAddDialog();
    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "pi@raspberrypi.local" },
    });
    // The name defaults to the first DNS label of the host.
    expect(within(dialog).getByLabelText(/^Name/)).toHaveAttribute(
      "placeholder",
      "raspberrypi",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));

    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith("raspberrypi is ready");
    });
    expect(mocked.hostsAdd).toHaveBeenCalledWith("raspberrypi", "pi@raspberrypi.local");
    expect(mocked.hostsBootstrapInstall).toHaveBeenCalledWith(7, "Linux x86_64");
    expect(mocked.hostsTestConnection).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(useAddDeviceDialogStore.getState().open).toBe(false));
    // The store refresh brings the new device into the list.
    expect(await screen.findByText("SSH pi@raspberrypi.local")).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("skips the install when the helper is already there", async () => {
    mocked.hostsTestConnection.mockResolvedValue(READY);
    // Keep the dialog open after success so the step list can be read.
    const setOpen = vi.fn();
    useAddDeviceDialogStore.setState({ setOpen });
    render(<HostsSection />);
    act(() => useAddDeviceDialogStore.setState({ open: true }));
    const dialog = await screen.findByRole("dialog", { name: "Add device" });

    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "deus@zeus" },
    });
    fireEvent.change(within(dialog).getByLabelText(/^Name/), {
      target: { value: "Zeus box" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Zeus box is ready"));
    expect(mocked.hostsAdd).toHaveBeenCalledWith("Zeus box", "deus@zeus");
    expect(mocked.hostsBootstrapInstall).not.toHaveBeenCalled();
    expect(mocked.hostsReinstallRemote).not.toHaveBeenCalled();
    expect(stepState("Install Codemux helper")).toBe("skipped");
    expect(stepState("Ready")).toBe("done");
    expect(setOpen).toHaveBeenCalledWith(false);
  });

  it("stops on an SSH failure, keeps the device and retries without adding it again", async () => {
    mocked.hostsTestConnection.mockResolvedValueOnce({
      ok: false,
      message: "Permission denied (publickey).",
      needs_install: false,
      uname: null,
    });

    const dialog = await openAddDialog();
    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "deus@zeus" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));

    expect(await within(dialog).findByText("Permission denied (publickey).")).toBeInTheDocument();
    expect(within(dialog).getByText("ssh deus@zeus")).toBeInTheDocument();
    expect(stepState("Save device")).toBe("done");
    expect(stepState("Connect over SSH")).toBe("failed");
    expect(stepState("Install Codemux helper")).toBe("pending");
    expect(mocked.hostsBootstrapInstall).not.toHaveBeenCalled();
    expect(useAddDeviceDialogStore.getState().open).toBe(true);

    mocked.hostsTestConnection.mockResolvedValue(READY);
    fireEvent.click(within(dialog).getByRole("button", { name: "Retry" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("zeus is ready"));
    expect(mocked.hostsAdd).toHaveBeenCalledTimes(1);
    expect(mocked.hostsUpdate).not.toHaveBeenCalled();
  });

  it("continues setup on a device already saved for the same target", async () => {
    // Saved by an earlier attempt the dialog was closed on.
    backendHosts = [makeHost({ name: "homelab" })];
    mocked.hostsTestConnection.mockResolvedValue(READY);
    mocked.hostsUpdate.mockImplementation((id, name, sshTarget) =>
      Promise.resolve(makeHost({ id, name, ssh_target: sshTarget })),
    );

    const dialog = await openAddDialog();
    await screen.findByText("SSH deus@zeus");
    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "Deus@Zeus" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));

    // No name typed: the device keeps its own.
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("homelab is ready"));
    expect(mocked.hostsAdd).not.toHaveBeenCalled();
    expect(mocked.hostsUpdate).not.toHaveBeenCalled();
    expect(mocked.hostsTestConnection).toHaveBeenCalledWith(1);
  });

  it("renames the matching device when a name is typed", async () => {
    backendHosts = [makeHost()];
    mocked.hostsTestConnection.mockResolvedValue(READY);
    mocked.hostsUpdate.mockImplementation((id, name, sshTarget) =>
      Promise.resolve(makeHost({ id, name, ssh_target: sshTarget })),
    );

    const dialog = await openAddDialog();
    await screen.findByText("SSH deus@zeus");
    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "deus@zeus" },
    });
    fireEvent.change(within(dialog).getByLabelText(/^Name/), {
      target: { value: "Zeus box" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Zeus box is ready"));
    expect(mocked.hostsAdd).not.toHaveBeenCalled();
    expect(mocked.hostsUpdate).toHaveBeenCalledWith(1, "Zeus box", "deus@zeus");
  });

  it("a run left behind by a closed dialog does not close a newly opened one", async () => {
    let finishCheck: (r: typeof READY) => void = () => {};
    mocked.hostsTestConnection
      .mockResolvedValueOnce(READY)
      .mockImplementationOnce(
        () => new Promise((resolve) => (finishCheck = resolve)),
      );

    const dialog = await openAddDialog();
    fireEvent.change(within(dialog).getByLabelText("SSH host"), {
      target: { value: "deus@zeus" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Connect" }));
    // The run is waiting on its final check.
    await waitFor(() => expect(mocked.hostsTestConnection).toHaveBeenCalledTimes(2));

    act(() => useAddDeviceDialogStore.getState().setOpen(false));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Add device" })).toBeNull(),
    );
    act(() => useAddDeviceDialogStore.getState().setOpen(true));
    const reopened = await screen.findByRole("dialog", { name: "Add device" });
    fireEvent.change(within(reopened).getByLabelText("SSH host"), {
      target: { value: "pi@raspberrypi.local" },
    });

    await act(async () => finishCheck(READY));

    // The device really is ready, so the toast still fires.
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("zeus is ready"));
    expect(useAddDeviceDialogStore.getState().open).toBe(true);
    expect(
      within(screen.getByRole("dialog", { name: "Add device" })).getByLabelText("SSH host"),
    ).toHaveValue("pi@raspberrypi.local");
  });

  it("offers ssh config hosts that are not added yet", async () => {
    backendHosts = [makeHost()];
    mocked.hostsSshConfigHosts.mockResolvedValue(["homelab", "deus@zeus", "pi@raspberrypi.local"]);

    const dialog = await openAddDialog();
    const chip = await within(dialog).findByRole("button", { name: "homelab" });
    expect(within(dialog).queryByRole("button", { name: "deus@zeus" })).toBeNull();

    fireEvent.click(chip);
    expect(within(dialog).getByLabelText("SSH host")).toHaveValue("homelab");
    expect(within(dialog).getByLabelText("SSH host")).toHaveFocus();
  });
});

describe("HostsSection — row actions", () => {
  async function openMenu(user: ReturnType<typeof userEvent.setup>, name = "zeus") {
    await user.click(
      await screen.findByRole("button", { name: `More actions for ${name}` }),
    );
  }

  it("removes a device after confirming in a dialog", async () => {
    backendHosts = [makeHost()];
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Remove…" }));

    const confirm = await screen.findByRole("alertdialog");
    expect(within(confirm).getByText("Remove zeus?")).toBeInTheDocument();
    expect(mocked.hostsDelete).not.toHaveBeenCalled();
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));

    await waitFor(() => expect(mocked.hostsDelete).toHaveBeenCalledWith(1));
    expect(await screen.findByText(/No devices yet/)).toBeInTheDocument();
  });

  it("renames a device and keeps its SSH target", async () => {
    backendHosts = [makeHost()];
    mocked.hostsUpdate.mockImplementation((id, name, sshTarget) => {
      backendHosts = [makeHost({ id, name, ssh_target: sshTarget })];
      return Promise.resolve(backendHosts[0]);
    });
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Rename…" }));
    const input = await screen.findByRole("textbox", { name: "Device name" });
    await user.clear(input);
    await user.type(input, "zeus-desktop{Enter}");

    await waitFor(() =>
      expect(mocked.hostsUpdate).toHaveBeenCalledWith(1, "zeus-desktop", "deus@zeus"),
    );
    expect(await screen.findByText("zeus-desktop")).toBeInTheDocument();
  });

  it("probes first and installs over a missing helper, without a prior test", async () => {
    backendHosts = [makeHost()];
    mocked.hostsTestConnection
      .mockResolvedValueOnce(NEEDS_INSTALL)
      .mockResolvedValueOnce(READY);
    mocked.hostsBootstrapInstall.mockResolvedValue({ ok: true, message: "installed" });
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Set up again" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("zeus is ready"));
    expect(mocked.hostsBootstrapInstall).toHaveBeenCalledWith(1, "Linux x86_64");
    // The forced reinstall restarts the device's terminals.
    expect(mocked.hostsReinstallRemote).not.toHaveBeenCalled();
    expect(mocked.hostsTestConnection).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("upgrades an out-of-date helper in place", async () => {
    backendHosts = [makeHost()];
    mocked.hostsTestConnection
      .mockResolvedValueOnce({
        ok: true,
        message: "codemux-remote v0.1.0 on the device, v0.2.0 bundled",
        needs_install: true,
        uname: "Linux aarch64",
      })
      .mockResolvedValueOnce(READY);
    mocked.hostsBootstrapInstall.mockResolvedValue({ ok: true, message: "installed" });
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Set up again" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("zeus is ready"));
    expect(mocked.hostsBootstrapInstall).toHaveBeenCalledWith(1, "Linux aarch64");
    expect(mocked.hostsReinstallRemote).not.toHaveBeenCalled();
  });

  it("falls back to a reinstall when the probe has no uname to go on", async () => {
    backendHosts = [makeHost()];
    mocked.hostsTestConnection.mockResolvedValueOnce({ ...NEEDS_INSTALL, uname: null });
    mocked.hostsReinstallRemote.mockResolvedValue({ ok: false, message: "upload: connection refused" });
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Set up again" }));

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Couldn't set up zeus",
        expect.objectContaining({ description: "upload: connection refused" }),
      ),
    );
    expect(mocked.hostsReinstallRemote).toHaveBeenCalledWith(1);
    expect(mocked.hostsBootstrapInstall).not.toHaveBeenCalled();
    expect(mocked.hostsTestConnection).toHaveBeenCalledTimes(1);
  });

  it("stops without installing when the device can't be reached", async () => {
    backendHosts = [makeHost()];
    mocked.hostsTestConnection.mockResolvedValueOnce({
      ok: false,
      message: "ssh: connect to host zeus port 22: Connection timed out",
      needs_install: false,
      uname: null,
    });
    const user = userEvent.setup();
    render(<HostsSection />);

    await openMenu(user);
    await user.click(await screen.findByRole("menuitem", { name: "Set up again" }));

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Couldn't reach zeus",
        expect.objectContaining({
          description: "ssh: connect to host zeus port 22: Connection timed out",
        }),
      ),
    );
    expect(mocked.hostsBootstrapInstall).not.toHaveBeenCalled();
    expect(mocked.hostsReinstallRemote).not.toHaveBeenCalled();
  });
});

describe("defaultDeviceName", () => {
  it.each([
    ["deus@zeus.local", "zeus"],
    ["homelab", "homelab"],
    ["pi@raspberrypi.local", "raspberrypi"],
    ["ubuntu@5.5.5.5", "5.5.5.5"],
  ])("%s → %s", (target, name) => {
    expect(defaultDeviceName(target)).toBe(name);
  });
});
