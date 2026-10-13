import type { AgentConnectorAccess, AgentConnectorClient, AgentConnectorPending, AgentConnectorStatus } from "@/tauri/types";

/** Synthetic browser-dev state only; never loaded by native/production code. */
export function createAgentConnectorMock(
  listener: () => { running: boolean; port: number },
  changed: () => void,
) {
  let enabled = false;
  let publicOrigin: string | null = null;
  let sequence = 0;
  let pending: AgentConnectorPending[] = [];
  let clients: AgentConnectorClient[] = [];
  const awaitingExchange = new Map<string, { request: Extract<AgentConnectorPending, { phase: "approved_awaiting_client" }>; expiresAt: number }>();
  let boundIssuer: string | null = null;
  const prune = () => {
    const issuer = publicOrigin ?? `http://127.0.0.1:${listener().port}`;
    if (boundIssuer !== issuer) {
      pending = [];
      awaitingExchange.clear();
      boundIssuer = issuer;
    }
    const now = Date.now();
    pending = pending.filter((request) => Date.parse(request.requestedAt) + 300_000 > now);
    clients = clients.filter((client) => Date.parse(client.expiresAt) > now);
    for (const [id, completion] of awaitingExchange) {
      if (completion.expiresAt <= now) awaitingExchange.delete(id);
    }
  };
  const status = (): AgentConnectorStatus => {
    prune();
    const { running, port } = listener();
    return {
      enabled, publicOrigin, listenerRunning: running,
      mcpUrl: enabled && running ? new URL("/mcp", publicOrigin ?? `http://127.0.0.1:${port}`).href : null,
      localCommand: "/usr/bin/codemux mcp",
      pending: pending.map((request) => ({ ...request })),
      clients: clients.map((client) => ({ ...client })),
    };
  };
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    agent_connector_status: status,
    agent_connector_set_config: (args) => {
      if (typeof args.enabled !== "boolean") throw new Error("Explicit enabled state required");
      if (args.publicOrigin !== null && (typeof args.publicOrigin !== "string" || !httpsOrigin(args.publicOrigin))) {
        throw new Error("Use an HTTPS origin only");
      }
      const nextOrigin = args.publicOrigin === null ? null : new URL(args.publicOrigin as string).origin;
      if (!args.enabled || publicOrigin !== nextOrigin) {
        pending = [];
        awaitingExchange.clear();
      }
      enabled = args.enabled;
      publicOrigin = nextOrigin;
      changed();
      return status();
    },
    agent_connector_approve: (args) => {
      prune();
      if (!enabled || !listener().running) throw new Error("HTTP listener is unavailable");
      if (!["read_only", "supervised", "full_access"].includes(String(args.access))) throw new Error("Invalid access level");
      const request = pending.find((item) => item.id === args.requestId);
      if (!request) throw new Error("Request no longer pending");
      if (request.phase !== "awaiting_approval") throw new Error("Request already decided");
      pending = pending.map((item) => item.id === request.id ? {
        ...item, phase: "approved_awaiting_client", access: args.access as AgentConnectorAccess,
      } : item);
      changed();
    },
    agent_connector_deny: (args) => {
      prune();
      const request = pending.find((item) => item.id === args.requestId);
      if (!request) throw new Error("Request no longer pending");
      if (request.phase !== "awaiting_approval") throw new Error("Request already decided");
      pending = pending.map((item) => item.id === request.id ? { ...item, phase: "denied", access: null } : item);
      changed();
    },
    agent_connector_revoke: (args) => {
      if (!clients.some((item) => item.id === args.clientId)) throw new Error("Client no longer granted");
      clients = clients.filter((item) => item.id !== args.clientId);
      changed();
    },
  };
  return {
    handlers,
    /** Fixture-only Continue: no browser binding or native OAuth proof. */
    continuePendingClient(requestId: string): void {
      prune();
      if (!enabled || !listener().running) throw new Error("HTTP listener is unavailable");
      const request = pending.find((item) => item.id === requestId);
      if (!request) throw new Error("Request no longer pending");
      if (request.phase === "awaiting_approval") return;
      pending = pending.filter((item) => item.id !== requestId);
      if (request.phase === "approved_awaiting_client") {
        awaitingExchange.set(requestId, { request, expiresAt: Date.now() + 60_000 });
      }
      changed();
    },
    /** Fixture-only exchange: creates a synthetic display grant, not a token. */
    exchangePendingClient(requestId: string): void {
      prune();
      if (!enabled || !listener().running) throw new Error("HTTP listener is unavailable");
      const completion = awaitingExchange.get(requestId);
      if (!completion) throw new Error("No synthetic exchange is pending");
      const { request } = completion;
      const now = Date.now();
      clients.push({
        id: `mock-client-${++sequence}`, clientName: request.clientName, callbackOrigin: request.callbackOrigin,
        access: request.access, createdAt: new Date(now).toISOString(),
        expiresAt: new Date(now + 3_600_000).toISOString(),
      });
      awaitingExchange.delete(requestId);
      changed();
    },
    /** Browser QA hook: creates no codes, tokens or other credential material. */
    addPendingClient(clientName = "Example assistant", callbackOrigin = "https://assistant.example.test"): string {
      prune();
      if (!enabled || !listener().running) throw new Error("Enable HTTP MCP and its listener first");
      if (!httpsOrigin(callbackOrigin)) throw new Error("Use an HTTPS callback origin only");
      const id = `mock-request-${++sequence}`;
      pending.push({ id, clientName, callbackOrigin, requestedAt: new Date().toISOString(), expiresAt: new Date(Date.now() + 300_000).toISOString(), phase: "awaiting_approval", access: null });
      changed();
      return id;
    },
  };
}

function httpsOrigin(value: string): boolean {
  if (/[\u0000-\u001f\u007f]/.test(value) || !/^https:\/\/[^/?#\\\s@]+\/?$/.test(value)) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !!url.hostname && !url.username && !url.password && url.pathname === "/" && !url.search && !url.hash;
  } catch { return false; }
}
