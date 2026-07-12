import { describe, it, expect, vi, beforeEach } from "vitest";

// The command wrappers are thin `invoke` calls; mock the Tauri core so we can
// observe the exact command name + args each wrapper sends over the IPC/WS.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  Channel: class {},
}));

import { invoke } from "@tauri-apps/api/core";
import {
  webRemoteRequestUpdate,
  webRemotePublishUpdateAvailable,
} from "./commands";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockClear();
});

describe("webRemoteRequestUpdate", () => {
  // The web client's "update & restart desktop" button rides this command; the
  // desktop turns it into the `web-remote-update-requested` event.
  it("invokes web_remote_request_update with no args", async () => {
    await webRemoteRequestUpdate();
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_request_update");
  });
});

describe("webRemotePublishUpdateAvailable", () => {
  it("forwards availability + version so web clients can prompt", async () => {
    await webRemotePublishUpdateAvailable(true, "2.0.1");
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_publish_update_available", {
      available: true,
      version: "2.0.1",
    });
  });

  // Clearing must pass a null version so a stale one can't linger server-side.
  it("passes a null version when clearing availability", async () => {
    await webRemotePublishUpdateAvailable(false, null);
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_publish_update_available", {
      available: false,
      version: null,
    });
  });
});
