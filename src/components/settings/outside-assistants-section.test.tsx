import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentConnectorStatus } from "@/tauri/types";

const api = vi.hoisted(() => ({
  remote: false,
  status: vi.fn(), config: vi.fn(), approve: vi.fn(), deny: vi.fn(), revoke: vi.fn(), issue: vi.fn(), revokeAll: vi.fn(),
  listen: vi.fn(), copy: vi.fn(), changed: null as (() => void) | null,
  remoteListen: vi.fn(), openUrl: vi.fn(),
  toastSuccess: vi.fn(), toastError: vi.fn(),
}));
vi.mock("@/tauri/commands", () => ({
  agentConnectorStatus: api.status, agentConnectorSetConfig: api.config,
  agentConnectorApprove: api.approve, agentConnectorDeny: api.deny, agentConnectorRevoke: api.revoke,
  agentConnectorIssue: api.issue, agentConnectorRevokeAll: api.revokeAll,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: api.openUrl }));
vi.mock("@/tauri/events", () => ({
  onAgentConnectorChanged: api.listen,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: api.remoteListen }));
vi.mock("@/lib/clipboard", () => ({ copyToClipboard: api.copy, COPY_FAILED_MESSAGE: "Couldn't copy — select and copy manually." }));
vi.mock("@/lib/toast", () => ({ toast: { success: api.toastSuccess, error: api.toastError } }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => api.remote }));

import { OUTSIDE_AGENTS_GUIDE_URL, OutsideAssistantsSection } from "./outside-assistants-section";

const baseline: AgentConnectorStatus = {
  enabled: false, publicOrigin: null, listenerRunning: false, port: 4378, bindError: null, localUrl: null, mcpUrl: null,
  localExecutable: "/usr/bin/codemux", localCommand: "/usr/bin/codemux mcp", pending: [], clients: [],
};
/** Network connections on and the connector's listener bound. */
const LISTENING = { enabled: true, listenerRunning: true, localUrl: "http://127.0.0.1:4378/mcp", mcpUrl: "http://127.0.0.1:4378/mcp" } as const;
const CODEX_COMMAND = "codex mcp add codemux -- /usr/bin/codemux mcp --client codex";
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
  api.status.mockReset().mockResolvedValue(snapshot());
  for (const fn of [api.approve, api.deny, api.revoke, api.issue, api.revokeAll]) fn.mockReset().mockResolvedValue(undefined);
  // Like the native command, a config write answers with a status.
  api.config.mockReset().mockResolvedValue(snapshot());
  api.openUrl.mockReset().mockResolvedValue(undefined);
  api.listen.mockReset().mockImplementation(async (cb: () => void) => { api.changed = cb; return vi.fn(); });
  api.remoteListen.mockReset().mockResolvedValue(vi.fn());
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

it.each(["awaiting_approval", "approved_awaiting_client", "denied", "grant"] as const)("authoritatively refreshes idle %s expiry and locks stale actions during read-back", async (phase) => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-08T12:00:00Z"));
  const expiresAt = new Date(Date.now() + (phase === "grant" ? 3_600_000 : 300_000)).toISOString();
  const pending = phase === "grant" ? [] : [{ ...request, requestedAt: new Date().toISOString(), expiresAt,
    ...(phase === "approved_awaiting_client" ? { phase, access: "read_only" as const } : { phase, access: null }) }];
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending,
    clients: phase === "grant" ? [{ ...granted, expiresAt }] : [] }));
  render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(api.status).toHaveBeenCalledTimes(1);
  const read = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(read.promise);
  await act(async () => vi.advanceTimersByTimeAsync(phase === "grant" ? 3_600_000 : 300_000));
  expect(api.status).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("switch", { name: "Allow network connections" })).toBeDisabled();
  if (phase === "awaiting_approval") {
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(api.approve).not.toHaveBeenCalled();
  }
  await act(async () => read.resolve(snapshot({ ...LISTENING })));
  expect(screen.queryByRole("group")).not.toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Allow network connections" })).not.toBeDisabled();
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
  expect(screen.getByRole("heading", { name: "Outside assistants" })).toBeInTheDocument();
  expect(screen.getByText("Manage connections from the desktop")).toBeInTheDocument();
  expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  expect(screen.queryByText(CODEX_COMMAND)).not.toBeInTheDocument();
  await act(async () => {});
  for (const fn of [api.status, api.listen, api.remoteListen, api.issue, api.revokeAll, api.config]) expect(fn).not.toHaveBeenCalled();
});

it("shows a fresh user only the local rows and the network switch", async () => {
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(CODEX_COMMAND)).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "On this computer" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Over the network" })).toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Allow network connections" })).not.toBeChecked();
  expect(screen.queryByRole("heading", { name: "Assistants with access" })).not.toBeInTheDocument();
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Create token" })).not.toBeInTheDocument();
  expect(screen.queryByText(/Remote Access/)).not.toBeInTheDocument();
  expect(screen.getByText(/only names the assistant on the messages it sends/)).toHaveTextContent(/Hermes starts as Supervised/);
});

it("opens the outside-agent guide", async () => {
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Setup guide" }));
  expect(api.openUrl).toHaveBeenCalledWith(OUTSIDE_AGENTS_GUIDE_URL);
});

it.each([
  ["Copy Claude Code command", "claude mcp add --scope user codemux -- '/opt/My Apps/codemux' mcp --client claude-code"],
  ["Copy Codex command", "codex mcp add codemux -- '/opt/My Apps/codemux' mcp --client codex"],
  ["Copy Hermes command", "hermes mcp add codemux --command '/opt/My Apps/codemux' --args mcp --client hermes --access supervised"],
  ["Copy command", "'/opt/My Apps/codemux' mcp --client my-agent"],
])("copies each client's local setup command: %s", async (button, command) => {
  api.status.mockResolvedValue(snapshot({ localExecutable: "/opt/My Apps/codemux" }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(command)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: button }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith(command));
  await waitFor(() => expect(api.toastSuccess).toHaveBeenCalled());
  // Confirmation lives inside the button; no status row grows the layout.
  expect(screen.queryByText("Copied")).not.toBeInTheDocument();
});

it.each([
  ["omits it", () => { const { localExecutable: _omitted, ...status } = baseline; return status; }],
  ["sends an empty one", () => snapshot({ localExecutable: "" })],
])("falls back to codemux on PATH when the backend %s", async (_case, status) => {
  api.status.mockResolvedValue(status());
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText("codex mcp add codemux -- codemux mcp --client codex")).toBeInTheDocument();
});

it("reports a listener failure and changes the port by read-back", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, bindError: "bind 127.0.0.1:4378: Address already in use" }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText("Couldn’t listen on port 4378")).toBeInTheDocument();
  expect(screen.getByText("bind 127.0.0.1:4378: Address already in use")).toBeInTheDocument();
  expect(screen.getByText(/A new port signs out every assistant that has access/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Open Remote Access" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Create token" })).not.toBeInTheDocument();
  const write = deferred<AgentConnectorStatus>();
  api.config.mockReturnValue(write.promise);
  fireEvent.change(screen.getByRole("textbox", { name: "Port" }), { target: { value: "5000" } });
  fireEvent.click(screen.getByRole("button", { name: "Use port" }));
  expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: null, port: 5000 });
  expect(screen.getByRole("button", { name: "Use port" })).toBeDisabled();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, port: 5000, localUrl: "http://127.0.0.1:5000/mcp", mcpUrl: "http://127.0.0.1:5000/mcp" }));
  // The write's own reply is not trusted: it claims a different port.
  await act(async () => write.resolve(snapshot({ ...LISTENING, port: 6000 })));
  expect(await screen.findByText("Listening on this computer, port 5000")).toBeInTheDocument();
  expect(screen.getByText("http://127.0.0.1:5000/mcp")).toBeInTheDocument();
  expect(screen.getByText("tailscale serve --bg 5000")).toBeInTheDocument();
  expect(screen.queryByText(/port 6000/)).not.toBeInTheDocument();
});

it.each(["80", "70000", "abc", "4378.5"])("rejects port %s with an alert and no IPC", async (port) => {
  api.status.mockResolvedValue(snapshot({ enabled: true, bindError: "bind 127.0.0.1:4378: Address already in use" }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Port" }), { target: { value: port } });
  fireEvent.click(screen.getByRole("button", { name: "Use port" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/port between 1024 and 65535|Enter a number/);
  expect(api.config).not.toHaveBeenCalled();
});

it("retries a failed bind with the current settings", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true, publicOrigin: "https://box.example.test", bindError: "bind 127.0.0.1:4378: Address already in use" }));
  render(<OutsideAssistantsSection />);
  // The saved origin is the issuer, so a new port signs nobody out; only the proxy needs it.
  expect(await screen.findByText(/After a port change, point your HTTPS proxy at the new port/)).toBeInTheDocument();
  expect(screen.queryByText(/signs out every assistant/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: "https://box.example.test" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
});

it("says the listener is starting while it has neither bound nor failed", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText("Starting…")).toBeInTheDocument();
  expect(screen.queryByRole("textbox", { name: "Public HTTPS origin" })).not.toBeInTheDocument();
});

it("shows HTTPS guidance with the tailscale command only while listening without a public origin", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  const view = render(<OutsideAssistantsSection />);
  expect(await screen.findByText("Listening on this computer, port 4378")).toBeInTheDocument();
  expect(screen.getByText(/Only this computer can reach this address/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Copy Tailscale command" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith("tailscale serve --bg 4378"));
  fireEvent.click(screen.getByRole("button", { name: "Copy local MCP URL" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith("http://127.0.0.1:4378/mcp"));
  expect(screen.getByText(/Assistants that support OAuth sign in with this URL/)).toBeInTheDocument();
  view.unmount();
  api.status.mockResolvedValue(snapshot({ enabled: false }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(/^Off\./)).toBeInTheDocument();
  expect(screen.queryByText(/tailscale serve/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy local MCP URL" })).not.toBeInTheDocument();
});

it("shows the public MCP URL and the proxy disclaimer once an origin is saved", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, publicOrigin: "https://box.example.test", mcpUrl: "https://box.example.test/mcp" }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy MCP URL" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith("https://box.example.test/mcp"));
  expect(screen.getByText(/Codemux doesn’t check the proxy/)).toHaveTextContent("Point your HTTPS proxy at http://127.0.0.1:4378/mcp");
  expect(screen.queryByRole("button", { name: "Copy local MCP URL" })).not.toBeInTheDocument();
  expect(screen.queryByText(/tailscale serve/)).not.toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Public HTTPS origin" })).toHaveValue("https://box.example.test");
});

it("says how many assistants an address change signed out", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  api.config.mockResolvedValue(snapshot({ ...LISTENING, publicOrigin: "https://box.example.test", revokedGrants: 2 }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Public HTTPS origin" }), { target: { value: "https://box.example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  expect(await screen.findByText(/2 assistants were signed out/)).toBeInTheDocument();
  api.config.mockResolvedValue(snapshot({ ...LISTENING, revokedGrants: 0 }));
  fireEvent.click(screen.getByRole("switch", { name: "Allow network connections" }));
  await waitFor(() => expect(api.config).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByText(/signed out\./)).not.toBeInTheDocument());
});

it("verifies network enablement by read-back, not the write response", async () => {
  const write = deferred<AgentConnectorStatus>();
  api.config.mockReturnValue(write.promise);
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Allow network connections" });
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
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Public HTTPS origin" }), { target: { value: origin } });
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/HTTPS origin only/i);
  expect(api.config).not.toHaveBeenCalled();
});

it.each([31, 0, 1, 127])("rejects a retained raw origin control character %i before IPC", async (code) => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
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

it("saves a public HTTPS origin by read-back and warns that it signs assistants out", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Public HTTPS origin" }), { target: { value: "https://connector.example.test" } });
  expect(screen.getByText(/Changing it signs out every assistant/)).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, publicOrigin: "https://connector.example.test", mcpUrl: "https://connector.example.test/mcp" }));
  fireEvent.click(screen.getByRole("button", { name: "Save origin" }));
  await waitFor(() => expect(api.config).toHaveBeenCalledWith({ enabled: true, publicOrigin: "https://connector.example.test" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Save origin" })).not.toBeDisabled());
  expect(screen.getByRole("button", { name: "Copy MCP URL" })).toBeInTheDocument();
});

it("keeps the verified state after a failed write and does not expose raw errors", async () => {
  api.status.mockResolvedValue(snapshot({ enabled: true }));
  api.config.mockRejectedValue(new Error("token=never-render-this"));
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Allow network connections" });
  fireEvent.click(toggle);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to update/i);
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(toggle).toBeChecked();
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});

it("keeps an unsaved public origin when only network enablement is changed", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  const input = await screen.findByRole("textbox", { name: "Public HTTPS origin" });
  fireEvent.change(input, { target: { value: "https://draft.example.test" } });
  fireEvent.click(screen.getByRole("switch", { name: "Allow network connections" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  expect(api.config).toHaveBeenCalledWith({ enabled: false, publicOrigin: null });
  expect(screen.getByRole("textbox", { name: "Public HTTPS origin" })).toHaveValue("https://draft.example.test");
});

it("does not show old clipboard success for a changed setup command", async () => {
  render(<OutsideAssistantsSection />);
  const copied = () => screen.getByRole("button", { name: "Copy Codex command" }).querySelector(".text-status-open");
  fireEvent.click(await screen.findByRole("button", { name: "Copy Codex command" }));
  await waitFor(() => expect(copied()).not.toBeNull());
  api.status.mockResolvedValue(snapshot({ localExecutable: "/opt/new-codemux" }));
  await act(async () => api.changed?.());
  expect(screen.getByText("codex mcp add codemux -- /opt/new-codemux mcp --client codex")).toBeInTheDocument();
  expect(copied()).toBeNull();
});

it("reports failed clipboard writes instead of claiming copy success", async () => {
  api.copy.mockResolvedValue(false);
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Copy Claude Code command" }));
  await waitFor(() => expect(api.toastError).toHaveBeenCalledWith("Couldn't copy — select and copy manually."));
  expect(api.toastSuccess).not.toHaveBeenCalled();
});

it("does not grant a request invalidated by a refresh before React re-renders", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
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

it("warns to approve only a sign-in you just started, and marks a returning assistant", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [{ ...request, reconnecting: true }] }));
  render(<OutsideAssistantsSection />);
  const group = within(await screen.findByRole("group", { name: "Connection request from " + request.clientName }));
  const warning = group.getByText(/Approve only a sign-in you just started/);
  expect(warning).toHaveTextContent("Approve only a sign-in you just started. Any program on this computer can ask.");
  expect(warning.compareDocumentPosition(group.getByRole("button", { name: "Approve" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(group.getByText("Reconnecting · approved before")).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [{ ...request, phase: "approved_awaiting_client", access: "read_only" }] }));
  await act(async () => api.changed?.());
  expect(screen.queryByText(/Approve only a sign-in you just started/)).not.toBeInTheDocument();
  expect(screen.queryByText("Reconnecting · approved before")).not.toBeInTheDocument();
});

it("warns that anyone who can reach a saved public origin can ask", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, publicOrigin: "https://box.example.test", mcpUrl: "https://box.example.test/mcp", pending: [request] }));
  render(<OutsideAssistantsSection />);
  const group = within(await screen.findByRole("group", { name: "Connection request from " + request.clientName }));
  expect(group.getByText(/Approve only a sign-in you just started/)).toHaveTextContent("Approve only a sign-in you just started. Anyone who can reach https://box.example.test can ask.");
  expect(group.queryByText(/Any program on this computer/)).not.toBeInTheDocument();
});

it("reloads on change events and ignores an older completion", async () => {
  render(<OutsideAssistantsSection />);
  await screen.findByText(CODEX_COMMAND);
  const older = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(older.promise).mockResolvedValueOnce(snapshot({ ...LISTENING }));
  await act(async () => api.changed?.());
  await act(async () => api.changed?.());
  expect(screen.getByRole("switch", { name: "Allow network connections" })).toBeChecked();
  await act(async () => older.resolve(snapshot({ enabled: false })));
  expect(screen.getByRole("switch", { name: "Allow network connections" })).toBeChecked();
});

it("follows the connector's own listener through change events without trusting payloads", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
  render(<OutsideAssistantsSection />);
  await screen.findByText("http://127.0.0.1:4378/mcp");
  const rebind = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(rebind.promise);
  await act(async () => api.changed?.());
  expect(screen.queryByRole("button", { name: "Copy local MCP URL" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  await act(async () => rebind.resolve(snapshot({ ...LISTENING, port: 5432, localUrl: "http://127.0.0.1:5432/mcp", mcpUrl: "http://127.0.0.1:5432/mcp", pending: [request] })));
  expect(screen.getByText("http://127.0.0.1:5432/mcp")).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ enabled: true, port: 5432, bindError: "bind 127.0.0.1:5432: Address already in use", pending: [request] }));
  await act(async () => api.changed?.());
  expect(screen.getByText("Couldn’t listen on port 5432")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy local MCP URL" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  expect(api.status).toHaveBeenCalledTimes(3);
});

it("does not subscribe to web-remote state", async () => {
  render(<OutsideAssistantsSection />);
  await screen.findByText(CODEX_COMMAND);
  expect(api.listen).toHaveBeenCalledTimes(1);
  expect(api.remoteListen).not.toHaveBeenCalled();
});

it("refreshes successfully after status failure without leaking the raw message", async () => {
  api.status.mockRejectedValueOnce(new Error("client_secret=never-render-this"));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to verify/);
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  expect(await screen.findByText(CODEX_COMMAND)).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("keeps a failed live subscription warning across successful refresh and mutation", async () => {
  api.listen.mockRejectedValue(new Error("token=never-render-this"));
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Allow network connections" });
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
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});

it("preserves an edited public origin while refreshing server state", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  const input = await screen.findByRole("textbox", { name: "Public HTTPS origin" });
  fireEvent.change(input, { target: { value: "https://draft.example.test" } });
  api.status.mockResolvedValue(snapshot({ ...LISTENING, publicOrigin: "https://saved.example.test", mcpUrl: "https://saved.example.test/mcp" }));
  await act(async () => api.changed?.());
  expect(screen.getByRole("button", { name: "Copy MCP URL" })).toBeInTheDocument();
  expect(input).toHaveValue("https://draft.example.test");
});

it("waits for the connector subscription before its initial read", async () => {
  const pending = deferred<() => void>();
  api.listen.mockReturnValue(pending.promise);
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(api.status).not.toHaveBeenCalled();
  await act(async () => pending.resolve(vi.fn()));
  expect(await screen.findByText(CODEX_COMMAND)).toBeInTheDocument();
  expect(api.status).toHaveBeenCalledTimes(1);
  view.unmount();
});

it("disposes the active subscription and ignores callbacks after unmount", async () => {
  const dispose = vi.fn();
  api.listen.mockImplementation(async (cb: () => void) => { api.changed = cb; return dispose; });
  const view = render(<OutsideAssistantsSection />);
  await screen.findByText(CODEX_COMMAND);
  view.unmount();
  expect(dispose).toHaveBeenCalledTimes(1);
  await act(async () => { api.changed?.(); fireEvent.focus(window); });
  expect(api.status).toHaveBeenCalledTimes(1);
});

it("disposes a late event subscription after unmount", async () => {
  const subscription = deferred<() => void>();
  const unlisten = vi.fn();
  api.listen.mockReturnValue(subscription.promise);
  const view = render(<OutsideAssistantsSection />);
  view.unmount();
  await act(async () => subscription.resolve(unlisten));
  expect(unlisten).toHaveBeenCalledTimes(1);
  expect(api.status).not.toHaveBeenCalled();
});

it("re-reads a connector event during mutation read-back instead of publishing stale state", async () => {
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Allow network connections" });
  const older = deferred<AgentConnectorStatus>();
  api.status.mockReturnValueOnce(older.promise).mockResolvedValueOnce(snapshot({ enabled: true }));
  fireEvent.click(toggle);
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  await act(async () => api.changed?.());
  await act(async () => older.resolve(snapshot({ enabled: false })));
  await waitFor(() => expect(toggle).not.toBeDisabled());
  expect(toggle).toBeChecked();
  expect(api.status).toHaveBeenCalledTimes(3);
});

it("ignores a late subscription failure after unmount", async () => {
  const pending = deferred<() => void>();
  api.listen.mockReturnValue(pending.promise);
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  view.unmount();
  await act(async () => pending.reject(new Error("token=never-render-this")));
  expect(api.status).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("locks privileged actions until a failed config read-back is refreshed", async () => {
  render(<OutsideAssistantsSection />);
  const toggle = await screen.findByRole("switch", { name: "Allow network connections" });
  api.status.mockRejectedValueOnce(new Error("secret=never-render-this"));
  fireEvent.click(toggle);
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to verify/);
  expect(toggle).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  await waitFor(() => expect(toggle).not.toBeDisabled());
});

it("does not reload in a loop when a grant expires beyond the timer range", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-11-03T12:00:00Z"));
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [{ ...granted, expiresAt: new Date(Date.now() + 30 * 86_400_000).toISOString() }] }));
  render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(api.status).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(api.status).toHaveBeenCalledTimes(1);
});

it("clears its expiry timer on unmount", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-11-03T12:00:00Z"));
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [{ ...request, requestedAt: new Date().toISOString(), expiresAt: new Date(Date.now() + 60_000).toISOString() }] }));
  const view = render(<OutsideAssistantsSection />);
  await act(async () => {});
  expect(vi.getTimerCount()).toBeGreaterThan(0);
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
  await act(async () => vi.advanceTimersByTimeAsync(120_000));
  expect(api.status).toHaveBeenCalledTimes(1);
});

const request = { id: "request-1", clientName: "<img src=x onerror=evil()> assistant", callbackOrigin: "https://assistant.example.test", requestedAt: "2026-10-08T00:00:00Z", expiresAt: new Date(Date.now() + 300_000).toISOString(), phase: "awaiting_approval" as const, access: null, reconnecting: false };
const granted = { id: "client-1", kind: "oauth" as const, clientName: "Example assistant", callbackOrigin: "https://assistant.example.test", access: "read_only" as const, createdAt: "2026-10-08T00:00:00Z", expiresAt: new Date(Date.now() + 3_600_000).toISOString(), label: null, renews: false };
const tokenClient = { id: "mock-token-1", kind: "token" as const, clientName: "<b>hermes-bot</b>", callbackOrigin: null, access: "supervised" as const, createdAt: "2026-10-08T00:00:00Z", expiresAt: new Date(Date.now() + 86_400_000).toISOString(), label: "<b>hermes-bot</b>", renews: false };
const issuedToken = { token: "mock-token-not-a-credential-7", clientId: "mock-token-1", label: "hermes-bot", access: "supervised" as const,
  expiresAt: new Date(Date.now() + 30 * 86_400_000).toISOString(), mcpUrl: "http://127.0.0.1:4378/mcp", localUrl: "http://127.0.0.1:4378/mcp", replaced: false };

/** Issue answers with `token` and, like the backend, makes later reads list its client. */
function issueListing(token: typeof issuedToken, state: Partial<AgentConnectorStatus> = LISTENING) {
  api.issue.mockImplementation(async () => {
    api.status.mockResolvedValue(snapshot({ ...state, clients: [{ ...tokenClient, id: token.clientId, clientName: token.label, label: token.label }] }));
    return token;
  });
}

it("hides the token form while network connections are off or the listener is down", async () => {
  for (const state of [snapshot({ clients: [granted] }), snapshot({ enabled: true }), snapshot({ enabled: true, bindError: "bind 127.0.0.1:4378: Address already in use" })]) {
    api.status.mockResolvedValue(state);
    const view = render(<OutsideAssistantsSection />);
    expect(await screen.findByRole("heading", { name: "Assistants with access" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Token label" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create token" })).not.toBeInTheDocument();
    view.unmount();
  }
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByRole("textbox", { name: "Token label" })).toBeInTheDocument();
  expect(screen.getByText("No assistants have network access yet.")).toBeInTheDocument();
  expect(screen.getByRole("combobox", { name: "Access for new token" })).toHaveValue("supervised");
});

it("creates a token by read-back, shows it once and forgets it on Done", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  const write = deferred<typeof issuedToken>();
  api.issue.mockReturnValue(write.promise);
  const view = render(<OutsideAssistantsSection />);
  const create = await screen.findByRole("button", { name: "Create token" });
  expect(create).toBeDisabled();
  fireEvent.change(screen.getByRole("textbox", { name: "Token label" }), { target: { value: "  hermes-bot " } });
  fireEvent.click(create);
  expect(api.issue).toHaveBeenCalledWith({ label: "hermes-bot", access: "supervised" });
  expect(create).toBeDisabled();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [{ ...tokenClient, clientName: "hermes-bot", label: "hermes-bot" }] }));
  await act(async () => write.resolve(issuedToken));
  const reveal = within((await screen.findByText("Copy the token for “hermes-bot” now")).closest("[role=status]") as HTMLElement);
  expect(reveal.getByText("Copy the token for “hermes-bot” now")).toBeInTheDocument();
  expect(reveal.getByText(issuedToken.token)).toBeInTheDocument();
  expect(reveal.getByText(/won’t show it again/)).toHaveTextContent(issuedToken.mcpUrl);
  expect(reveal.getByText(/won’t show it again/)).toHaveTextContent("stops working sooner if you change the port or public origin.");
  fireEvent.click(reveal.getByRole("button", { name: "Copy token" }));
  await waitFor(() => expect(api.copy).toHaveBeenCalledWith(issuedToken.token));
  await waitFor(() => expect(api.toastSuccess).toHaveBeenCalledWith("Copied to clipboard"));
  expect(screen.getByRole("group", { name: "Token hermes-bot" })).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Token label" })).toHaveValue("");
  for (const element of document.querySelectorAll("[aria-label], [title]")) {
    expect(`${element.getAttribute("aria-label")} ${element.getAttribute("title")}`).not.toContain(issuedToken.token);
  }
  fireEvent.click(reveal.getByRole("button", { name: "Done" }));
  expect(document.body.textContent).not.toContain(issuedToken.token);
  view.unmount();
  render(<OutsideAssistantsSection />);
  await screen.findByRole("group", { name: "Token hermes-bot" });
  expect(document.body.textContent).not.toContain(issuedToken.token);
  for (const call of [...api.toastSuccess.mock.calls, ...api.toastError.mock.calls]) expect(JSON.stringify(call)).not.toContain(issuedToken.token);
});

it("drops an unconfirmed token reveal on unmount", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  issueListing({ ...issuedToken, replaced: true });
  const view = render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "hermes-bot" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByText(/earlier token for “hermes-bot” no longer works/)).toBeInTheDocument();
  view.unmount();
  render(<OutsideAssistantsSection />);
  await screen.findByRole("textbox", { name: "Token label" });
  expect(document.body.textContent).not.toContain(issuedToken.token);
});

it("names only the public origin in a token reveal bound to it", async () => {
  const origin = { ...LISTENING, publicOrigin: "https://box.example.test", mcpUrl: "https://box.example.test/mcp" };
  api.status.mockResolvedValue(snapshot(origin));
  issueListing({ ...issuedToken, mcpUrl: "https://box.example.test/mcp" }, origin);
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "hermes-bot" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  const sentence = await screen.findByText(/won’t show it again/);
  expect(sentence).toHaveTextContent("https://box.example.test/mcp");
  expect(sentence).toHaveTextContent("stops working sooner if you change the public origin.");
  expect(sentence).not.toHaveTextContent(/port/);
});

it("keeps a token reveal through a failed read and drops it once its row is revoked", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  issueListing(issuedToken);
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "hermes-bot" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByText(issuedToken.token)).toBeInTheDocument();
  api.status.mockRejectedValueOnce(new Error("synthetic read failure"));
  await act(async () => api.changed?.());
  expect(screen.getByRole("alert")).toHaveTextContent("Unable to verify connections");
  expect(screen.getByText(issuedToken.token)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Refresh outside assistants" }));
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  expect(screen.getByText(issuedToken.token)).toBeInTheDocument();
  const row = within(screen.getByRole("group", { name: "Token hermes-bot" }));
  fireEvent.click(row.getByRole("button", { name: "Revoke" }));
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  fireEvent.click(row.getByRole("button", { name: "Revoke access" }));
  await waitFor(() => expect(api.revoke).toHaveBeenCalledWith(issuedToken.clientId));
  await waitFor(() => expect(screen.queryByText(/Copy the token/)).not.toBeInTheDocument());
  expect(document.body.textContent).not.toContain(issuedToken.token);
});

it("does not bring back a revoked token's reveal when network connections return", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  issueListing(issuedToken);
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "hermes-bot" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByText(issuedToken.token)).toBeInTheDocument();
  const listed = { ...tokenClient, clientName: "hermes-bot", label: "hermes-bot" };
  api.status.mockResolvedValue(snapshot({ clients: [listed] }));
  fireEvent.click(screen.getByRole("switch", { name: "Allow network connections" }));
  expect(await screen.findByText(/Network connections are off/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Revoke all" }));
  api.status.mockResolvedValue(snapshot());
  fireEvent.click(screen.getByRole("button", { name: "Revoke every assistant" }));
  await waitFor(() => expect(screen.queryByRole("heading", { name: "Assistants with access" })).not.toBeInTheDocument());
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  fireEvent.click(screen.getByRole("switch", { name: "Allow network connections" }));
  expect(await screen.findByText("No assistants have network access yet.")).toBeInTheDocument();
  expect(screen.queryByText(/Copy the token/)).not.toBeInTheDocument();
  expect(document.body.textContent).not.toContain(issuedToken.token);
});

it("keeps a failed token creation generic", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  api.issue.mockRejectedValue(new Error("token=never-render-this"));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "hermes-bot" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Unable to create a token. Refresh and retry.");
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Copy the token/)).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Token label" })).not.toBeDisabled());
  expect(screen.getByRole("textbox", { name: "Token label" })).toHaveValue("hermes-bot");
});

it.each(["-leading-dash", "bad/slash", "tab\tlabel"])("rejects token label %j with an alert and no IPC", async (label) => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: label } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/token label/);
  expect(api.issue).not.toHaveBeenCalled();
});

it("issues the chosen access for a new token", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  api.issue.mockResolvedValue({ ...issuedToken, access: "read_only" });
  render(<OutsideAssistantsSection />);
  fireEvent.change(await screen.findByRole("textbox", { name: "Token label" }), { target: { value: "ci-bot" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Access for new token" }), { target: { value: "read_only" } });
  fireEvent.click(screen.getByRole("button", { name: "Create token" }));
  await waitFor(() => expect(api.issue).toHaveBeenCalledWith({ label: "ci-bot", access: "read_only" }));
});

it("lists OAuth and token clients with their kind, access and expiry, and revokes either", async () => {
  const renewing = { ...granted, id: "client-2", clientName: "Renewing assistant", renews: true };
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [granted, renewing, tokenClient] }));
  render(<OutsideAssistantsSection />);
  const oauth = within(await screen.findByRole("group", { name: "Signed-in assistant Example assistant" }));
  expect(oauth.getByText("https://assistant.example.test · Read-only")).toBeInTheDocument();
  expect(oauth.getByText(/^Approved .* · Expires /)).toBeInTheDocument();
  expect(within(screen.getByRole("group", { name: "Signed-in assistant Renewing assistant" })).getByText(/Renews automatically · lapses if unused by/)).toBeInTheDocument();
  const token = within(screen.getByRole("group", { name: "Token <b>hermes-bot</b>" }));
  expect(token.getByText("<b>hermes-bot</b>")).toBeInTheDocument();
  expect(document.querySelector("b")).toBeNull();
  expect(token.getByText("Token · Supervised")).toBeInTheDocument();
  expect(token.getByText(/^Created .* · Expires /)).toBeInTheDocument();
  fireEvent.click(token.getByRole("button", { name: "Revoke" }));
  expect(token.getByText("Revoke this token? Assistants using it stop working right away.")).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [granted, renewing] }));
  fireEvent.click(token.getByRole("button", { name: "Revoke access" }));
  await waitFor(() => expect(api.revoke).toHaveBeenCalledWith("mock-token-1"));
  await waitFor(() => expect(screen.queryByRole("group", { name: "Token <b>hermes-bot</b>" })).not.toBeInTheDocument());
  fireEvent.click(oauth.getByRole("button", { name: "Revoke" }));
  expect(oauth.getByText("Revoke this assistant’s access? It will need approval to reconnect.")).toBeInTheDocument();
  fireEvent.click(oauth.getByRole("button", { name: "Revoke access" }));
  await waitFor(() => expect(api.revoke).toHaveBeenCalledWith("client-1"));
});

it("revokes every assistant only after confirming", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [granted, tokenClient] }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Revoke all" }));
  expect(api.revokeAll).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.click(screen.getByRole("button", { name: "Revoke all" }));
  api.status.mockResolvedValue(snapshot({ ...LISTENING }));
  fireEvent.click(screen.getByRole("button", { name: "Revoke every assistant" }));
  await waitFor(() => expect(api.revokeAll).toHaveBeenCalledTimes(1));
  expect(await screen.findByText("No assistants have network access yet.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Revoke all" })).not.toBeInTheDocument();
});

it("says paused grants can't connect while network connections are off", async () => {
  api.status.mockResolvedValue(snapshot({ clients: [tokenClient] }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(/Network connections are off, so these can’t connect/)).toBeInTheDocument();
});

it("approves untrusted client names as plain text with read-only access by default", async () => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
  render(<OutsideAssistantsSection />);
  expect(await screen.findByText(request.clientName)).toBeInTheDocument();
  expect(document.querySelector("img")).toBeNull();
  expect(screen.getByText(/Registered callback origin/)).toHaveTextContent(request.callbackOrigin);
  expect(screen.queryByText(/Verified callback/)).not.toBeInTheDocument();
  expect(screen.getByText(/client name and callback origin are self-asserted/i)).toHaveTextContent(/not proof of identity/i);
  expect(screen.getByRole("combobox", { name: "Access for " + request.clientName })).toHaveValue("read_only");
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [{ ...request, phase: "approved_awaiting_client", access: "read_only" }] }));
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(api.approve).toHaveBeenCalledWith("request-1", "read_only"));
  expect(await screen.findByText(/Approved.*Read-only.*waiting for the assistant/)).toBeInTheDocument();
  expect(screen.getByText(request.clientName)).toBeInTheDocument();
  api.status.mockResolvedValue(snapshot({ ...LISTENING, clients: [granted] }));
  await act(async () => api.changed?.());
  expect(await screen.findByText("Example assistant")).toBeInTheDocument();
  expect(screen.queryByText(request.clientName)).not.toBeInTheDocument();
});

it.each(["read_only", "supervised", "full_access"] as const)("shows decided native approval awaiting the client without repeated decisions for %s", async (access) => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const select = await screen.findByRole("combobox", { name: "Access for " + request.clientName });
  fireEvent.change(select, { target: { value: access } });
  const approved = { ...request, phase: "approved_awaiting_client" as const, access };
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [approved] }));
  fireEvent.click(screen.getByRole("button", { name: "Approve" }));
  await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument());
  const group = within(screen.getByRole("group", { name: "Connection request from " + request.clientName }));
  expect(group.queryByRole("button", { name: "Deny" })).not.toBeInTheDocument();
  expect(group.queryByRole("combobox")).not.toBeInTheDocument();
  expect(group.getByText(/Approved.*waiting for the assistant/i)).toHaveTextContent(access === "read_only" ? "Read-only" : access === "supervised" ? "Supervised" : "Full access");
  expect(group.getByText(/Continue.*browser/i)).toBeInTheDocument();
  expect(screen.queryByRole("group", { name: /^Signed-in assistant/ })).not.toBeInTheDocument();
  expect(api.approve).toHaveBeenCalledTimes(1);
  expect(api.deny).not.toHaveBeenCalled();
});

it.each(["read_only", "supervised", "full_access"] as const)("discloses instance-wide consent before approval for %s", async (access) => {
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
  render(<OutsideAssistantsSection />);
  const group = within(await screen.findByRole("group", { name: "Connection request from " + request.clientName }));
  fireEvent.change(group.getByRole("combobox"), { target: { value: access } });
  const scope = group.getByText(/Applies to all Codemux workspaces and visible conversations, not just this project/i);
  const reads = group.getByText(/visible user and top-level assistant messages, not reasoning or tool payloads/i);
  const approve = group.getByRole("button", { name: "Approve" });
  expect(scope.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(reads.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const authority = group.getByText(access === "read_only" ? /cannot launch workers, send messages or change state/i
    : access === "supervised" ? /some modes allow file edits/i : /with your account’s permissions/i);
  expect(authority.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  if (access === "supervised") {
    expect(authority).toHaveTextContent(/You handle approval requests in Codemux/i);
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
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request] }));
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
  expect(screen.queryByRole("group", { name: /^Signed-in assistant/ })).not.toBeInTheDocument();
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
  api.status.mockResolvedValue(snapshot({ ...LISTENING, pending: [request], clients: [granted] }));
  render(<OutsideAssistantsSection />);
  fireEvent.click(await screen.findByRole("button", { name: action === "approve" ? "Approve" : action === "deny" ? "Deny" : "Revoke" }));
  if (action === "revoke") fireEvent.click(screen.getByRole("button", { name: "Revoke access" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/Unable to/);
  await waitFor(() => expect(screen.getByRole("button", { name: action === "revoke" ? "Revoke access" : action === "approve" ? "Approve" : "Deny" })).not.toBeDisabled());
  expect(screen.getByText(request.clientName)).toBeInTheDocument();
  expect(screen.getByText("Example assistant")).toBeInTheDocument();
  expect(screen.queryByText(/never-render-this/)).not.toBeInTheDocument();
});
