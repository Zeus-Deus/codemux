/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { PrWatchStatus, WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  status: null as PrWatchStatus | null,
  start: vi.fn(),
  stop: vi.fn(),
  listener: null as ((payload: { thread_id: string; watch: PrWatchStatus | null }) => void) | null,
  failure: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  prWatchStatus: vi.fn(async () => mocks.status),
  prWatchStart: mocks.start,
  prWatchStop: mocks.stop,
}));
vi.mock("@/tauri/events", () => ({
  onPrWatchChanged: vi.fn(async (cb: typeof mocks.listener) => {
    mocks.listener = cb;
    return () => {
      mocks.listener = null;
    };
  }),
}));
vi.mock("@/lib/toast", () => ({ toast: { failure: mocks.failure } }));
vi.mock("@/hooks/use-active-chat-tasks", () => ({
  useActiveChatTasks: () => ({ threadId: "thread-1", tasks: null }),
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
  mocks.start.mockReset().mockImplementation(async () => watch());
  mocks.stop.mockReset().mockResolvedValue(true);
  mocks.failure.mockReset();
});
afterEach(cleanup);

describe("PrWatchRow", () => {
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
