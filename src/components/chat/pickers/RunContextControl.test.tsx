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

import {
  RunContextControl,
  type RunContextControlProps,
} from "./RunContextControl";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";

afterEach(() => cleanup());

beforeEach(() => {
  hosts = [];
  statuses = {};
  localName = "ai-node";
  setShowSettingsMock.mockClear();
  useAddDeviceDialogStore.setState({ open: false });
});

function renderControl(overrides: Partial<RunContextControlProps> = {}) {
  const props: RunContextControlProps = {
    hostId: null,
    onChangeHostId: vi.fn(),
    showWorkspace: false,
    checkoutMode: "worktree",
    worktreeName: "",
    onChangeCheckoutMode: vi.fn(),
    onChangeWorktreeName: vi.fn(),
    ...overrides,
  };
  render(<RunContextControl {...props} />);
  return props;
}

function trigger() {
  return screen.getByRole("button", { name: /^Run on:/ });
}

describe("RunContextControl", () => {
  describe("device", () => {
    it("names this device generically, while the menu row carries the hostname", async () => {
      const user = userEvent.setup();
      renderControl();
      expect(trigger()).toHaveAccessibleName("Run on: This device");

      await user.click(trigger());
      const local = (await screen.findByText("this device")).closest("[cmdk-item]");
      expect(local).toHaveTextContent("ai-node");
      expect(local).toContainElement(screen.getByLabelText("Selected"));
    });

    it("shows the device name when a device is selected", () => {
      hosts = [host(7, "homelab"), host(8, "vps-fra")];
      renderControl({ hostId: 7 });
      expect(trigger()).toHaveAccessibleName("Run on: homelab");
    });

    it("shows a neutral label, with no row checked, for a device the list doesn't know", async () => {
      // The send still goes to that device (e.g. the list failed to load),
      // so the control must not claim this machine.
      const user = userEvent.setup();
      hosts = [host(7, "homelab")];
      renderControl({ hostId: 999 });
      expect(trigger()).toHaveAccessibleName("Run on: Device unavailable");

      await user.click(trigger());
      await screen.findByText("this device");
      expect(screen.queryByLabelText("Selected")).toBeNull();
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
      renderControl();
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
      hosts = [host(7, "homelab")];
      const { onChangeHostId } = renderControl({ hostId: 7 });

      await user.click(trigger());
      await user.click(await screen.findByText("this device"));
      expect(onChangeHostId).toHaveBeenLastCalledWith(null);

      await user.click(trigger());
      const row = document.querySelector<HTMLElement>('[data-host-id="7"]');
      expect(row).not.toBeNull();
      await user.click(row!);
      expect(onChangeHostId).toHaveBeenLastCalledWith(7);
    });

    it("offers Add a device… with no devices and opens the add dialog in Settings", async () => {
      const user = userEvent.setup();
      renderControl();
      await user.click(trigger());
      await user.click(await screen.findByRole("button", { name: "Add a device…" }));
      expect(setShowSettingsMock).toHaveBeenCalledWith(true, "hosts");
      expect(useAddDeviceDialogStore.getState().open).toBe(true);
    });

    it("offers Manage devices… once a device exists, without opening the add dialog", async () => {
      const user = userEvent.setup();
      hosts = [host(7, "homelab")];
      renderControl();
      await user.click(trigger());
      await user.click(await screen.findByRole("button", { name: "Manage devices…" }));
      expect(setShowSettingsMock).toHaveBeenCalledWith(true, "hosts");
      expect(useAddDeviceDialogStore.getState().open).toBe(false);
    });

    it("does not open while disabled", async () => {
      const user = userEvent.setup();
      renderControl({ disabled: true });
      await user.click(trigger());
      expect(screen.queryByText("this device")).toBeNull();
    });
  });

  describe("workspace", () => {
    it("names the workspace alongside the device", () => {
      renderControl({ showWorkspace: true, checkoutMode: "worktree" });
      expect(trigger()).toHaveAccessibleName("Run on: This device · New worktree");
      cleanup();
      renderControl({ showWorkspace: true, checkoutMode: "worktree", worktreeName: "fix-login" });
      expect(trigger()).toHaveAccessibleName("Run on: This device · fix-login");
      cleanup();
      renderControl({ showWorkspace: true, checkoutMode: "current" });
      expect(trigger()).toHaveAccessibleName("Run on: This device · Current checkout");
    });

    it("leaves out the device list until a device is configured", async () => {
      const user = userEvent.setup();
      renderControl({ showWorkspace: true });
      await user.click(trigger());
      expect(await screen.findByText("Workspace")).toBeInTheDocument();
      expect(screen.queryByText("Run on")).toBeNull();
      expect(screen.queryByText("this device")).toBeNull();
      expect(screen.getByRole("button", { name: "Add a device…" })).toBeInTheDocument();
    });

    it("lists devices above the workspace once one is configured", async () => {
      const user = userEvent.setup();
      hosts = [host(7, "homelab")];
      renderControl({ showWorkspace: true });
      await user.click(trigger());
      const runOn = await screen.findByText("Run on");
      expect(
        runOn.compareDocumentPosition(screen.getByText("Workspace")) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    });

    it("reports the picked checkout mode", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode } = renderControl({
        showWorkspace: true,
        checkoutMode: "worktree",
      });
      await user.click(trigger());
      await user.click(await screen.findByText("Current checkout"));
      expect(onChangeCheckoutMode).toHaveBeenLastCalledWith("current");
    });

    it("renames the worktree in place from the pencil, saving on Enter", async () => {
      const user = userEvent.setup();
      const { onChangeWorktreeName, onChangeCheckoutMode } = renderControl({
        showWorkspace: true,
        checkoutMode: "worktree",
      });
      await user.click(trigger());
      await user.click(await screen.findByRole("button", { name: "Rename worktree" }));
      const input = screen.getByRole("textbox", { name: "Worktree name" });
      expect(input).toHaveFocus();
      await user.type(input, "fix-login{Enter}");

      expect(onChangeWorktreeName).toHaveBeenCalledTimes(1);
      expect(onChangeWorktreeName).toHaveBeenCalledWith("fix-login");
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      // The menu stays open on the workspace choices.
      expect(screen.queryByRole("textbox", { name: "Worktree name" })).toBeNull();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
    });

    it("switches to a new worktree when renaming from the current checkout", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode } = renderControl({
        showWorkspace: true,
        checkoutMode: "current",
      });
      await user.click(trigger());
      await user.click(await screen.findByRole("button", { name: "Rename worktree" }));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
    });

    it("reverts on Escape without closing the menu", async () => {
      const user = userEvent.setup();
      const { onChangeWorktreeName } = renderControl({
        showWorkspace: true,
        worktreeName: "old-name",
      });
      await user.click(trigger());
      await user.click(await screen.findByRole("button", { name: "Rename worktree" }));
      const input = screen.getByRole("textbox", { name: "Worktree name" });
      expect(input).toHaveValue("old-name");
      await user.clear(input);
      await user.type(input, "new-name{Escape}");

      expect(onChangeWorktreeName).not.toHaveBeenCalled();
      expect(screen.queryByRole("textbox", { name: "Worktree name" })).toBeNull();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
    });

    it("shows a typed name faintly on the New worktree row", async () => {
      const user = userEvent.setup();
      renderControl({ showWorkspace: true, worktreeName: "fix-login" });
      await user.click(trigger());
      const row = (await screen.findByText("New worktree")).closest("[cmdk-item]");
      expect(row).toHaveTextContent("fix-login");
    });

    it("renames the highlighted New worktree row with F2", async () => {
      const user = userEvent.setup();
      renderControl({ showWorkspace: true, checkoutMode: "current" });
      await user.click(trigger());
      await screen.findByText("Workspace");
      // First row (Current checkout) starts highlighted; move to New worktree.
      await user.keyboard("{ArrowDown}{F2}");
      expect(screen.getByRole("textbox", { name: "Worktree name" })).toHaveFocus();
    });
  });
});
