import { beforeEach, describe, expect, it, vi } from "vitest";

// Exercise the web_remote_* wrappers against a mocked Tauri `invoke` to
// pin the exact command names and (camelCase) argument shapes the backend
// deserializes. These live in `src/tauri/commands.ts`; the test sits in the
// settings lane that owns the surface.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve()),
  Channel: class {},
}));

import { invoke } from "@tauri-apps/api/core";
import {
  webRemoteApproveSession,
  webRemoteCreatePairing,
  webRemoteDisable,
  webRemoteEnable,
  webRemoteListEndpoints,
  webRemoteListSessions,
  webRemoteRejectSession,
  webRemoteRevokeSession,
  webRemoteSetConfig,
  webRemoteStatus,
} from "@/tauri/commands";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => invokeMock.mockReset().mockResolvedValue(undefined));

describe("web_remote_* command wrappers", () => {
  it("maps the argument-free reads/mutators to their command names", () => {
    webRemoteStatus();
    webRemoteEnable();
    webRemoteDisable();
    webRemoteCreatePairing();
    webRemoteListEndpoints();
    webRemoteListSessions();

    expect(invokeMock).toHaveBeenNthCalledWith(1, "web_remote_status");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "web_remote_enable");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "web_remote_disable");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "web_remote_create_pairing");
    expect(invokeMock).toHaveBeenNthCalledWith(5, "web_remote_list_endpoints");
    expect(invokeMock).toHaveBeenNthCalledWith(6, "web_remote_list_sessions");
  });

  it("set_config sends all three fields, defaulting the omitted ones to null", () => {
    webRemoteSetConfig({ port: 5000 });
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_set_config", {
      port: 5000,
      requireApproval: null,
      bindScope: null,
    });

    webRemoteSetConfig({ requireApproval: true });
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_set_config", {
      port: null,
      requireApproval: true,
      bindScope: null,
    });

    webRemoteSetConfig({ bindScope: "tailscale" });
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_set_config", {
      port: null,
      requireApproval: null,
      bindScope: "tailscale",
    });
  });

  it("session mutators pass the sessionId through", () => {
    webRemoteRevokeSession("s-1");
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_revoke_session", {
      sessionId: "s-1",
    });

    webRemoteApproveSession("s-2");
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_approve_session", {
      sessionId: "s-2",
    });

    webRemoteRejectSession("s-3");
    expect(invokeMock).toHaveBeenLastCalledWith("web_remote_reject_session", {
      sessionId: "s-3",
    });
  });
});
