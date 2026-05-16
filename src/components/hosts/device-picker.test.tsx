/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  hostsList: vi.fn(),
}));

import { hostsList, type HostView } from "@/tauri/commands";
import { DevicePicker } from "./device-picker";

afterEach(() => cleanup());

function host(over: Partial<HostView>): HostView {
  return {
    id: 1,
    server_id: null,
    name: "homelab",
    ssh_target: "u@h",
    created_at: "2026-05-16",
    updated_at: "2026-05-16",
    dirty: false,
    ...over,
  };
}

describe("DevicePicker", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows 'Local Device' label when hostId is null", async () => {
    vi.mocked(hostsList).mockResolvedValue([]);
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveAttribute(
        "aria-label",
        "Device: Local Device",
      );
    });
  });

  it("respects the localLabel override", async () => {
    vi.mocked(hostsList).mockResolvedValue([]);
    render(
      <DevicePicker
        hostId={null}
        onSelectHostId={() => {}}
        localLabel="This device"
      />,
    );
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveAttribute(
        "aria-label",
        "Device: This device",
      );
    });
  });

  it("shows the host name when a remote host is selected", async () => {
    vi.mocked(hostsList).mockResolvedValue([
      host({ id: 7, name: "homelab", ssh_target: "u@h" }),
      host({ id: 8, name: "vps-fra", ssh_target: "u@v" }),
    ]);
    render(<DevicePicker hostId={7} onSelectHostId={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveAttribute(
        "aria-label",
        "Device: homelab",
      );
    });
  });

  it("falls back to local label if the configured hostId no longer exists", async () => {
    // Realistic scenario: workspace was assigned to a host that's
    // since been deleted on another device. We must not crash; we
    // also must not pretend it's still selected. Showing "Local
    // Device" is the safest default.
    vi.mocked(hostsList).mockResolvedValue([]);
    render(<DevicePicker hostId={999} onSelectHostId={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveAttribute(
        "aria-label",
        "Device: Local Device",
      );
    });
  });

  it("opens the dropdown and exposes the Local Device entry", async () => {
    const user = userEvent.setup();
    vi.mocked(hostsList).mockResolvedValue([]);
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await waitFor(() => screen.getByRole("button"));
    await user.click(screen.getByRole("button"));
    // The trigger label and the menu item both contain "Local Device"
    // — assert at-least-one match (we don't care which one). findAll
    // is the right primitive for "render eventually showed this."
    await waitFor(() =>
      expect(screen.getAllByText("Local Device").length).toBeGreaterThanOrEqual(1),
    );
  });

  it("renders the 'Other Hosts' submenu only when remote hosts exist", async () => {
    const user = userEvent.setup();
    vi.mocked(hostsList).mockResolvedValue([
      host({ id: 7, name: "homelab" }),
    ]);
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await waitFor(() => screen.getByRole("button"));
    await user.click(screen.getByRole("button"));
    await waitFor(() =>
      expect(screen.getAllByText("Other Hosts").length).toBeGreaterThanOrEqual(1),
    );
  });

  it("does not throw when hostsList rejects (falls back to local-only)", async () => {
    // Defensive: a broken DB or auth state shouldn't crash the
    // surrounding new-workspace dialog. The picker must render the
    // local option even if the listing failed.
    vi.mocked(hostsList).mockRejectedValue(new Error("db down"));
    render(<DevicePicker hostId={null} onSelectHostId={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveAttribute(
        "aria-label",
        "Device: Local Device",
      );
    });
  });
});
