import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

const { listen, unlisten, openNotificationTarget, remote } = vi.hoisted(() => ({
  listen: vi.fn(),
  unlisten: vi.fn(),
  openNotificationTarget: vi.fn(),
  remote: { value: false },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@/lib/open-notification-target", () => ({ openNotificationTarget }));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => remote.value,
}));

import {
  NOTIFICATION_ACTIVATE_EVENT,
  useNotificationActivate,
} from "./use-notification-activate";

describe("useNotificationActivate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    remote.value = false;
    listen.mockResolvedValue(unlisten);
  });
  afterEach(cleanup);

  it("opens the clicked notification's agent on desktop", async () => {
    const { unmount } = renderHook(() => useNotificationActivate());

    expect(listen).toHaveBeenCalledWith(
      NOTIFICATION_ACTIVATE_EVENT,
      expect.any(Function),
    );
    const handler = listen.mock.calls[0][1];
    const payload = { workspace_id: "ws-1", pane_id: "pane-7" };
    handler({ payload });
    expect(openNotificationTarget).toHaveBeenCalledWith(payload);

    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());
  });

  it("registers nothing on a remote client", () => {
    remote.value = true;
    renderHook(() => useNotificationActivate());
    expect(listen).not.toHaveBeenCalled();
  });
});
