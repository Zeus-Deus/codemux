import type {
  AgentConnectorAccess,
  AgentConnectorClient,
  AgentConnectorIssuedToken,
  AgentConnectorPending,
  AgentConnectorStatus,
} from "@/tauri/types";

/** The connector listener's default port (its own, not Remote Access's). */
export const MOCK_CONNECTOR_PORT = 4378;
/** A renewing sign-in lapses when it goes this long unused (the refresh-token lifetime). */
export const MOCK_SIGN_IN_LIFETIME_MS = 30 * 86_400_000;
const PENDING_TTL_MS = 300_000;
const EXCHANGE_TTL_MS = 60_000;
const DEFAULT_TOKEN_HOURS = 30 * 24;
const MAX_TOKEN_HOURS = 365 * 24;
const ACCESS_LEVELS: readonly string[] = ["read_only", "supervised", "full_access"];
const TOKEN_LABEL = /^[A-Za-z0-9][A-Za-z0-9 ._@-]{0,63}$/;

/** Synthetic browser-dev state only; never loaded by native/production code.
 *  The connector owns its loopback listener, so Remote Access never changes it. */
export function createAgentConnectorMock(changed: () => void) {
  let enabled = false;
  let publicOrigin: string | null = null;
  let port = MOCK_CONNECTOR_PORT;
  /** Why the listener could not bind; it binds whenever this is null. */
  let failure: string | null = null;
  let sequence = 0;
  let pending: AgentConnectorPending[] = [];
  let clients: AgentConnectorClient[] = [];
  /** The resource each grant is bound to; an address change revokes the others. */
  const grantResource = new Map<string, string>();
  /** Token clients keep their id and first label spelling across reissues, keyed by lower-case label. */
  const tokenClients = new Map<string, { clientId: string; label: string }>();
  /** OAuth clients approved before, so a new request reads as reconnecting. */
  const approvedBefore = new Set<string>();
  const awaitingExchange = new Map<string, { request: Extract<AgentConnectorPending, { phase: "approved_awaiting_client" }>; expiresAt: number }>();

  const running = () => enabled && failure === null;
  const resource = () => new URL("/mcp", publicOrigin ?? `http://127.0.0.1:${port}`).href;
  const localUrl = () => `http://127.0.0.1:${port}/mcp`;
  const prune = () => {
    const now = Date.now();
    pending = pending.filter((request) => Date.parse(request.requestedAt) + PENDING_TTL_MS > now);
    clients = clients.filter((client) => Date.parse(client.expiresAt) > now);
    for (const [id, completion] of awaitingExchange) {
      if (completion.expiresAt <= now) awaitingExchange.delete(id);
    }
  };
  const status = (): AgentConnectorStatus => {
    prune();
    return {
      enabled, publicOrigin, listenerRunning: running(), port,
      bindError: enabled ? failure : null,
      localUrl: running() ? localUrl() : null,
      mcpUrl: running() ? resource() : null,
      localExecutable: "/usr/bin/codemux",
      localCommand: "/usr/bin/codemux mcp",
      pending: pending.map((request) => ({ ...request })),
      clients: clients.map((client) => ({ ...client })),
    };
  };
  const grant = (client: AgentConnectorClient) => {
    clients = [...clients.filter((item) => item.id !== client.id), client];
    grantResource.set(client.id, resource());
  };

  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    agent_connector_status: status,
    agent_connector_set_config: (args) => {
      if (typeof args.enabled !== "boolean") throw new Error("Explicit enabled state required");
      if (args.publicOrigin !== null && (typeof args.publicOrigin !== "string" || !httpsOrigin(args.publicOrigin))) {
        throw new Error("Use an HTTPS origin only");
      }
      const nextPort = args.port === undefined ? port : args.port;
      if (typeof nextPort !== "number" || !Number.isInteger(nextPort) || nextPort < 1024 || nextPort > 65535) {
        throw new Error("Invalid port");
      }
      prune();
      const before = resource();
      // A different port binds afresh; the same one stays as busy as it was.
      if (nextPort !== port) failure = null;
      enabled = args.enabled;
      publicOrigin = args.publicOrigin === null ? null : new URL(args.publicOrigin as string).origin;
      port = nextPort;
      const moved = resource() !== before;
      if (!enabled || moved) {
        pending = [];
        awaitingExchange.clear();
      }
      // Turning the connector off only pauses grants; a new address ends them.
      const kept = moved ? clients.filter((client) => grantResource.get(client.id) === resource()) : clients;
      const revokedGrants = clients.length - kept.length;
      clients = kept;
      changed();
      return { ...status(), revokedGrants };
    },
    agent_connector_approve: (args) => {
      prune();
      if (!running()) throw new Error("HTTP listener is unavailable");
      if (!ACCESS_LEVELS.includes(String(args.access))) throw new Error("Invalid access level");
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
    agent_connector_revoke_all: () => {
      clients = [];
      // Approvals not yet exchanged for a token are withdrawn too, so nothing
      // mints right after the incident switch. Undecided requests stay.
      pending = pending.filter((request) => request.phase !== "approved_awaiting_client");
      awaitingExchange.clear();
      changed();
    },
    agent_connector_issue: (args): AgentConnectorIssuedToken => {
      prune();
      if (!enabled) throw new Error("Turn the connector on first: codemux connector enable");
      const label = typeof args.label === "string" ? args.label.trim() : "";
      if (!TOKEN_LABEL.test(label)) {
        throw new Error("Token labels are 1-64 letters, digits, spaces or . _ @ -, starting with a letter or digit");
      }
      if (!ACCESS_LEVELS.includes(String(args.access))) throw new Error("Invalid access level");
      const hours = args.expiresInHours === undefined ? DEFAULT_TOKEN_HOURS : args.expiresInHours;
      if (typeof hours !== "number" || !Number.isInteger(hours) || hours < 1 || hours > MAX_TOKEN_HOURS) {
        throw new Error("Token expiry must be between 1 hour and 365 days");
      }
      const access = args.access as AgentConnectorAccess;
      // A label matches in any case and keeps its first spelling, as the store does.
      const known = tokenClients.get(label.toLowerCase());
      const clientId = known?.clientId ?? `mock-token-${++sequence}`;
      const firstLabel = known?.label ?? label;
      tokenClients.set(label.toLowerCase(), { clientId, label: firstLabel });
      const replaced = clients.some((client) => client.id === clientId);
      const now = Date.now();
      const expiresAt = new Date(now + hours * 3_600_000).toISOString();
      grant({
        id: clientId, kind: "token", clientName: firstLabel, callbackOrigin: null, access,
        createdAt: new Date(now).toISOString(), expiresAt, label: firstLabel, renews: false,
      });
      changed();
      return {
        token: `mock-token-not-a-credential-${++sequence}`, clientId, label: firstLabel, access, expiresAt,
        mcpUrl: resource(), localUrl: localUrl(), replaced,
      };
    },
  };
  return {
    handlers,
    /** Fixture-only Continue: no browser binding or native OAuth proof. */
    continuePendingClient(requestId: string): void {
      prune();
      if (!running()) throw new Error("HTTP listener is unavailable");
      const request = pending.find((item) => item.id === requestId);
      if (!request) throw new Error("Request no longer pending");
      if (request.phase === "awaiting_approval") return;
      pending = pending.filter((item) => item.id !== requestId);
      if (request.phase === "approved_awaiting_client") {
        awaitingExchange.set(requestId, { request, expiresAt: Date.now() + EXCHANGE_TTL_MS });
      }
      changed();
    },
    /** Fixture-only exchange: creates a synthetic display grant, not a token. */
    exchangePendingClient(requestId: string): void {
      prune();
      if (!running()) throw new Error("HTTP listener is unavailable");
      const completion = awaitingExchange.get(requestId);
      if (!completion) throw new Error("No synthetic exchange is pending");
      const { request } = completion;
      const now = Date.now();
      grant({
        id: `mock-client-${++sequence}`, kind: "oauth", clientName: request.clientName,
        callbackOrigin: request.callbackOrigin, access: request.access, createdAt: new Date(now).toISOString(),
        expiresAt: new Date(now + MOCK_SIGN_IN_LIFETIME_MS).toISOString(), label: null, renews: true,
      });
      approvedBefore.add(`${request.clientName}\n${request.callbackOrigin}`);
      awaitingExchange.delete(requestId);
      changed();
    },
    /** Browser QA hook: creates no codes, tokens or other credential material. */
    addPendingClient(clientName = "Example assistant", callbackOrigin = "https://assistant.example.test"): string {
      prune();
      if (!running()) throw new Error("Turn on network connections and let the listener start first");
      if (!httpsOrigin(callbackOrigin)) throw new Error("Use an HTTPS callback origin only");
      const id = `mock-request-${++sequence}`;
      const now = Date.now();
      pending.push({
        id, clientName, callbackOrigin, requestedAt: new Date(now).toISOString(),
        expiresAt: new Date(now + PENDING_TTL_MS).toISOString(), phase: "awaiting_approval", access: null,
        reconnecting: approvedBefore.has(`${clientName}\n${callbackOrigin}`),
      });
      changed();
      return id;
    },
    /** Browser QA hook: the listener fails to bind (`reason`) or binds again (`null`). */
    failListener(reason: string | null): void {
      failure = reason;
      changed();
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
