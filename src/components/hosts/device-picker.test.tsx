/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { host, status } from "@/components/devices/host-fixtures.test-utils";
import type { HostStatusView, HostView } from "@/tauri/commands";

let hosts: HostView[] = [];
let statuses: Record<number, HostStatusView> = {};
let localName: string | null = "ai-node";
const setShowSettingsMock = vi.fn();

vi.mock("@/stores/hosts-store", () => ({ useHosts: () => hosts }));
vi.mock("@/stores/host-status-store", () => ({
  useHostStatuses: () => statuses,
}));
vi.mock("@/stores/local-device-store", () => ({
  useLocalDeviceName: () => localName,
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ setShowSettings: setShowSettingsMock }),
  ),
}));

import { DevicePicker } from "./device-picker";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";

afterEach(() => cleanup());

beforeEach(() => {
  hosts = [];
  statuses = {};
  localName = "ai-node";
  setShowSettingsMock.mockClear();
  useAddDeviceDialogStore.setState({ open: false });
});

function trigger() {
  return screen.getByRole("button", { name: /^Device:/ });
}

describe("DevicePicker", () => {
  it("labels the trigger with this machine's hostname when local", () => {
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    expect(trigger()).toHaveAccessibleName("Device: ai-node");
  });

  it("falls back to 'This device' until the hostname loads", () => {
    localName = null;
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    expect(trigger()).toHaveAccessibleName("Device: This device");
  });

  it("shows the device name when a device is selected", () => {
    hosts = [host(7, "homelab"), host(8, "vps-fra")];
    render(<DevicePicker hostId={7} onSelectHostId={() => {}} />);
    expect(trigger()).toHaveAccessibleName("Device: homelab");
  });

  it("shows a neutral label, with no row checked, for a device the list doesn't know", async () => {
    // The send still goes to that device (e.g. the list failed to load),
    // so the picker must not claim this machine.
    const user = userEvent.setup();
    hosts = [host(7, "homelab")];
    render(<DevicePicker hostId={999} onSelectHostId={() => {}} />);
    expect(trigger()).toHaveAccessibleName("Device: Device unavailable");
    expect(trigger()).toHaveAttribute("title", "Runs on a device that isn't available");

    await user.click(trigger());
    await screen.findByText("this device");
    expect(screen.queryByLabelText("Selected")).toBeNull();
  });

  it("checks this device's row when local", async () => {
    const user = userEvent.setup();
    hosts = [host(7, "homelab")];
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await user.click(trigger());
    const local = (await screen.findByText("this device")).closest("[cmdk-item]");
    expect(screen.getAllByLabelText("Selected")).toHaveLength(1);
    expect(local).toContainElement(screen.getByLabelText("Selected"));
    expect(local).toHaveTextContent("ai-node");
  });

  it("lists this device and every device with its live status", async () => {
    const user = userEvent.setup();
    hosts = [host(1, "homelab"), host(2, "zeus"), host(3, "nas"), host(4, "pi")];
    statuses = {
      1: status(1, { reachable: true }),
      2: status(2, { reachable: true, last_error: "codemux-remote is not installed" }),
      3: status(3),
      4: status(4, { probed: false }),
    };
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await user.click(trigger());

    expect(await screen.findByText("this device")).toBeInTheDocument();
    const row = (id: number) =>
      document.querySelector<HTMLElement>(`[data-host-id="${id}"]`)!;
    // One line each: name and status; the SSH address is the tooltip.
    expect(row(1)).toHaveTextContent(/homelab.*online/);
    expect(row(1)).toHaveAttribute("title", "SSH deus@homelab · Online");
    expect(row(2)).toHaveTextContent(/zeus.*needs setup/);
    expect(row(3)).toHaveTextContent(/nas.*offline/);
    expect(row(4)).toHaveTextContent(/pi.*checking…/);
  });

  it("reports the picked device, and null for this device", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    hosts = [host(7, "homelab")];
    render(<DevicePicker hostId={7} onSelectHostId={onSelect} />);

    await user.click(trigger());
    await user.click(await screen.findByText("this device"));
    expect(onSelect).toHaveBeenLastCalledWith(null);

    await user.click(trigger());
    const row = document.querySelector<HTMLElement>('[data-host-id="7"]');
    expect(row).not.toBeNull();
    await user.click(row!);
    expect(onSelect).toHaveBeenLastCalledWith(7);
  });

  it("offers Add a device… with no devices and opens the add dialog in Settings", async () => {
    const user = userEvent.setup();
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await user.click(trigger());
    await user.click(await screen.findByRole("button", { name: "Add a device…" }));
    expect(setShowSettingsMock).toHaveBeenCalledWith(true, "hosts");
    expect(useAddDeviceDialogStore.getState().open).toBe(true);
  });

  it("offers Manage devices… once a device exists, without opening the add dialog", async () => {
    const user = userEvent.setup();
    hosts = [host(7, "homelab")];
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await user.click(trigger());
    await user.click(await screen.findByRole("button", { name: "Manage devices…" }));
    expect(setShowSettingsMock).toHaveBeenCalledWith(true, "hosts");
    expect(useAddDeviceDialogStore.getState().open).toBe(false);
  });

  it("does not open while disabled", async () => {
    const user = userEvent.setup();
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} disabled />);
    await user.click(trigger());
    expect(screen.queryByText("this device")).toBeNull();
  });
});
