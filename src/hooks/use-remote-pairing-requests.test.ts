import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

import type { WebRemoteSessionView, WebRemoteStatus } from "@/tauri/types";

vi.mock("@/lib/toast", () => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
    error: vi.fn(),
    dismiss: vi.fn(),
  },
}));

const events = vi.hoisted(() => ({
  cb: null as null | ((s: WebRemoteStatus) => void),
}));
vi.mock("@/remote/web-remote-events", () => ({
  onWebRemoteStateChanged: vi.fn((cb: (s: WebRemoteStatus) => void) => {
    events.cb = cb;
    return Promise.resolve(() => {});
  }),
}));

const cmds = vi.hoisted(() => ({
  webRemoteStatus: vi.fn(),
  webRemoteApproveSession: vi.fn(),
  webRemoteRejectSession: vi.fn(),
}));
vi.mock("@/tauri/commands", () => cmds);

import { toast } from "@/lib/toast";
import { useRemotePairingRequests } from "./use-remote-pairing-requests";

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

function status(sessions: WebRemoteSessionView[]): WebRemoteStatus {
  return { sessions } as WebRemoteStatus;
}

type ToastOpts = {
  id: string;
  duration: number;
  action: { onClick: () => void };
  cancel: { onClick: () => void };
};

/** The first question asked, with its toast options narrowed to the fields
 *  the hook always sets. */
function firstQuestion(): [string, ToastOpts] {
  return vi.mocked(toast.info).mock.calls[0] as unknown as [string, ToastOpts];
}

beforeEach(() => {
  events.cb = null;
  cmds.webRemoteStatus.mockResolvedValue(
    status([sess({ id: "old", name: "Old tablet", approved: false })]),
  );
  cmds.webRemoteApproveSession.mockResolvedValue(status([]));
  cmds.webRemoteRejectSession.mockResolvedValue(status([]));
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});

async function mounted() {
  renderHook(() => useRemotePairingRequests());
  await waitFor(() => expect(events.cb).not.toBeNull());
  await waitFor(() => expect(cmds.webRemoteStatus).toHaveBeenCalled());
  // Let the seeding snapshot settle before events arrive.
  await Promise.resolve();
}

describe("useRemotePairingRequests", () => {
  it("asks once for a new request, with Approve and Reject, and leaves older ones alone", async () => {
    await mounted();
    const next = status([
      sess({ id: "old", name: "Old tablet", approved: false }),
      sess({ id: "new", name: "iPhone", approved: false }),
    ]);
    events.cb?.(next);
    events.cb?.(next);

    expect(toast.info).toHaveBeenCalledTimes(1);
    const [title, opts] = firstQuestion();
    expect(title).toBe("iPhone wants to connect");
    expect(opts.duration).toBe(Infinity);

    opts.action.onClick();
    expect(cmds.webRemoteApproveSession).toHaveBeenCalledWith("new");
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith("iPhone can now use this desktop."),
    );

    opts.cancel.onClick();
    expect(cmds.webRemoteRejectSession).toHaveBeenCalledWith("new");
  });

  it("closes the question once the request is answered elsewhere", async () => {
    await mounted();
    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: false })]));
    const [, opts] = firstQuestion();

    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: true })]));
    expect(toast.dismiss).toHaveBeenCalledWith(opts.id);
  });

  it("does nothing in a remote browser, which cannot approve others", async () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    renderHook(() => useRemotePairingRequests());
    await Promise.resolve();
    expect(cmds.webRemoteStatus).not.toHaveBeenCalled();
    expect(events.cb).toBeNull();
  });
});
