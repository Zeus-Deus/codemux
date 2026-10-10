import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentConnectorStatus } from "@/tauri/types";

const api = vi.hoisted(() => ({
  remote: false,
  status: vi.fn(), config: vi.fn(), approve: vi.fn(), deny: vi.fn(), revoke: vi.fn(),
  listen: vi.fn(), copy: vi.fn(), changed: null as (() => void) | null,
  remoteListen: vi.fn(), webRemoteChanged: null as ((event: { payload: unknown }) => void) | null,
}));
vi.mock("@/tauri/commands", () => ({
  agentConnectorStatus: api.status, agentConnectorSetConfig: api.config,
  agentConnectorApprove: api.approve, agentConnectorDeny: api.deny, agentConnectorRevoke: api.revoke,
}));
vi.mock("@/tauri/events", () => ({
  onAgentConnectorChanged: api.listen,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: api.remoteListen }));
vi.mock("@/lib/clipboard", () => ({ copyToClipboard: api.copy, COPY_FAILED_MESSAGE: "Couldn't copy — select and copy manually." }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => api.remote }));

import { OutsideAssistantsSection } from "./outside-assistants-section";

const baseline: AgentConnectorStatus = {
  enabled: false, publicOrigin: null, listenerRunning: false, mcpUrl: null,
  localCommand: "/usr/bin/codemux mcp", pending: [], clients: [],
};
function snapshot(overrides: Partial<AgentConnectorStatus> = {}) {
  return { ...baseline, ...overrides };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.clearAllMocks();
  api.remote = false;
  api.copy.mockReset().mockResolvedValue(true);
  api.changed = null;
  api.webRemoteChanged = null;
  api.status.mockReset().mockResolvedValue(snapshot());
  for (const fn of [api.config, api.approve, api.deny, api.revoke]) fn.mockReset().mockResolvedValue(undefined);
  api.listen.mockReset().mockImplementation(async (cb: () => void) => { api.changed = cb; return vi.fn(); });
  api.remoteListen.mockReset().mockImplementation(async (_event: string, cb: (event: { payload: unknown }) => void) => { api.webRemoteChanged = cb; return vi.fn(); });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

it.each(["awaiting_approval", "approved_awaiting_client", "denied", "grant"] as const)("authoritatively refreshes idle %s expiry and locks stale actions during read-back", async (phase) => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-08T12:00:00Z"));
  const expiresAt = new Date(Date.now() + (phase === "grant" ? 3_600_000 : 300_000)).toISOString();
  const pending = phase === "grant" ? [] : [{ ...request, requestedAt: new Date().toISOString(), expiresAt,
    ...(phase === "approved_awaiting_client" ? { phase, access: "read_only" as const } : { phase, access: null }) }];
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending,
    clients: phase === "grant" ? [{ ...granted, expiresAt }] : [] }));
  render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(api.status).toHaveBeenCalledTimes(1);
  const read = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(read.promise);
  await act(async () => vi.advanceTimersByTimeAsync(phase === "grant" ? 3_600_000 : 300_000));
  expect(api.status).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("switch", { name: "Enable HTTP MCP connections" })).toBeDisabled();
  if (phase === "awaiting_approval") {
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(api.approve).not.toHaveBeenCalled();
  }
  await act(async () => read.resolve(snapshot({ enabled: true, listenerRunning: true })));
  expect(screen.queryByRole("group")).not.toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Enable HTTP MCP connections" })).not.toBeDisabled();
});

it.each(["focus", "visibility", "overdue click"] as const)("revalidates after %s without trusting stale action or clearing subscription errors", async (trigger) => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-08T12:00:00Z"));
  api.listen.mockRejectedValue(new Error("synthetic subscription unavailable"));
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [{ ...request,
    expiresAt: new Date(Date.now() + 300_000).toISOString() }] }));
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  const read = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(read.promise);
  vi.setSystemTime(new Date("2026-10-08T12:05:00Z")); // browser timers were suspended
  act(() => {
    if (trigger === "focus") fireEvent.focus(window);
    else if (trigger === "visibility") fireEvent(document, new Event("visibilitychange"));
    else fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  });
  expect(api.status).toHaveBeenCalledTimes(2);
  expect(api.approve).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  await act(async () => read.resolve(snapshot()));
  expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent(/Live connection updates are unavailable/);
  view.unmount();
  act(() => { fireEvent.focus(window); fireEvent(document, new Event("visibilitychange")); });
  await act(async () => vi.advanceTimersByTimeAsync(3_600_000));
  expect(api.status).toHaveBeenCalledTimes(2);
});

it("fences an expiry during mutation read-back and performs a trailing authoritative read", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-08T12:00:00Z"));
  const expiring = { ...request, expiresAt: new Date(Date.now() + 300_000).toISOString() };
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [expiring] }));
  render(<OutsideAssistantsSection />);
  await act(async () => {});
  const stale = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(stale.promise).mockResolvedValueOnce(snapshot());
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await act(async () => {});
  expect(api.status).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(300_000));
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  await act(async () => stale.resolve(snapshot({ enabled: true, listenerRunning: true, pending: [{ ...expiring, phase: "approved_awaiting_client", access: "full_access" }] })));
  expect(api.status).toHaveBeenCalledTimes(3);
  expect(screen.queryByRole("group")).not.toBeInTheDocument();
  expect(api.approve).toHaveBeenCalledTimes(1);
});

it("hides every inbound privileged control from remote clients without IPC", async () => {
  api.remote = true;
  render(<OutsideAssistantsSection />);
  expect(screen.getByText("Manage connections from the desktop")).toBeInTheDocument();
  expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  expect(screen.queryByText(baseline.localCommand)).not.toBeInTheDocument();
  await act(async () => {});
  expect(api.status).not.toHaveBeenCalled();
  expect(api.listen).not.toHaveBeenCalled();
  expect(api.remoteListen).not.toHaveBeenCalled();
});

it("copies the backend-supplied local STDIO command while HTTP is disabled", async () => {
  api.status.mockResolvedValue(snapshot({ localCommand: "/opt/codemux mcp" }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy local command" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith("/opt/codemux mcp"));
  expect(await screen.findByText("Copied")).toBeInTheDocument();
});

it("explains listener-offline and routes to existing Remote Access without starting it", async () => {
  const { useUIStore } = await import("@/stores/ui-store");
  api.status.mockResolvedValue(snapshot({ enabled: true, mcpUrl: "http://127.0.0.1:4377/mcp" }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Open Remote Access" }));
  expect(useUIStore.getState().settingsSection).toBe("remote_access");
  expect(screen.getByText(/listener is offline/i)).toBeInTheDocument();
  expect(screen.queryByText("http://127.0.0.1:4377/mcp")).not.toBeInTheDocument();
  expect(screen.getByText(/iroh relay does not support hosted HTTP MCP/i)).toBeInTheDocument();
  expect(api.config).not.toHaveBeenCalled();
});

it("verifies HTTP enablement by read-back, not the write response", async () => {
  const write = deferred<AgentConnectorStatus>();
  api.config.mockReturnValue(write.promise);
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Enable HTTP MCP connections" });
  fireEvent.click(toggle);
  expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: null });
  expect(toggle).not.toBeChecked();
  expect(toggle).toBeDisabled();
  api.status.mockResolvedValue(snapshot({ enabled: false }));
  await act(async () => write.resolve(snapshot({ enabled: true })));
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(toggle).not.toBeChecked();
  expect(api.status).toHaveBeenCalledTimes(2);
});

it.each([
  "http://example.test", "https://u:p@example.test", "https://example.test/path",
  "https://example.test?x=1", "https://example.test#fragment", "https://example.test?",
  "https://example.test#", "https://example.test/../", "https://example.test\\\\path", "not an origin",
  " https://example.test", "https://example.test ",
])("rejects non-origin HTTPS configuration: %s", async (origin) => {
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Public HTTPS origin" }), { target: { value: origin } });
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/HTTPS origin only/i);
  expect(api.config).not.toHaveBeenCalled();
});

it.each([31, 0, 1, 127])("rejects a retained raw origin control character %i before IPC", async (code) => {
  const origin = "https://connector.example.test" + String.fromCharCode(code);
  render(<OutsideAssistantsSection />);
  const input = await screen.findByRole("textbox", { name: "Public HTTPS origin" });
  fireEvent.change(input, { target: { value: origin } });
  expect(input).toHaveValue(origin);
  expect((input as HTMLInputElement).value.charCodeAt(origin.length - 1)).toBe(code);
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  expect(api.config).not.toHaveBeenCalled();
  expect(await screen.findByRole("alert")).toHaveTextContent(/HTTPS origin only/i);
});

it("saves a public HTTPS origin without claiming reverse-proxy reachability", async () => {
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Public HTTPS origin" }), { target: { value: "https://connector.example.test" } });
  api.status.mockResolvedValue(snapshot({ publicOrigin: "https://connector.example.test" }));
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  await waitFor(() => expect(api.config).toHaveBeenCalledWith({ enabled: false, publicOrigin: "https://connector.example.test" }));
  expect(screen.getByText(/reachable HTTPS reverse proxy/i)).toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole("button", { name: "Save origin" })).not.toBeDisabled());
  expect(screen.queryByRole("button", { name: "Copy MCP URL" })).not.toBeInTheDocument();
});

it("shows a copyable MCP URL only when enabled and the listener is running", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, mcpUrl: "https://connector.example.test/mcp" }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy MCP URL" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith("https://connector.example.test/mcp"));
});

it("keeps the verified state after a failed write and does not expose raw errors", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true }));
  api.config.mockRejectedValue(new Error("token=never-render-this"));
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Enable HTTP MCP connections" });
  fireEvent.click(toggle);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to update/i);
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(toggle).toBeChecked();
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});

it("keeps an unsaved public origin when only HTTP enablement is changed", async () => {
  render(<OutsideAssistantsSection />);
  const input = await screen.findByRole("textbox", { name: "Public HTTPS origin" });
  fireEvent.change(input, { target: { value: "https://draft.example.test" } });
  fireEvent.click(screen.getByRole("switch", { name: "Enable HTTP MCP connections" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  expect(input).toHaveValue("https://draft.example.test");
  expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: null });
});

it("does not show old clipboard success for a changed setup command", async () => {
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy local command" }));
  expect(await screen.findByText("Copied")).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ localCommand: "/opt/new-codemux mcp" }));
  await act(async () => api.changed?.());
  expect(screen.getByText("/opt/new-codemux mcp")).toBeInTheDocument();
  expect(screen.queryByText("Copied")).not.toBeInTheDocument();
});

it("reports failed clipboard writes instead of claiming copy success", async () => {
  api.copy.mockResolvedValue(false);
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy local command" }));
  expect(await screen.findByText("Couldn't copy — select and copy manually.")).toBeInTheDocument();
  expect(screen.queryByText("Copied")).not.toBeInTheDocument();
});

it("does not grant a request invalidated by a refresh before React re-renders", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const approve = await screen.findByRole("button", { name: "Approve" });
  const read = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(read.promise);
  act(() => { api.changed?.(); fireEvent.click(approve); });
  expect(api.approve).not.toHaveBeenCalled();
  expect(approve).toBeDisabled();
  await act(async () => read.resolve(snapshot()));
  expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
});

it("guides the assistant OAuth request back to desktop approval", async () => {
  render(<OutsideAssistantsSection />);
  await screen.findByText(baseline.localCommand);
  expect(screen.getByText(/Add the MCP URL in your assistant/i)).toHaveTextContent(/start.*OAuth.*approve.*desktop.*Continue.*browser/i);
});

it("reloads on change events and ignores an older completion", async () => {
  render(<OutsideAssistantsSection />);
  await screen.findByText(baseline.localCommand);
  const older = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(older.promise).mockResolvedValueOnce(snapshot({ enabled: true, listenerRunning: true }));
  await act(async () => api.changed?.());
  await act(async () => api.changed?.());
  expect(screen.getByRole("switch", { name: "Enable HTTP MCP connections" })).toBeChecked();
  await act(async () => older.resolve(snapshot({ enabled: false })));
  expect(screen.getByRole("switch", { name: "Enable HTTP MCP connections" })).toBeChecked();
});

it("refreshes successfully after status failure without leaking the raw message", async () => {
  api.status.mockRejectedValueOnce(new Error("client_secret=never-render-this"));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to verify/);
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  expect(await screen.findByText(baseline.localCommand)).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it.each(["connector", "listener"])("keeps a failed %s live subscription warning across successful refresh and mutation", async (subscription) => {
  (subscription === "connector" ? api.listen : api.remoteListen).mockRejectedValue(new Error("token=never-render-this"));
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Enable HTTP MCP connections" });
  expect(screen.getByRole("alert")).toHaveTextContent(/Live connection updates are unavailable/);
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("alert")).toHaveTextContent(/Live connection updates are unavailable/);
  api.status.mockResolvedValue(snapshot({ enabled: true }));
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: null });
  expect(toggle).toBeChecked();
  expect(screen.getByRole("alert")).toHaveTextContent(/Live connection updates are unavailable/);
  expect(api.listen).toHaveBeenCalledTimes(1);
  expect(api.remoteListen).toHaveBeenCalledTimes(1);
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});

it("preserves an edited public origin while refreshing server state", async () => {
  render(<OutsideAssistantsSection />);
  const input = await screen.findByRole("textbox", { name: "Public HTTPS origin" });
  fireEvent.change(input, { target: { value: "https://draft.example.test" } });
  api.status.mockResolvedValue(snapshot({ enabled: true, publicOrigin: "https://saved.example.test" }));
  await act(async () => api.changed?.());
  expect(screen.getByRole("switch", { name: "Enable HTTP MCP connections" })).toBeChecked();
  expect(input).toHaveValue("https://draft.example.test");
});

it("reloads on actual web-remote listener rebind, disable and enable events without trusting payloads", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, mcpUrl: "http://127.0.0.1:4377/mcp", pending: [request] }));
  render(<OutsideAssistantsSection />);
  await screen.findByText("http://127.0.0.1:4377/mcp");
  expect(api.remoteListen).toHaveBeenCalledWith("web-remote-state-changed", expect.any(Function));
  const rebind = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(rebind.promise);
  await act(async () => api.webRemoteChanged?.({ payload: { running: true, port: 9999, registration_error: "token=never-render-this" } }));
  expect(screen.queryByRole("button", { name: "Copy MCP URL" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  await act(async () => rebind.resolve(snapshot({ enabled: true, listenerRunning: true, mcpUrl: "http://127.0.0.1:5432/mcp", pending: [request] })));
  expect(screen.getByText("http://127.0.0.1:5432/mcp")).toBeInTheDocument();
  expect(screen.queryByText(/9999|never-render-this/)).not.toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ enabled: true, pending: [request] }));
  await act(async () => api.webRemoteChanged?.({ payload: { running: true, port: 9999 } }));
  expect(screen.getByText(/listener is offline/i)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy MCP URL" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, mcpUrl: "http://127.0.0.1:5432/mcp", pending: [request] }));
  await act(async () => api.webRemoteChanged?.({ payload: { running: false } }));
  expect(screen.getByRole("button", { name: "Copy MCP URL" })).not.toBeDisabled();
  expect(screen.getByRole("button", { name: "Approve" })).not.toBeDisabled();
  expect(api.status).toHaveBeenCalledTimes(4);
});

it.each(["connector", "listener"])("waits for the %s subscription before its initial read", async (subscription) => {
  const pending = deferred<() => void>();
  (subscription === "connector" ? api.listen : api.remoteListen).mockReturnValue(pending.promise);
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(api.status).not.toHaveBeenCalled();
  await act(async () => pending.resolve(vi.fn()));
  expect(await screen.findByText(baseline.localCommand)).toBeInTheDocument();
  expect(api.status).toHaveBeenCalledTimes(1);
  view.unmount();
});

it("disposes both active subscriptions and ignores callbacks after unmount", async () => {
  const connectorDispose = vi.fn();
  const listenerDispose = vi.fn();
  api.listen.mockImplementation(async (cb: () => void) => { api.changed = cb; return connectorDispose; });
  api.remoteListen.mockImplementation(async (_event: string, cb: (event: { payload: unknown }) => void) => { api.webRemoteChanged = cb; return listenerDispose; });
  const view = render(<OutsideAssistantsSection />);
  await screen.findByText(baseline.localCommand);
  view.unmount();
  expect(connectorDispose).toHaveBeenCalledTimes(1);
  expect(listenerDispose).toHaveBeenCalledTimes(1);
  await act(async () => { api.changed?.(); api.webRemoteChanged?.({ payload: { token: "never-render-this" } }); });
  expect(api.status).toHaveBeenCalledTimes(1);
});

it.each(["connector", "listener"])("disposes a late %s subscription along with the installed peer", async (subscription) => {
  const pending = deferred<() => void>();
  const lateDispose = vi.fn();
  const peerDispose = vi.fn();
  (subscription === "connector" ? api.listen : api.remoteListen).mockReturnValue(pending.promise);
  (subscription === "connector" ? api.remoteListen : api.listen).mockResolvedValue(peerDispose);
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  view.unmount();
  expect(peerDispose).toHaveBeenCalledTimes(1);
  await act(async () => pending.resolve(lateDispose));
  expect(lateDispose).toHaveBeenCalledTimes(1);
  expect(api.status).not.toHaveBeenCalled();
});

it("disposes a late event subscription after unmount", async () => {
  const subscription = deferred<() => void>();
  const unlisten = vi.fn();
  api.listen.mockReturnValue(subscription.promise);
  const view = render(<OutsideAssistantsSection />);
  view.unmount();
  await act(async () => subscription.resolve(unlisten));
  expect(unlisten).toHaveBeenCalledTimes(1);
});

it.each(["connector", "listener"])("re-reads a %s event during mutation read-back instead of publishing stale state", async (subscription) => {
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Enable HTTP MCP connections" });
  const older = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(older.promise).mockResolvedValueOnce(snapshot({ enabled: true }));
  fireEvent.click(toggle);
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  await act(async () => {
    if (subscription === "connector") api.changed?.();
    else api.webRemoteChanged?.({ payload: { token: "never-render-this" } });
  });
  await act(async () => older.resolve(snapshot({ enabled: false })));
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(toggle).toBeChecked();
  expect(api.status).toHaveBeenCalledTimes(3);
});

it.each(["connector", "listener"])("ignores a late %s subscription failure after unmount", async (subscription) => {
  const pending = deferred<() => void>();
  const peerDispose = vi.fn();
  (subscription === "connector" ? api.listen : api.remoteListen).mockReturnValue(pending.promise);
  (subscription === "connector" ? api.remoteListen : api.listen).mockResolvedValue(peerDispose);
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  view.unmount();
  await act(async () => pending.reject(new Error("token=never-render-this")));
  expect(peerDispose).toHaveBeenCalledTimes(1);
  expect(api.status).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it.each(["connector", "listener"])("disposes two late subscriptions with %s completing first", async (first) => {
  const connector = deferred<() => void>();
  const listener = deferred<() => void>();
  const connectorDispose = vi.fn();
  const listenerDispose = vi.fn();
  api.listen.mockReturnValue(connector.promise);
  api.remoteListen.mockReturnValue(listener.promise);
  const view = render(<OutsideAssistantsSection />);
  view.unmount();
  await act(async () => {
    if (first === "connector") connector.resolve(connectorDispose);
    else listener.resolve(listenerDispose);
  });
  await act(async () => {
    if (first === "connector") listener.resolve(listenerDispose);
    else connector.resolve(connectorDispose);
  });
  expect(connectorDispose).toHaveBeenCalledTimes(1);
  expect(listenerDispose).toHaveBeenCalledTimes(1);
  expect(api.status).not.toHaveBeenCalled();
});

it("locks privileged actions until a failed config read-back is refreshed", async () => {
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Enable HTTP MCP connections" });
  api.status.mockRejectedValueOnce(new Error("secret=never-render-this"));
  fireEvent.click(toggle);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to verify/);
  expect(toggle).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  await waitFor(() => expect(toggle).not.toBeDisabled());
});

const request = { id: "request-1", clientName: "<img src=x onerror=evil()> assistant", callbackOrigin: "https://assistant.example.test", requestedAt: "2026-10-08T00:00:00Z", expiresAt: new Date(Date.now() + 300_000).toISOString(), phase: "awaiting_approval" as const, access: null };
const granted = { id: "client-1", clientName: "Example assistant", callbackOrigin: "https://assistant.example.test", access: "read_only" as const, createdAt: "2026-10-08T00:00:00Z", expiresAt: new Date(Date.now() + 3_600_000).toISOString() };

it("approves untrusted client names as plain text with read-only access by default", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(request.clientName)).toBeInTheDocument();
  expect(document.querySelector("img")).toBeNull();
  expect(screen.getByText(/Registered callback origin/)).toHaveTextContent(request.callbackOrigin);
  expect(screen.queryByText(/Verified callback/)).not.toBeInTheDocument();
  expect(screen.getByText(/client name and callback origin are self-asserted/i)).toHaveTextContent(/not proof of identity/i);
  expect(screen.getByRole("combobox", { name: "Access for " + request.clientName })).toHaveValue("read_only");
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [{ ...request, phase: "approved_awaiting_client", access: "read_only" }] }));
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(api.approve).toHaveBeenCalledWith("request-1", "read_only"));
  expect(await screen.findByText(/Approved.*Read-only.*waiting for the assistant/)).toBeInTheDocument();
  expect(screen.getByText(request.clientName)).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, clients: [granted] }));
  await act(async () => api.changed?.());
  expect(await screen.findByText("Example assistant")).toBeInTheDocument();
  expect(screen.queryByText(request.clientName)).not.toBeInTheDocument();
});

it.each(["read_only", "supervised", "full_access"] as const)("shows decided native approval awaiting the client without repeated decisions for %s", async (access) => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const select = await screen.findByRole("combobox", { name: "Access for " + request.clientName });
  fireEvent.change(select, { target: { value: access } });
  const approved = { ...request, phase: "approved_awaiting_client" as const, access };
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [approved] }));
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument());
  const group = within(screen.getByRole("group", { name: "Connection request from " + request.clientName }));
  expect(group.queryByRole("button", { name: "Deny" })).not.toBeInTheDocument();
  expect(group.queryByRole("combobox")).not.toBeInTheDocument();
  expect(group.getByText(/Approved.*waiting for the assistant/i)).toHaveTextContent(access === "read_only" ? "Read-only" : access === "supervised" ? "Supervised" : "Full access");
  expect(group.getByText(/Continue.*browser/i)).toBeInTheDocument();
  expect(screen.getByText("No approved assistants.")).toBeInTheDocument();
  expect(api.approve).toHaveBeenCalledTimes(1);
  expect(api.deny).not.toHaveBeenCalled();
});

it.each(["read_only", "supervised", "full_access"] as const)("discloses instance-wide consent before approval for %s", async (access) => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const group = within(await screen.findByRole("group", { name: "Connection request from " + request.clientName }));
  fireEvent.change(group.getByRole("combobox"), { target: { value: access } });
  const scope = group.getByText(/Applies to all CodeMux workspaces and visible conversations, not just this project/i);
  const reads = group.getByText(/visible user and top-level assistant messages, not reasoning or tool payloads/i);
  const approve = group.getByRole("button", { name: "Approve" });
  expect(scope.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(reads.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const authority = group.getByText(access === "read_only" ? /cannot launch workers, send messages or change state/i
    : access === "supervised" ? /some modes allow file edits/i : /with your account’s permissions/i);
  expect(authority.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  if (access === "supervised") {
    expect(authority).toHaveTextContent(/You handle approval requests in CodeMux/i);
    expect(authority).toHaveTextContent(/cannot approve its own requests/i);
    expect(group.queryByText(/requires human worker permissions for changes/i)).not.toBeInTheDocument();
  }
  if (access === "full_access") {
    expect(authority).toHaveTextContent(/not a project-confined sandbox/i);
    expect(authority).toHaveTextContent(/Approve only an assistant you trust/i);
  }
  expect(api.approve).not.toHaveBeenCalled();
});

it.each(["supervised", "full_access"] as const)("explains and explicitly approves %s access", async (access) => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("combobox", { name: "Access for " + request.clientName }), { target: { value: access } });
  expect(screen.getByText(access === "supervised" ? /some modes allow file edits/i : /can run commands/i)).toBeInTheDocument();
  if (access === "supervised") expect(screen.getByText(/cannot approve its own requests/i)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(api.approve).toHaveBeenCalledWith("request-1", access));
});

it("denies pending requests without granting a client", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const button = await screen.findByRole("button", { name: "Deny" });
  api.status.mockResolvedValue(snapshot({ enabled: true, pending: [{ ...request, phase: "denied", access: null }] }));
  fireEvent.click(button);
  await waitFor(() => expect(api.deny).toHaveBeenCalledWith("request-1"));
  expect(await screen.findByText(/Denied.*no access was granted/i)).toBeInTheDocument();
  expect(screen.getByText(request.clientName)).toBeInTheDocument();
  expect(api.approve).not.toHaveBeenCalled();
});

it("keeps offline denial actionable and shows the retained denied native phase without repeat actions", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: false, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const deny = await screen.findByRole("button", { name: "Deny" });
  expect(deny).not.toBeDisabled();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "full_access" } });
  const denied = { ...request, phase: "denied" as const, access: null };
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: false, pending: [denied] }));
  fireEvent.click(deny);
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  const group = within(screen.getByRole("group", { name: "Connection request from " + request.clientName }));
  expect(group.getByText(/Denied.*no access was granted/i)).toBeInTheDocument();
  expect(group.queryByText(/Full access can run commands/i)).not.toBeInTheDocument();
  expect(group.queryByRole("button")).not.toBeInTheDocument();
  expect(group.queryByRole("combobox")).not.toBeInTheDocument();
  expect(api.deny).toHaveBeenCalledTimes(1);
  expect(api.approve).not.toHaveBeenCalled();
  expect(screen.getByText("No approved assistants.")).toBeInTheDocument();
});

it("confirms revocation and prevents duplicate requests until read-back completes", async () => {
  const write = deferred<void>();
  const read = deferred<AgentConnectorStatus>();
  api.revoke.mockReturnValue(write.promise);
  api.status.mockResolvedValue(snapshot({ clients: [granted] }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Revoke" }));
  expect(api.revoke).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(api.revoke).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
  const confirm = screen.getByRole("button", { name: "Revoke access" });
  fireEvent.click(confirm);
  fireEvent.click(confirm);
  expect(api.revoke).toHaveBeenCalledTimes(1);
  expect(api.revoke).toHaveBeenCalledWith("client-1");
  expect(confirm).toBeDisabled();
  api.status.mockReturnValue(read.promise);
  await act(async () => write.resolve());
  expect(confirm).toBeDisabled();
  expect(screen.getByText("Example assistant")).toBeInTheDocument();
  await act(async () => read.resolve(snapshot()));
  await waitFor(() => expect(screen.queryByText("Example assistant")).not.toBeInTheDocument());
});

it.each(["approve", "deny", "revoke"] as const)("keeps records and offers retry after failed %s", async (action) => {
  api[action].mockRejectedValue(new Error("authorization_code=never-render-this"));
  api.status.mockResolvedValue(snapshot({ enabled: true, listenerRunning: true, pending: [request], clients: [granted] }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: action === "approve" ? "Approve" : action === "deny" ? "Deny" : "Revoke" }));
  if (action === "revoke") fireEvent.click(screen.getByRole("button", { name: "Revoke access" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to/);
  await waitFor(() => expect(screen.getByRole("button", { name: action === "revoke" ? "Revoke access" : action === "approve" ? "Approve" : "Deny" })).not.toBeDisabled());
  expect(screen.getByText(request.clientName)).toBeInTheDocument();
  expect(screen.getByText("Example assistant")).toBeInTheDocument();
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});
