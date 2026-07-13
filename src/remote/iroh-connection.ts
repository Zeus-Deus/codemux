/**
 * Bridges the account device-registry into {@link RemoteTransport}'s injectable
 * seams so the existing transport/shim/app stack runs unchanged over iroh.
 *
 * `RemoteTransport` bootstraps every (re)connect as: `POST /api/ws-ticket` →
 * `wsFactory(url)`. Over iroh there is no HTTP ticket endpoint and no session
 * token in a URL to protect; the natural analog of "mint a short-lived
 * credential for this connection" is `POST /api/devices/:id/connect`, which
 * returns a fresh {@link DeviceGrant}. So this module supplies:
 *
 *   - a `fetchImpl` that intercepts `/api/ws-ticket` and, instead, mints a fresh
 *     grant (bound to a new `browserNonce`), caches it, and returns a stub
 *     ticket. A grant-mint failure maps onto the ticket status the transport
 *     already understands: an account-auth failure → `401` (→ `onUnauthorized`,
 *     the bootstrap re-signs-in); a transient/offline failure → `503` (→ the
 *     transport's normal backoff retry, so a briefly-offline desktop reconnects).
 *   - a `wsFactory` that opens an {@link IrohWebSocket} to the cached grant's
 *     `node_id`, sending `{grant, browserNonce}` in the `hello-account` frame.
 *
 * A **fatal** handshake rejection (bad/expired grant, node mismatch) latches a
 * flag so the *next* ticket request returns `401` — breaking what would
 * otherwise be an unauthenticatable reconnect loop, and routing the user back to
 * sign-in. A **pending** rejection is left transient: the transport keeps
 * re-minting + re-dialing (polling for desktop approval), exactly as the WS path
 * polls a `403` ticket.
 */
import type { WebSocketLike } from "./transport";
import type { TransportDeps } from "./transport";
import { DeviceRegistry, DeviceRegistryError, type DeviceGrant } from "./device-registry";
import { IrohWebSocket, type IrohDialer } from "./iroh-transport";

export interface IrohConnectionOptions {
  /** Anything that can mint a grant — the real {@link DeviceRegistry} in prod. */
  registry: Pick<DeviceRegistry, "connectDevice">;
  /** Server row id of the selected device (the `:id` in `/connect`). */
  deviceId: string;
  dialer: IrohDialer;
  /** The desktop reported the session is pending approval (poll continues). */
  onPending?(): void;
  /** A grant mint failed; advisory for UI ("device offline, retrying…"). The
   *  transport still retries on its own backoff. */
  onMintError?(err: DeviceRegistryError): void;
  /** Override the random `browserNonce` generator (tests). */
  nonceFactory?(): string;
}

/**
 * Build the `{fetchImpl, wsFactory}` pair to hand to `installShim({deps})` /
 * `new RemoteTransport({deps})`. The returned deps close over a single grant
 * cache shared between the ticket mint and the socket dial.
 */
export function createIrohConnection(
  opts: IrohConnectionOptions,
): Required<Pick<TransportDeps, "fetchImpl" | "wsFactory">> {
  const nonceFactory = opts.nonceFactory ?? randomNonce;
  let current: DeviceGrant | null = null;
  let currentNonce: string | null = null;
  let fatal = false;

  const fetchImpl = (async (input: RequestInfo | URL): Promise<Response> => {
    const url = String(input instanceof Request ? input.url : input);
    if (!url.includes("/api/ws-ticket")) {
      // The transport never calls anything else through this fetch, but be safe.
      return new Response(null, { status: 404 });
    }
    if (fatal) {
      // A prior handshake fatally rejected — stop trying to reconnect.
      return new Response(null, { status: 401 });
    }
    const nonce = nonceFactory();
    try {
      const grant = await opts.registry.connectDevice(opts.deviceId, nonce);
      current = grant;
      currentNonce = nonce;
      return new Response(JSON.stringify({ ticket: "iroh" }), { status: 200 });
    } catch (err) {
      if (err instanceof DeviceRegistryError) {
        opts.onMintError?.(err);
        // Auth failures / removed device → give up (re-auth). Everything else is
        // transient (offline, network, 5xx) → let the transport back off + retry.
        const giveUp = err.kind === "unauthorized" || err.kind === "not_found";
        return new Response(null, { status: giveUp ? 401 : 503 });
      }
      return new Response(null, { status: 503 });
    }
  }) as typeof fetch;

  const wsFactory = (_url: string): WebSocketLike =>
    new IrohWebSocket({
      dialer: opts.dialer,
      target: () => {
        if (!current) throw new Error("iroh: no grant minted before dial");
        return { nodeId: current.nodeId, relayUrl: current.relayUrlHint };
      },
      handshake: () => {
        if (!current || !currentNonce) {
          throw new Error("iroh: no grant minted before handshake");
        }
        return { grant: current.grant, nonce: currentNonce };
      },
      onReject: (kind) => {
        if (kind === "fatal") fatal = true;
        else opts.onPending?.();
      },
    });

  return { fetchImpl, wsFactory };
}

/** A URL-safe random nonce (16 bytes, base64url, no padding). */
function randomNonce(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
