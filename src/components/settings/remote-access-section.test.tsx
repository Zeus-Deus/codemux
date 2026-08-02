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
  webRemoteRegistrationStatus: vi.fn(),
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
  cmds.webRemoteRegistrationStatus.mockResolvedValue({ registered: false });
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

  it("defaults the access scope to All networks and switches to Tailscale only", async () => {
    const user = userEvent.setup();
    cmds.webRemoteSetConfig.mockResolvedValueOnce(
      status({
        enabled: true,
        running: true,
        bind_scope: "tailscale",
        sessions: [sess({ id: "macbook", name: "MacBook Air", approved: true })],
      }),
    );
    render(<RemoteAccessSection />);

    // No explicit bind_scope on the initial status → the control defaults to
    // "All networks" being selected.
    const allBtn = await screen.findByRole("radio", { name: /all networks/i });
    await waitFor(() => expect(allBtn).toHaveAttribute("aria-checked", "true"));

    await user.click(screen.getByRole("radio", { name: /tailscale only/i }));
    await waitFor(() =>
      expect(cmds.webRemoteSetConfig).toHaveBeenCalledWith({ bindScope: "tailscale" }),
    );
    // The returned status flips the active option.
    await waitFor(() =>
      expect(screen.getByRole("radio", { name: /tailscale only/i })).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    );
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

describe("RemoteAccessSection — account access", () => {
  const accountStatus = (p: Partial<WebRemoteStatus> = {}) =>
    status({
      enabled: true,
      running: true,
      account_mode_enabled: true,
      account_signed_in: true,
      trust_account_browsers: false,
      sessions: [
        sess({
          id: "acct-laptop",
          name: "Work laptop",
          approved: true,
          source: "account",
        }),
        sess({
          id: "paired-phone",
          name: "Phone",
          approved: true,
          source: "pair",
        }),
      ],
      ...p,
    });

  it("shows the account toggle and the approval opt-out when account mode is on", async () => {
    cmds.webRemoteStatus.mockResolvedValue(accountStatus());
    render(<RemoteAccessSection />);
    expect(
      await screen.findByLabelText(/toggle account sign-in/i),
    ).toBeChecked();
    // The "trust browsers without approval" opt-out is visible + off.
    expect(
      screen.getByLabelText(/toggle trust account browsers/i),
    ).not.toBeChecked();
  });

  it("toggling account sign-in calls webRemoteSetConfig({ accountModeEnabled })", async () => {
    const user = userEvent.setup();
    cmds.webRemoteStatus.mockResolvedValue(
      accountStatus({ account_mode_enabled: false }),
    );
    render(<RemoteAccessSection />);
    const toggle = await screen.findByLabelText(/toggle account sign-in/i);
    expect(toggle).not.toBeChecked();

    await user.click(toggle);
    await waitFor(() =>
      expect(cmds.webRemoteSetConfig).toHaveBeenCalledWith({
        accountModeEnabled: true,
      }),
    );
  });

  it("toggling the opt-out calls webRemoteSetConfig({ trustAccountBrowsers })", async () => {
    const user = userEvent.setup();
    cmds.webRemoteStatus.mockResolvedValue(accountStatus());
    render(<RemoteAccessSection />);

    await user.click(
      await screen.findByLabelText(/toggle trust account browsers/i),
    );
    await waitFor(() =>
      expect(cmds.webRemoteSetConfig).toHaveBeenCalledWith({
        trustAccountBrowsers: true,
      }),
    );
  });

  it("warns when account mode is on but the desktop is signed out", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      accountStatus({ account_signed_in: false }),
    );
    render(<RemoteAccessSection />);
    expect(
      await screen.findByText(/isn't signed into a Codemux account/i),
    ).toBeInTheDocument();
  });

  it("tags account-minted vs paired devices in the list", async () => {
    cmds.webRemoteStatus.mockResolvedValue(accountStatus());
    render(<RemoteAccessSection />);
    await screen.findByText("Work laptop");
    // Both an "Account" and a "Paired" tag are present in the devices list.
    expect(screen.getAllByText("Account").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("Paired").length).toBeGreaterThanOrEqual(1);
  });
});

// The from-anywhere (relay) transport: a master toggle that rides
// `web_remote_set_config`, plus a registration readout sourced from
// `web_remote_registration_status` with the live status broadcast as fallback.
describe("RemoteAccessSection — from anywhere (relay)", () => {
  const relayStatus = (p: Partial<WebRemoteStatus> = {}) =>
    status({
      enabled: true,
      running: true,
      account_signed_in: true,
      sessions: [sess({ id: "macbook", name: "MacBook Air", approved: true })],
      ...p,
    });

  it("is off by default and doesn't read registration state", async () => {
    cmds.webRemoteStatus.mockResolvedValue(relayStatus());
    render(<RemoteAccessSection />);

    const toggle = await screen.findByLabelText(/toggle from-anywhere access/i);
    expect(toggle).not.toBeChecked();
    // No readout while off…
    expect(screen.queryByText(/device registration/i)).toBeNull();
    // …and the section explains what turning it on does.
    expect(
      screen.getByText(/end-to-end encrypted between that browser/i),
    ).toBeInTheDocument();
    expect(cmds.webRemoteRegistrationStatus).not.toHaveBeenCalled();
  });

  it("toggling calls webRemoteSetConfig({ relayModeEnabled })", async () => {
    const user = userEvent.setup();
    cmds.webRemoteStatus.mockResolvedValue(relayStatus());
    render(<RemoteAccessSection />);

    await user.click(await screen.findByLabelText(/toggle from-anywhere access/i));
    await waitFor(() =>
      expect(cmds.webRemoteSetConfig).toHaveBeenCalledWith({
        relayModeEnabled: true,
      }),
    );
  });

  it("shows the registered readout with the device identity when on", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({ relay_mode_enabled: true, device_registered: true }),
    );
    cmds.webRemoteRegistrationStatus.mockResolvedValue({
      registered: true,
      device_id: "device-abc123",
      node_id: "node-xyz789",
      last_registered_at: new Date(Date.now() - 60_000).toISOString(),
      last_error: null,
    });
    render(<RemoteAccessSection />);

    expect(await screen.findByLabelText(/toggle from-anywhere access/i)).toBeChecked();
    await waitFor(() => expect(screen.getByText(/^Registered$/)).toBeInTheDocument());
    // No `name` from this (older) backend, so the id is the fallback label.
    expect(screen.getByText("device-abc123")).toBeInTheDocument();
    expect(screen.getByText("node-xyz789")).toBeInTheDocument();
    expect(
      screen.getByText(/listed with your account, so a browser signed into it/i),
    ).toBeInTheDocument();
  });

  it("names the device by hostname when the backend reports one", async () => {
    // "Registered as vps-fra-1" is what a user recognises in the device
    // picker; the raw uuid is only meaningful when two hosts share a name.
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({ relay_mode_enabled: true, device_registered: true }),
    );
    cmds.webRemoteRegistrationStatus.mockResolvedValue({
      registered: true,
      device_id: "device-abc123",
      name: "vps-fra-1",
      node_id: "node-xyz789",
    });
    render(<RemoteAccessSection />);

    await waitFor(() =>
      expect(screen.getByText("vps-fra-1")).toBeInTheDocument(),
    );
    expect(screen.queryByText("device-abc123")).toBeNull();
    expect(
      screen.getByLabelText(/copy device name/i),
    ).toBeInTheDocument();
  });

  it("shows a pending readout (with the last error) when registration hasn't landed", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({ relay_mode_enabled: true, device_registered: false }),
    );
    cmds.webRemoteRegistrationStatus.mockResolvedValue({
      registered: false,
      device_id: "device-abc123",
      node_id: null,
      last_registered_at: null,
      last_error: "control plane unreachable",
    });
    render(<RemoteAccessSection />);

    await waitFor(() =>
      expect(screen.getByText(/not registered yet/i)).toBeInTheDocument(),
    );
    expect(
      await screen.findByText(/last attempt failed: control plane unreachable/i),
    ).toBeInTheDocument();
  });

  it("warns when relay mode is on but the desktop is signed out", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({ relay_mode_enabled: true, account_signed_in: false }),
    );
    render(<RemoteAccessSection />);

    expect(
      await screen.findByText(/can't register for from-anywhere access/i),
    ).toBeInTheDocument();
    // The registration readout is replaced by the sign-in warning.
    expect(screen.queryByText(/device registration/i)).toBeNull();
  });

  it("live-updates the readout when the state-changed event flips registration", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({ relay_mode_enabled: true, device_registered: false }),
    );
    cmds.webRemoteRegistrationStatus.mockResolvedValue({
      registered: false,
      device_id: "device-abc123",
    });
    render(<RemoteAccessSection />);
    await waitFor(() =>
      expect(screen.getByText(/not registered yet/i)).toBeInTheDocument(),
    );

    cmds.webRemoteRegistrationStatus.mockResolvedValue({
      registered: true,
      device_id: "device-abc123",
      node_id: "node-xyz789",
    });
    act(() => {
      events.cb?.(
        relayStatus({ relay_mode_enabled: true, device_registered: true }),
      );
    });

    await waitFor(() => expect(screen.getByText(/^Registered$/)).toBeInTheDocument());
    expect(screen.getByText("node-xyz789")).toBeInTheDocument();
  });

  it("falls back to the status flags when the registration read fails", async () => {
    cmds.webRemoteStatus.mockResolvedValue(
      relayStatus({
        relay_mode_enabled: true,
        device_registered: true,
        iroh_node_id: "node-from-status",
      }),
    );
    cmds.webRemoteRegistrationStatus.mockRejectedValue(
      new Error("unknown command"),
    );
    render(<RemoteAccessSection />);

    await waitFor(() => expect(screen.getByText(/^Registered$/)).toBeInTheDocument());
    expect(screen.getByText("node-from-status")).toBeInTheDocument();
  });
});

// The rebind bug: a scope/port change from the *web* client rebinds the
// server and drops this socket before web_remote_set_config can answer, so the
// invoke rejects with "connection lost" even though the change was applied.
// The old handler surfaced that as an error toast and snapped the control
// back. On a web client it must instead reflect the requested value and show a
// reconnecting affordance.
describe("RemoteAccessSection — web-client rebind disconnect", () => {
  beforeEach(() => {
    cmds.webRemoteStatus.mockResolvedValue(enabledStatus());
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
  });
  afterEach(() => {
    delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  });

  it("treats a scope-rebind disconnect as expected: no error toast, keeps the requested value", async () => {
    const user = userEvent.setup();
    // The jsdom origin is http://localhost → a loopback origin, which survives
    // every scope (no cutoff confirm). The rebind still drops the socket.
    cmds.webRemoteSetConfig.mockRejectedValueOnce(
      new Error("web-remote: connection lost"),
    );
    render(<RemoteAccessSection />);
    const tailscaleBtn = await screen.findByRole("radio", {
      name: /tailscale only/i,
    });

    await user.click(tailscaleBtn);
    await waitFor(() =>
      expect(cmds.webRemoteSetConfig).toHaveBeenCalledWith({ bindScope: "tailscale" }),
    );

    // Optimistic value sticks (no snap-back to All networks)…
    await waitFor(() =>
      expect(
        screen.getByRole("radio", { name: /tailscale only/i }),
      ).toHaveAttribute("aria-checked", "true"),
    );
    // …a reconnecting affordance shows instead of an error…
    expect(screen.getByText(/reconnecting to this device/i)).toBeInTheDocument();
    // …and crucially, NO error toast for the expected disconnect.
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("still surfaces a genuine backend rejection as an error and reverts", async () => {
    const user = userEvent.setup();
    cmds.webRemoteSetConfig.mockRejectedValueOnce(
      new Error("No Tailscale address found — connect Tailscale or choose a different access scope"),
    );
    render(<RemoteAccessSection />);
    const tailscaleBtn = await screen.findByRole("radio", {
      name: /tailscale only/i,
    });

    await user.click(tailscaleBtn);
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    // Reverted: the control snaps back to the server's actual scope.
    await waitFor(() =>
      expect(
        screen.getByRole("radio", { name: /all networks/i }),
      ).toHaveAttribute("aria-checked", "true"),
    );
    expect(screen.queryByText(/reconnecting to this device/i)).toBeNull();
  });
});
