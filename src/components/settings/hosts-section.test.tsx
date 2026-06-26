/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { HostView } from "@/tauri/commands";

// `hosts_reinstall_remote` is the dev-workflow escape hatch (issue #24):
// it re-uploads codemux-remote and restarts its daemon regardless of the
// version already installed. These tests pin the UI wiring — the right
// command is called for the selected host, and the outcome surfaces as a
// success/error toast.
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    hostsList: vi.fn(),
    hostsReinstallRemote: vi.fn(),
    hostsTestConnection: vi.fn(),
  };
});

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

import { HostsSection } from "./hosts-section";
import { hostsList, hostsReinstallRemote } from "@/tauri/commands";
import { toast } from "@/lib/toast";

const hostsListMock = hostsList as unknown as ReturnType<typeof vi.fn>;
const hostsReinstallRemoteMock =
  hostsReinstallRemote as unknown as ReturnType<typeof vi.fn>;

function makeHost(overrides: Partial<HostView> = {}): HostView {
  return {
    id: 1,
    server_id: null,
    name: "homelab",
    ssh_target: "deus@homelab.local",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("HostsSection — Reinstall agent", () => {
  it("reinstalls the selected host and shows a success toast", async () => {
    hostsListMock.mockResolvedValue([makeHost()]);
    hostsReinstallRemoteMock.mockResolvedValue({
      ok: true,
      message:
        "codemux-remote v0.9.5 reinstalled on homelab — daemon restarted; " +
        "the next push uses the fresh binary.",
    });

    render(<HostsSection />);

    const button = await screen.findByRole("button", { name: /^reinstall$/i });
    fireEvent.click(button);

    await waitFor(() => {
      // The first (and only) host is auto-selected on load, so the
      // reinstall targets its id.
      expect(hostsReinstallRemoteMock).toHaveBeenCalledWith(1);
    });
    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(
        "Agent reinstalled",
        expect.objectContaining({
          description: expect.stringContaining("reinstalled"),
        }),
      );
    });
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("shows an error toast when the reinstall fails", async () => {
    hostsListMock.mockResolvedValue([makeHost()]);
    hostsReinstallRemoteMock.mockResolvedValue({
      ok: false,
      message: "Reinstall failed: upload: connection refused",
    });

    render(<HostsSection />);

    const button = await screen.findByRole("button", { name: /^reinstall$/i });
    fireEvent.click(button);

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(
        "Reinstall failed",
        expect.objectContaining({
          description: expect.stringContaining("upload"),
        }),
      );
    });
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("surfaces a thrown command error as an error toast", async () => {
    hostsListMock.mockResolvedValue([makeHost()]);
    hostsReinstallRemoteMock.mockRejectedValue("ssh: host key verification failed");

    render(<HostsSection />);

    const button = await screen.findByRole("button", { name: /^reinstall$/i });
    fireEvent.click(button);

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(
        "Reinstall failed",
        expect.objectContaining({
          description: expect.stringContaining("host key verification"),
        }),
      );
    });
  });
});
