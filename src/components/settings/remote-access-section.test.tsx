/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type {
  WebRemoteEndpoint,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

// ── Mocks ────────────────────────────────────────────────────────────

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  },
}));

// QR generation is async + environment-sensitive; stub it to a fixed SVG so
// the component test is deterministic.
vi.mock("./use-qr-svg", () => ({
  useQrSvg: (text: string | null) =>
    text ? "<svg aria-label='qr'></svg>" : null,
}));

const events = vi.hoisted(() => ({
  cb: null as null | ((s: WebRemoteStatus) => void),
}));
vi.mock("@/remote/web-remote-events", () => ({
  WEB_REMOTE_STATE_CHANGED_EVENT: "web-remote-state-changed",
  onWebRemoteStateChanged: vi.fn((cb: (s: WebRemoteStatus) => void) => {
    events.cb = cb;
    return Promise.resolve(() => {});
  }),
}));

const cmds = vi.hoisted(() => ({
  webRemoteStatus: vi.fn(),
  webRemoteEnable: vi.fn(),
  webRemoteDisable: vi.fn(),
  webRemoteSetConfig: vi.fn(),
  webRemoteCreatePairing: vi.fn(),
  webRemoteListEndpoints: vi.fn(),
  webRemoteListSessions: vi.fn(),
  webRemoteRevokeSession: vi.fn(),
  webRemoteApproveSession: vi.fn(),
  webRemoteRejectSession: vi.fn(),
}));
vi.mock("@/tauri/commands", () => cmds);

import { toast } from "@/lib/toast";
import { RemoteAccessSection } from "./remote-access-section";

// ── Fixtures ─────────────────────────────────────────────────────────

function sess(p: Partial<WebRemoteSessionView>): WebRemoteSessionView {
  return {
    id: "s",
    name: null,
    user_agent: null,
    created_at: "2026-07-04T00:00:00Z",
    last_seen_at: null,
    approved: true,
    connected: false,
    ...p,
  };
}

function endpoints(): WebRemoteEndpoint[] {
  return [
    {
      kind: "loopback",
      group: "this_device",
      host: "127.0.0.1",
      port: 4377,
      url: "http://127.0.0.1:4377",
      secure: true,
      recommended: false,
      label: "loopback",
    },
    {
      kind: "lan",
      group: "local_network",
      host: "192.168.1.42",
      port: 4377,
      url: "http://192.168.1.42:4377",
      secure: false,
      recommended: true,
      label: "lan",
    },
  ];
}

/** A richer set spanning every group — including a collapsed "other" — to
 *  exercise the grouped "Reachable at" rendering. */
function groupedEndpoints(): WebRemoteEndpoint[] {
  return [
    {
      kind: "loopback",
      group: "this_device",
      host: "127.0.0.1",
      port: 4377,
      url: "http://127.0.0.1:4377",
      secure: true,
      recommended: false,
      label: "",
    },
    {
      kind: "lan",
      group: "local_network",
      host: "192.168.68.58",
      port: 4377,
      url: "http://192.168.68.58:4377",
      secure: false,
      recommended: false,
      label: "",
    },
    {
      kind: "magicdns",
      group: "tailscale",
      host: "mac-studio.tail9c2f.ts.net",
      port: 4377,
      url: "https://mac-studio.tail9c2f.ts.net:4377",
      secure: true,
      recommended: true,
      label: "",
    },
    {
      kind: "lan",
      group: "other",
      host: "fd7a:115c:a1e0::42",
      port: 4377,
      url: "http://[fd7a:115c:a1e0::42]:4377",
      secure: false,
      recommended: false,
      label: "",
    },
  ];
}

function status(p: Partial<WebRemoteStatus>): WebRemoteStatus {
  const sessions = p.sessions ?? [];
  return {
    enabled: false,
    running: false,
    port: 4377,
    require_approval: true,
    active_connections: 0,
    connected_sessions: sessions.filter((s) => s.connected).length,
    sessions,
    ...p,
  };
}

const enabledStatus = () =>
  status({
    enabled: true,
    running: true,
    sessions: [
      sess({ id: "macbook", name: "MacBook Air", approved: true, connected: true }),
      sess({ id: "pending1", name: "workstation", approved: false }),
    ],
  });

afterEach(() => {
  cleanup();
  events.cb = null;
});

beforeEach(() => {
  for (const fn of Object.values(cmds)) (fn as ReturnType<typeof vi.fn>).mockReset();
  vi.mocked(toast.success).mockReset();
  vi.mocked(toast.error).mockReset();
  vi.mocked(toast.info).mockReset();
  cmds.webRemoteStatus.mockResolvedValue(status({}));
  cmds.webRemoteListEndpoints.mockResolvedValue(endpoints());
  cmds.webRemoteEnable.mockResolvedValue(enabledStatus());
  cmds.webRemoteDisable.mockResolvedValue(status({}));
  cmds.webRemoteApproveSession.mockResolvedValue(enabledStatus());
  cmds.webRemoteRevokeSession.mockResolvedValue(enabledStatus());
  cmds.webRemoteRejectSession.mockResolvedValue(enabledStatus());
  cmds.webRemoteSetConfig.mockResolvedValue(enabledStatus());
  cmds.webRemoteCreatePairing.mockResolvedValue({
    url_path: "/#pair=tok",
    token: "tok",
    expires_at: new Date(Date.now() + 600_000).toISOString(),
  });
});

// ── Tests ────────────────────────────────────────────────────────────

describe("RemoteAccessSection — disabled", () => {
  it("shows the header, an unchecked master toggle, and the exposure warning; hides the server/device panels", async () => {
    render(<RemoteAccessSection />);
    await waitFor(() => expect(cmds.webRemoteStatus).toHaveBeenCalled());

    expect(screen.getByRole("heading", { name: /^Remote Access$/i })).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: /toggle remote access/i });
    expect(toggle).toHaveAttribute("data-state", "unchecked");
    expect(screen.getByText(/every network interface/i)).toBeInTheDocument();

    // Collapsed while off.
    expect(screen.queryByText(/Reachable at/i)).toBeNull();
    expect(screen.queryByText(/Paired devices/i)).toBeNull();
  });

  it("enabling calls webRemoteEnable", async () => {
    const user = userEvent.setup();
    render(<RemoteAccessSection />);
    await waitFor(() => expect(cmds.webRemoteStatus).toHaveBeenCalled());

    await user.click(screen.getByRole("switch", { name: /toggle remote access/i }));
    await waitFor(() => expect(cmds.webRemoteEnable).toHaveBeenCalledTimes(1));
  });
});

describe("RemoteAccessSection — enabled", () => {
  beforeEach(() => {
    cmds.webRemoteStatus.mockResolvedValue(enabledStatus());
  });

  it("renders endpoints grouped under labelled headers with Secure vs Limited hints", async () => {
    render(<RemoteAccessSection />);
    await waitFor(() =>
      expect(screen.getByText("http://127.0.0.1:4377")).toBeInTheDocument(),
    );
    expect(screen.getByText("http://192.168.1.42:4377")).toBeInTheDocument();
    expect(screen.getByText("Secure")).toBeInTheDocument();
    expect(screen.getByText("Limited")).toBeInTheDocument();
    // Grouped headers + the recommended chip on the primary LAN endpoint.
    expect(screen.getByText("This device")).toBeInTheDocument();
    expect(screen.getByText("Local network")).toBeInTheDocument();
    expect(screen.getByText("Recommended")).toBeInTheDocument();
    // The insecure hint spells out the clipboard/notifications degradation.
    expect(
      screen.getByText(/browser clipboard and notifications are disabled/i),
    ).toBeInTheDocument();
  });

  it("shows the Tailscale group and hides 'other' addresses behind a disclosure", async () => {
    cmds.webRemoteListEndpoints.mockResolvedValue(groupedEndpoints());
    render(<RemoteAccessSection />);
    await waitFor(() =>
      expect(
        screen.getByText("https://mac-studio.tail9c2f.ts.net:4377"),
      ).toBeInTheDocument(),
    );
    // Tailscale section header + its explanation are visible.
    expect(screen.getByText("Tailscale")).toBeInTheDocument();
    expect(screen.getByText(/set up Tailscale's HTTPS serve/i)).toBeInTheDocument();
    // The "other" IPv6 address lives inside a collapsed disclosure — its
    // details element is present but closed by default.
    const otherSummary = screen.getByText(/Other addresses/i);
    const details = otherSummary.closest("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
  });

  it("renders the pending-approval row and approves it", async () => {
    const user = userEvent.setup();
    render(<RemoteAccessSection />);
    await waitFor(() => expect(screen.getByText(/Waiting for approval/i)).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /approve/i }));
    await waitFor(() =>
      expect(cmds.webRemoteApproveSession).toHaveBeenCalledWith("pending1"),
    );
  });

  it("rejects a pending device", async () => {
    const user = userEvent.setup();
    render(<RemoteAccessSection />);
    await waitFor(() => expect(screen.getByRole("button", { name: /reject/i })).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /reject/i }));
    await waitFor(() =>
      expect(cmds.webRemoteRejectSession).toHaveBeenCalledWith("pending1"),
    );
  });

  it("revokes a paired device", async () => {
    const user = userEvent.setup();
    render(<RemoteAccessSection />);
    await waitFor(() => expect(screen.getByText("MacBook Air")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /^revoke$/i }));
    await waitFor(() =>
      expect(cmds.webRemoteRevokeSession).toHaveBeenCalledWith("macbook"),
    );
  });

  it("creates a pairing link and shows the QR + link", async () => {
    const user = userEvent.setup();
    render(<RemoteAccessSection />);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /create pairing link/i })).toBeInTheDocument(),
    );

    await user.click(screen.getByRole("button", { name: /create pairing link/i }));
    await waitFor(() => expect(cmds.webRemoteCreatePairing).toHaveBeenCalled());
    // Full pair URL composed against the primary (non-loopback) endpoint.
    await waitFor(() =>
      expect(screen.getByText("http://192.168.1.42:4377/#pair=tok")).toBeInTheDocument(),
    );
    expect(screen.getByLabelText(/pairing QR code/i)).toBeInTheDocument();
  });

  it("toasts and badges a new pending device arriving over the live event", async () => {
    render(<RemoteAccessSection />);
    await waitFor(() => expect(events.cb).not.toBeNull());
    // One pending device already ("Waiting for approval" with count 1).
    await waitFor(() => expect(screen.getByText(/Waiting for approval/i)).toBeInTheDocument());

    act(() => {
      events.cb?.(
        status({
          enabled: true,
          running: true,
          sessions: [
            sess({ id: "macbook", name: "MacBook Air", approved: true, connected: true }),
            sess({ id: "pending1", name: "workstation", approved: false }),
            sess({ id: "pending2", name: "iPad", approved: false }),
          ],
        }),
      );
    });

    await waitFor(() =>
      expect(toast.info).toHaveBeenCalledWith(
        expect.stringMatching(/iPad wants to connect/i),
        expect.objectContaining({ description: expect.any(String) }),
      ),
    );
    // The new pending device now shows in the list.
    expect(screen.getByText("iPad")).toBeInTheDocument();
  });
});
