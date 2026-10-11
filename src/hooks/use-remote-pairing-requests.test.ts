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
  unlisten: vi.fn(),
}));
vi.mock("@/remote/web-remote-events", () => ({
  onWebRemoteStateChanged: vi.fn((cb: (s: WebRemoteStatus) => void) => {
    events.cb = cb;
    return Promise.resolve(events.unlisten);
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
  cmds.webRemoteApproveSession.mockResolvedValue(
    status([sess({ id: "new", name: "iPhone", approved: true })]),
  );
  cmds.webRemoteRejectSession.mockResolvedValue(status([]));
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});

async function mounted() {
  const hook = renderHook(() => useRemotePairingRequests());
  await waitFor(() => expect(events.cb).not.toBeNull());
  await waitFor(() => expect(cmds.webRemoteStatus).toHaveBeenCalled());
  // Let the seeding snapshot settle before events arrive.
  await Promise.resolve();
  return hook;
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

  it("says what is asking in plain words", async () => {
    await mounted();
    events.cb?.(
      status([
        sess({
          id: "new",
          name: "iPhone",
          approved: false,
          user_agent:
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
        }),
      ]),
    );
    const [, opts] = firstQuestion();
    expect((opts as ToastOpts & { description: string }).description).toBe(
      "Chrome on Linux. Used a pairing link.",
    );
  });

  it("does not repeat the platform when the browser is named after it", async () => {
    await mounted();
    events.cb?.(
      status([
        sess({
          id: "new",
          name: "Chrome on Linux",
          approved: false,
          user_agent:
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
        }),
      ]),
    );
    const [, opts] = firstQuestion();
    expect((opts as ToastOpts & { description: string }).description).toBe(
      "Used a pairing link.",
    );
  });

  it("asks about each of several requests that arrive together", async () => {
    await mounted();
    events.cb?.(
      status([
        sess({ id: "a", name: "iPhone", approved: false }),
        sess({ id: "b", name: "iPad", approved: false }),
      ]),
    );
    expect(vi.mocked(toast.info).mock.calls.map(([title]) => title)).toEqual([
      "iPhone wants to connect",
      "iPad wants to connect",
    ]);
  });

  it("does not claim success when the request is gone before approval lands", async () => {
    cmds.webRemoteApproveSession.mockResolvedValue(status([]));
    await mounted();
    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: false })]));

    firstQuestion()[1].action.onClick();
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("iPhone is no longer waiting to connect."),
    );
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("reports a failed approve or reject", async () => {
    cmds.webRemoteApproveSession.mockRejectedValue("db locked");
    cmds.webRemoteRejectSession.mockRejectedValue("db locked");
    await mounted();
    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: false })]));
    const [, opts] = firstQuestion();

    opts.action.onClick();
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("Couldn't approve iPhone: db locked"),
    );
    opts.cancel.onClick();
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("Couldn't reject iPhone: db locked"),
    );
  });

  it("treats the first event as the baseline when the snapshot fails", async () => {
    cmds.webRemoteStatus.mockRejectedValue(new Error("not ready"));
    await mounted();
    const old = sess({ id: "old", name: "Old tablet", approved: false });
    events.cb?.(status([old]));
    expect(toast.info).not.toHaveBeenCalled();

    events.cb?.(status([old, sess({ id: "new", name: "iPhone", approved: false })]));
    expect(toast.info).toHaveBeenCalledTimes(1);
    expect(firstQuestion()[0]).toBe("iPhone wants to connect");
  });

  it("closes open questions and stops listening on unmount", async () => {
    const hook = await mounted();
    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: false })]));
    const [, opts] = firstQuestion();

    hook.unmount();
    expect(toast.dismiss).toHaveBeenCalledWith(opts.id);
    expect(events.unlisten).toHaveBeenCalled();
  });

  it("announces a request that arrives before the first snapshot", async () => {
    let seed: (s: WebRemoteStatus) => void = () => {};
    cmds.webRemoteStatus.mockReturnValue(
      new Promise<WebRemoteStatus>((resolve) => {
        seed = resolve;
      }),
    );
    renderHook(() => useRemotePairingRequests());
    await waitFor(() => expect(events.cb).not.toBeNull());

    events.cb?.(status([sess({ id: "new", name: "iPhone", approved: false })]));
    expect(toast.info).not.toHaveBeenCalled();

    seed(status([]));
    await waitFor(() =>
      expect(toast.info).toHaveBeenCalledWith("iPhone wants to connect", expect.anything()),
    );
  });

  it("does nothing in a remote browser, which cannot approve others", async () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    renderHook(() => useRemotePairingRequests());
    await Promise.resolve();
    expect(cmds.webRemoteStatus).not.toHaveBeenCalled();
    expect(events.cb).toBeNull();
  });
});
