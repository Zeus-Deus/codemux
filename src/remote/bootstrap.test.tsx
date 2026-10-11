/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const shim = vi.hoisted(() => ({
  transports: [] as {
    connect: ReturnType<typeof vi.fn>;
    close: ReturnType<typeof vi.fn>;
    settle: { resolve: () => void; reject: (e: unknown) => void };
  }[],
}));
vi.mock("./shim", () => ({
  installShim: vi.fn(() => {
    const settle = { resolve: () => {}, reject: (_e: unknown) => {} };
    const done = new Promise<void>((resolve, reject) => {
      settle.resolve = resolve;
      settle.reject = reject;
    });
    const transport = { connect: vi.fn(() => done), close: vi.fn(), settle };
    shim.transports.push(transport);
    return { transport };
  }),
}));
vi.mock("./hosted", () => ({ isHostedOrigin: () => false }));
vi.mock("./account-pair", async (importActual) => ({
  ...(await importActual<typeof import("./account-pair")>()),
  fetchServerInfo: vi.fn(() =>
    Promise.resolve({ version: null, accountModeEnabled: false }),
  ),
}));

import {
  APPROVAL_TIMEOUT_MS,
  CONNECT_GRACE_MS,
  ConnectScreen,
  ConnectingView,
  bootstrapRemote,
  pairingNotice,
} from "./bootstrap";
import { loadSession, storeSession } from "./session";

beforeEach(() => {
  shim.transports.length = 0;
  localStorage.clear();
});

afterEach(() => {
  cleanup();
  document.getElementById("codemux-remote-bootstrap")?.remove();
  window.history.replaceState(null, "", "/");
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("ConnectingView", () => {
  it("lets a waiting browser give up and says when this page stops waiting", () => {
    const onCancel = vi.fn();
    render(<ConnectingView host="192.168.1.42:4377" waiting onCancel={onCancel} />);

    expect(screen.getByText("Waiting for approval")).toBeInTheDocument();
    expect(screen.getByText(/stops waiting after 5 minutes/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /use a different code/i }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

describe("pairingNotice", () => {
  it("explains a decline, a timeout and a lost pairing, and stays quiet on cancel", () => {
    expect(pairingNotice("unauthorized", true)).toMatch(/declined/);
    expect(pairingNotice("unauthorized", false)).toMatch(/no longer paired/);
    expect(pairingNotice("timed-out", true)).toMatch(/in time/);
    expect(pairingNotice("cancelled", true)).toBeNull();
  });
});

describe("ConnectScreen", () => {
  it("shows why it is back on the code form", () => {
    render(
      <ConnectScreen
        baseUrl="http://192.168.1.42:4377"
        host="192.168.1.42:4377"
        methods={{ pairingCode: true, account: false }}
        notice="The desktop declined this browser."
        onPaired={() => {}}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The desktop declined this browser.",
    );
  });
});

describe("bootstrapRemote connect loop", () => {
  /** Open the page from a pairing link that the desktop must approve.
   *  `approvedByNow` is what the desktop answers when the browser withdraws. */
  function openPendingPairingLink(approvedByNow = false) {
    window.history.replaceState(null, "", "/#pair=abc");
    const fetchMock = vi.fn((url: string, _init?: RequestInit) =>
      Promise.resolve(
        new Response(
          JSON.stringify(
            url.endsWith("/api/withdraw")
              ? { approved: approvedByNow }
              : { session_id: "s1", session_token: "t1", approved: false },
          ),
          { status: 200 },
        ),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  function withdrawCalls(fetchMock: ReturnType<typeof openPendingPairingLink>) {
    return fetchMock.mock.calls.filter(([url]) => url.endsWith("/api/withdraw"));
  }

  it("cancels a pending request back to an empty code form", async () => {
    const fetchMock = openPendingPairingLink();
    void bootstrapRemote();

    fireEvent.click(await screen.findByRole("button", { name: /use a different code/i }));

    expect(await screen.findByLabelText("Pairing link or code")).toBeInTheDocument();
    expect(shim.transports[0].close).toHaveBeenCalled();
    expect(loadSession()).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    // The desktop is told, so it stops asking about this browser.
    const [[, init]] = withdrawCalls(fetchMock);
    expect(init?.headers).toEqual({ Authorization: "Bearer t1" });
  });

  it("says the desktop declined when the pending request is rejected", async () => {
    openPendingPairingLink();
    void bootstrapRemote();
    await screen.findByText("Waiting for approval");

    act(() => shim.transports[0].settle.reject(new Error("unauthorized")));

    expect(await screen.findByRole("alert")).toHaveTextContent(/declined/);
    expect(loadSession()).toBeNull();
  });

  it("gives up after the approval timeout and says why", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    openPendingPairingLink();
    void bootstrapRemote();
    await screen.findByText("Waiting for approval");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(APPROVAL_TIMEOUT_MS);
    });

    expect(await screen.findByRole("alert")).toHaveTextContent(/in time/);
    expect(shim.transports[0].close).toHaveBeenCalled();
    expect(withdrawCalls(vi.mocked(fetch))).toHaveLength(1);
  });

  it("keeps an approval that lands just before the timeout", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    openPendingPairingLink(true);
    const done = bootstrapRemote();
    await screen.findByText("Waiting for approval");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(APPROVAL_TIMEOUT_MS);
    });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(shim.transports[0].close).not.toHaveBeenCalled();

    // The next ticket poll connects.
    shim.transports[0].settle.resolve();
    await done;
    expect(loadSession()).not.toBeNull();
  });

  it("offers Cancel when a stored session is slow to reconnect", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    storeSession({ sessionId: "s1", sessionToken: "t1" });
    void bootstrapRemote();
    await waitFor(() => expect(shim.transports).toHaveLength(1));
    expect(screen.queryByRole("button", { name: /use a different code/i })).toBeNull();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(CONNECT_GRACE_MS);
    });

    fireEvent.click(await screen.findByRole("button", { name: /use a different code/i }));
    await waitFor(() => expect(loadSession()).toBeNull());
  });

  it("connects quietly with a stored session", async () => {
    storeSession({ sessionId: "s1", sessionToken: "t1" });
    const done = bootstrapRemote();
    await waitFor(() => expect(shim.transports).toHaveLength(1));
    shim.transports[0].settle.resolve();
    await done;

    expect(document.getElementById("codemux-remote-bootstrap")).toBeNull();
    expect(loadSession()).not.toBeNull();
  });
});
