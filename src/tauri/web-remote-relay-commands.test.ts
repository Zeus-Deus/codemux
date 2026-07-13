import { describe, it, expect, vi, beforeEach } from "vitest";

// The command wrappers are thin `invoke` calls; mock the Tauri core so we can
// observe the exact command name + args each wrapper sends over the IPC/WS.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  Channel: class {},
}));

import { invoke } from "@tauri-apps/api/core";
import {
  webRemoteIrohNodeId,
  webRemoteRegistrationStatus,
  webRemoteSetConfig,
} from "./commands";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockClear();
});

describe("webRemoteSetConfig relay toggle", () => {
  it("forwards relayModeEnabled to web_remote_set_config", async () => {
    await webRemoteSetConfig({ relayModeEnabled: true });
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_set_config", {
      port: null,
      requireApproval: null,
      bindScope: null,
      accountModeEnabled: null,
      trustAccountBrowsers: null,
      relayModeEnabled: true,
    });
  });

  it("passes null for relayModeEnabled when omitted (left unchanged)", async () => {
    await webRemoteSetConfig({ port: 4377 });
    expect(mockInvoke).toHaveBeenCalledWith(
      "web_remote_set_config",
      expect.objectContaining({ port: 4377, relayModeEnabled: null }),
    );
  });
});

describe("webRemoteIrohNodeId", () => {
  it("invokes web_remote_iroh_node_id with no args", async () => {
    await webRemoteIrohNodeId();
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_iroh_node_id");
  });
});

describe("webRemoteRegistrationStatus", () => {
  it("invokes web_remote_registration_status with no args", async () => {
    await webRemoteRegistrationStatus();
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("web_remote_registration_status");
  });
});
