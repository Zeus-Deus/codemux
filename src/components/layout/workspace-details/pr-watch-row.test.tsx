/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { PrWatchStatus, WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  status: null as PrWatchStatus | null,
  /** Watches of chats other than thread-1. */
  others: {} as Record<string, PrWatchStatus>,
  start: vi.fn(),
  stop: vi.fn(),
  listener: null as ((payload: { thread_id: string; watch: PrWatchStatus | null }) => void) | null,
  failure: vi.fn(),
  focused: "thread-1",
  epoch: 0,
  /** Set to hold `prWatchStatus` until the test resolves it. */
  deferred: null as null | { resolve: (s: PrWatchStatus | null) => void },
  readFails: false,
  /** Set to hold listener registration until the test releases it. */
  registration: null as null | { release: () => void },
}));

vi.mock("@/tauri/commands", () => ({
  prWatchStatus: vi.fn(
    (thread: string) =>
      new Promise<PrWatchStatus | null>((resolve, reject) => {
        if (mocks.readFails) reject("bridge unavailable");
        else if (mocks.deferred) mocks.deferred.resolve = resolve;
        else resolve(thread === "thread-1" ? mocks.status : (mocks.others[thread] ?? null));
      }),
  ),
  prWatchStart: mocks.start,
  prWatchStop: mocks.stop,
}));
vi.mock("@/tauri/events", () => ({
  onPrWatchChanged: vi.fn(async (cb: typeof mocks.listener) => {
    mocks.listener = cb;
    if (mocks.registration) {
      await new Promise<void>((release) => {
        if (mocks.registration) mocks.registration.release = release;
      });
    }
    return () => {
      mocks.listener = null;
    };
  }),
}));
vi.mock("@/lib/toast", () => ({ toast: { failure: mocks.failure } }));
vi.mock("@/hooks/use-active-chat-tasks", () => ({
  useActiveChatTasks: () => ({ threadId: mocks.focused, tasks: null }),
}));
vi.mock("@/remote/remote-connection-store", () => ({
  useRemoteReconnectEpoch: () => mocks.epoch,
}));

import { PrWatchChip } from "@/components/chat/PrWatchChip";
import { PrWatchRow } from "./pr-watch-row";

function watch(patch: Partial<PrWatchStatus> = {}): PrWatchStatus {
  return {
    thread_id: "thread-1",
    workspace_id: "ws",
    pr_number: 172,
    pr_url: "https://github.com/o/r/pull/172",
    status: "active",
    max_attempts: 3,
    attempts: 0,
    comment_streak: 0,
    head_sha: null,
    last_checked_at_ms: null,
    last_wake_at_ms: null,
    last_wake_summary: null,
    last_error: null,
    stop_reason: null,
    created_at_ms: 0,
    updated_at_ms: 0,
    ...patch,
  };
}

const workspace = (patch: Partial<WorkspaceSnapshot> = {}) =>
  ({
    workspace_id: "ws",
    pr_number: 172,
    pr_state: "OPEN",
    host_id: null,
    provider_kind: "github",
    surfaces: [],
    active_surface_id: "",
    ...patch,
  }) as unknown as WorkspaceSnapshot;

beforeEach(() => {
  mocks.status = null;
  mocks.others = {};
  mocks.focused = "thread-1";
  mocks.epoch = 0;
  mocks.deferred = null;
  mocks.readFails = false;
  mocks.registration = null;
  mocks.start.mockReset().mockImplementation(async () => watch());
  mocks.stop.mockReset().mockResolvedValue(true);
  mocks.failure.mockReset();
});
afterEach(cleanup);

describe("PrWatchRow", () => {
  it("keeps Stop for another chat's active watch without focusing that chat", async () => {
    mocks.others["thread-2"] = watch({ thread_id: "thread-2", pr_number: 99 });
    const split = workspace({
      surfaces: [
        {
          surface_id: "s",
          root: {
            kind: "split",
            children: [
              { kind: "agent_chat", thread_id: "thread-1" },
              { kind: "agent_chat", thread_id: "thread-2" },
            ],
          },
        },
      ],
      active_surface_id: "s",
    } as unknown as Partial<WorkspaceSnapshot>);
    render(<PrWatchRow workspace={split} />);
    const other = await screen.findByTestId("workspace-details-pr-watch-other");
    expect(other).toHaveTextContent("Watch #99 · other chat");
    // The focused chat's own row is still there to start a watch.
    expect(screen.getByTestId("workspace-details-pr-watch")).toHaveTextContent("Off");
    fireEvent.click(other);
    await waitFor(() => expect(mocks.stop).toHaveBeenCalledWith("thread-2"));
  });

  it("starts watching the workspace's pull request for its chat", async () => {
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    expect(row).toHaveTextContent("Off");
    fireEvent.click(row);
    await waitFor(() => expect(row).toHaveTextContent("On"));
    expect(mocks.start).toHaveBeenCalledWith("thread-1", 172);
    expect(row).toHaveAttribute("aria-pressed", "true");
  });

  it("stops an active watch and follows backend changes", async () => {
    mocks.status = watch();
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("On"));

    act(() => mocks.listener?.({ thread_id: "thread-1", watch: watch({ attempts: 1 }) }));
    expect(row).toHaveTextContent("On · 1/3");
    act(() => mocks.listener?.({ thread_id: "other", watch: null }));
    expect(row).toHaveTextContent("On · 1/3");

    fireEvent.click(row);
    await waitFor(() => expect(row).toHaveTextContent("Off"));
    expect(mocks.stop).toHaveBeenCalledWith("thread-1");
  });

  it("re-arms a stopped watch and reports a failed start", async () => {
    mocks.status = watch({ status: "exhausted", attempts: 3 });
    mocks.start.mockRejectedValueOnce("gh is signed out");
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("Stopped"));
    fireEvent.click(row);
    await waitFor(() =>
      expect(mocks.failure).toHaveBeenCalledWith("Couldn't watch the pull request", "gh is signed out"),
    );
    expect(row).toHaveTextContent("Stopped");
  });

  it("cannot toggle until the status is known", async () => {
    mocks.deferred = { resolve: () => {} };
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    expect(row).toBeDisabled();
    fireEvent.click(row);
    expect(mocks.start).not.toHaveBeenCalled();

    await act(async () => mocks.deferred?.resolve(watch({ attempts: 2 })));
    expect(row).toBeEnabled();
    expect(row).toHaveTextContent("On · 2/3");
  });

  it("drops a late answer for the chat it no longer shows", async () => {
    let finish: (s: PrWatchStatus) => void = () => {};
    mocks.start.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    const { rerender } = render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toBeEnabled());
    fireEvent.click(row);

    mocks.focused = "thread-2";
    rerender(<PrWatchRow workspace={workspace({ title: "switched" })} />);
    await waitFor(() => expect(row).toBeEnabled());
    await act(async () => finish(watch()));
    expect(row).toHaveTextContent("Off");
    act(() => mocks.listener?.({ thread_id: "thread-1", watch: watch() }));
    expect(row).toHaveTextContent("Off");
  });

  it("rereads the status after a remote reconnect", async () => {
    mocks.status = watch();
    const { rerender } = render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("On"));

    // Stopped elsewhere while this client was away: no event arrives.
    mocks.status = watch({ status: "exhausted" });
    mocks.epoch = 1;
    rerender(<PrWatchRow workspace={workspace({ title: "reconnected" })} />);
    await waitFor(() => expect(row).toHaveTextContent("Stopped"));
  });

  it("reads the status only once it is listening", async () => {
    const { prWatchStatus } = await import("@/tauri/commands");
    vi.mocked(prWatchStatus).mockClear();
    mocks.registration = { release: () => {} };
    render(<PrWatchRow workspace={workspace()} />);
    await waitFor(() => expect(mocks.listener).not.toBeNull());
    expect(prWatchStatus).not.toHaveBeenCalled();
    await act(async () => mocks.registration?.release());
    await waitFor(() => expect(prWatchStatus).toHaveBeenCalledWith("thread-1"));
  });

  it("keeps a newer event over a slower reply", async () => {
    let finish: () => void = () => {};
    mocks.status = watch();
    mocks.stop.mockImplementationOnce(() => new Promise<boolean>((resolve) => (finish = () => resolve(true))));
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("On"));
    fireEvent.click(row);
    // Another client re-arms before this stop's reply lands.
    act(() => mocks.listener?.({ thread_id: "thread-1", watch: watch({ attempts: 1 }) }));
    await act(async () => finish());
    expect(row).toHaveTextContent("On · 1/3");
  });

  it("offers a retry when the status cannot be read", async () => {
    mocks.readFails = true;
    mocks.status = watch({ attempts: 2 });
    render(<PrWatchRow workspace={workspace()} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("Retry"));
    expect(row).toBeEnabled();

    mocks.readFails = false;
    fireEvent.click(row);
    await waitFor(() => expect(row).toHaveTextContent("On · 2/3"));
    expect(mocks.start).not.toHaveBeenCalled();
  });

  it("keeps an active watch stoppable after the workspace loses its PR", async () => {
    mocks.status = watch({ attempts: 1 });
    render(<PrWatchRow workspace={workspace({ pr_number: null, pr_state: null })} />);
    const row = await screen.findByTestId("workspace-details-pr-watch");
    await waitFor(() => expect(row).toHaveTextContent("On · 1/3"));
    fireEvent.click(row);
    await waitFor(() => expect(mocks.stop).toHaveBeenCalledWith("thread-1"));
    await waitFor(() => expect(row).not.toBeInTheDocument());
  });

  it("is absent without an open pull request on GitHub", () => {
    const { container, rerender } = render(<PrWatchRow workspace={workspace({ pr_state: "MERGED" })} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<PrWatchRow workspace={workspace({ provider_kind: "gitlab" })} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("PrWatchChip", () => {
  it("shows only while the chat has a watch", async () => {
    const { container } = render(<PrWatchChip threadId="thread-1" />);
    await waitFor(() => expect(mocks.listener).not.toBeNull());
    expect(container).toBeEmptyDOMElement();

    act(() => mocks.listener?.({ thread_id: "thread-1", watch: watch({ attempts: 2 }) }));
    const chip = screen.getByTestId("pr-watch-chip");
    expect(chip).toHaveTextContent("2/3");
    expect(chip).toHaveAccessibleName("Stop watching #172");

    act(() => mocks.listener?.({ thread_id: "thread-1", watch: watch({ status: "exhausted" }) }));
    expect(chip).toHaveTextContent("Stopped");
    expect(chip).toHaveAccessibleName("Watch #172 again");
  });
});
