import { beforeEach, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { listen, type Event } from "@tauri-apps/api/event";
import * as events from "./events";
import * as commands from "./commands";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(invoke).mockResolvedValue(undefined);
  (window as unknown as { __CODEMUX_REMOTE__: boolean }).__CODEMUX_REMOTE__ = false;
});

it("reads the sanitized connector status through the exact native command", async () => {
  const status = { enabled: false, pending: [], clients: [] };
  vi.mocked(invoke).mockResolvedValueOnce(status);
  expect(await commands.agentConnectorStatus()).toBe(status);
  expect(invoke).toHaveBeenCalledWith("agent_connector_status");
});

it("writes connector configuration with explicit enabled and nullable public origin", async () => {
  await commands.agentConnectorSetConfig({ enabled: true, publicOrigin: null });
  expect(invoke).toHaveBeenCalledWith("agent_connector_set_config", { enabled: true, publicOrigin: null });
});

it("sends port only when given", async () => {
  await commands.agentConnectorSetConfig({ enabled: true, publicOrigin: null, port: 5000 });
  expect(invoke).toHaveBeenLastCalledWith("agent_connector_set_config", { enabled: true, publicOrigin: null, port: 5000 });
  await commands.agentConnectorSetConfig({ enabled: false, publicOrigin: "https://box.example.test", port: undefined });
  expect(invoke).toHaveBeenLastCalledWith("agent_connector_set_config", { enabled: false, publicOrigin: "https://box.example.test" });
});

it("issues a token with the exact label and access", async () => {
  const issued = { token: "synthetic-not-a-credential", clientId: "cmcp_token_1", label: "hermes-bot", access: "supervised" };
  vi.mocked(invoke).mockResolvedValueOnce(issued);
  expect(await commands.agentConnectorIssue({ label: "hermes-bot", access: "supervised" })).toBe(issued);
  expect(invoke).toHaveBeenCalledWith("agent_connector_issue", { label: "hermes-bot", access: "supervised" });
  await commands.agentConnectorIssue({ label: "ci-bot", access: "read_only", expiresInHours: 48 });
  expect(invoke).toHaveBeenLastCalledWith("agent_connector_issue", { label: "ci-bot", access: "read_only", expiresInHours: 48 });
});

it("revokes every grant through the exact native command", async () => {
  await commands.agentConnectorRevokeAll();
  expect(invoke).toHaveBeenCalledWith("agent_connector_revoke_all");
});

it.each(["read_only", "supervised", "full_access"] as const)("approves the exact request with %s access", async (access) => {
  await commands.agentConnectorApprove("request-exact", access);
  expect(invoke).toHaveBeenCalledWith("agent_connector_approve", { requestId: "request-exact", access });
});

it("denies the exact pending request", async () => {
  await commands.agentConnectorDeny("request-exact");
  expect(invoke).toHaveBeenCalledWith("agent_connector_deny", { requestId: "request-exact" });
});

it("uses connector change events only as a sanitized reload signal", async () => {
  const cb = vi.fn();
  await events.onAgentConnectorChanged(cb);
  expect(listen).toHaveBeenCalledWith("agent-connector-changed", expect.any(Function));
  const handler = vi.mocked(listen).mock.calls[0][1];
  handler({ payload: { enabled: true, accessToken: "never-forward-this" } } as Event<unknown>);
  expect(cb).toHaveBeenCalledWith(null);
});

it.each(["status", "config", "approve", "deny", "revoke", "issue", "revokeAll"])("blocks remote-client %s calls before IPC", async (action) => {
  (window as unknown as { __CODEMUX_REMOTE__: boolean }).__CODEMUX_REMOTE__ = true;
  const call = () => {
    switch (action) {
      case "status": return commands.agentConnectorStatus();
      case "config": return commands.agentConnectorSetConfig({ enabled: true, publicOrigin: null });
      case "approve": return commands.agentConnectorApprove("request-1", "read_only");
      case "deny": return commands.agentConnectorDeny("request-1");
      case "issue": return commands.agentConnectorIssue({ label: "hermes-bot", access: "supervised" });
      case "revokeAll": return commands.agentConnectorRevokeAll();
      default: return commands.agentConnectorRevoke("client-1");
    }
  };
  await expect(call()).rejects.toThrow(/desktop/);
  expect(invoke).not.toHaveBeenCalled();
});

it("revokes the exact granted client", async () => {
  await commands.agentConnectorRevoke("client-exact");
  expect(invoke).toHaveBeenCalledWith("agent_connector_revoke", { clientId: "client-exact" });
});
