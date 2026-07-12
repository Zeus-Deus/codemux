/**
 * Web-remote pre-app bootstrap.
 *
 * Runs before React mounts the real app (awaited from `main.tsx`). It:
 *   1. Consumes a `#pair=<token>` deep-link if present — POSTs
 *      `/api/pair`, stores the returned session in `localStorage`, and
 *      strips the token from the URL so a refresh/screenshot can't leak it.
 *   2. Ensures a valid session exists, prompting with a minimal, self-
 *      contained pairing screen (paste link/code) when it doesn't.
 *   3. Installs the WebSocket shim, connects the transport, and — on the
 *      first live connection — returns so `main.tsx` can mount the app.
 *   4. Handles approval mode: a `{approved:false}` pairing shows a
 *      "waiting for approval" state while the transport polls for a
 *      ticket (granted once the desktop approves).
 *
 * The pairing UI depends on nothing from the app itself (no stores, no
 * app components) so it can render standalone. It matches the app's dark
 * aesthetic via the shared CSS custom properties.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { installShim } from "./shim";
import type { ConnectionStatus } from "./transport";
import { fetchSnapshot } from "./snapshot-seed";
import { useRemoteConnectionStore } from "./remote-connection-store";
import {
  clearSession,
  deriveDeviceName,
  loadSession,
  storeSession,
  type RemoteSession,
} from "./session";

interface PairResult {
  session: RemoteSession;
  approved: boolean;
}

// ── HTTP helpers ────────────────────────────────────────────────────

async function fetchAppVersion(baseUrl: string): Promise<string | null> {
  try {
    const res = await fetch(`${baseUrl}/api/health`, { credentials: "include" });
    if (!res.ok) return null;
    const body = (await res.json()) as { version?: unknown };
    return typeof body.version === "string" ? body.version : null;
  } catch {
    return null;
  }
}

async function pairDevice(baseUrl: string, token: string): Promise<PairResult> {
  const res = await fetch(`${baseUrl}/api/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "include",
    body: JSON.stringify({ token, device_name: deriveDeviceName() }),
  });
  if (!res.ok) {
    // The server answers with machine codes (`invalid_or_expired_token`,
    // `rate_limited`, `origin_mismatch`), not user-facing copy, so map the
    // known statuses to a clear message. A server-provided string is only
    // surfaced when it reads as prose (contains whitespace) — e.g. a 500's
    // underlying error — so a raw code never clobbers the friendly message.
    let message: string;
    if (res.status === 429) {
      message = "Too many attempts. Wait a minute and try again.";
    } else if (res.status === 401 || res.status === 404) {
      message = "That pairing code is invalid or has expired.";
    } else if (res.status === 403) {
      message = "This device isn't allowed to pair from here.";
    } else {
      message = `Pairing failed (${res.status}).`;
      try {
        const body = (await res.json()) as { error?: unknown };
        if (typeof body.error === "string" && /\s/.test(body.error)) {
          message = body.error;
        }
      } catch {
        /* non-JSON error body — keep the status-derived message */
      }
    }
    throw new Error(message);
  }
  const body = (await res.json()) as {
    session_id?: unknown;
    session_token?: unknown;
    approved?: unknown;
  };
  if (typeof body.session_id !== "string" || typeof body.session_token !== "string") {
    throw new Error("The pairing response was incomplete.");
  }
  const session: RemoteSession = {
    sessionId: body.session_id,
    sessionToken: body.session_token,
  };
  storeSession(session);
  // Missing/undefined `approved` means approval mode is off → treat as
  // approved; only an explicit `false` is a pending approval.
  return { session, approved: body.approved !== false };
}

// ── URL / token parsing ─────────────────────────────────────────────

function readPairToken(): string | null {
  const hash = window.location.hash || "";
  const match = /(?:^#|[#&])pair=([^&]+)/.exec(hash);
  return match ? safeDecode(match[1]) : null;
}

function clearPairFragment(): void {
  const url = window.location.pathname + window.location.search;
  window.history.replaceState(null, "", url);
}

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/** Accept either a bare code or a full pairing link (…#pair=<token>). */
function extractToken(raw: string): string {
  const s = raw.trim();
  const idx = s.indexOf("pair=");
  if (idx >= 0) {
    const rest = s.slice(idx + "pair=".length).split(/[&\s]/)[0];
    return safeDecode(rest);
  }
  return s;
}

function dismissSplash(): void {
  const splash = document.getElementById("splash");
  if (!splash) return;
  splash.classList.add("fade-out");
  splash.addEventListener("transitionend", () => splash.remove(), { once: true });
}

// ── Standalone UI ───────────────────────────────────────────────────

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  zIndex: 2147483645,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: 24,
  background: "var(--background, #0C0C0E)",
  color: "var(--foreground, #e8e8e8)",
  fontFamily: "var(--font-sans, system-ui, -apple-system, sans-serif)",
};

const cardStyle: React.CSSProperties = {
  width: "100%",
  maxWidth: 380,
  background: "var(--card, #1a1a1c)",
  border: "1px solid var(--border, rgba(255,255,255,0.1))",
  borderRadius: 14,
  padding: 28,
  boxShadow: "0 12px 40px rgba(0,0,0,0.45)",
};

function PairingForm(props: {
  baseUrl: string;
  host: string;
  onPaired(result: PairResult): void;
}): React.ReactElement {
  const [value, setValue] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  async function submit(e: React.FormEvent): Promise<void> {
    e.preventDefault();
    const token = extractToken(value);
    if (!token) {
      setError("Paste your pairing link or code first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await pairDevice(props.baseUrl, token);
      props.onPaired(result);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Pairing failed.");
      setBusy(false);
    }
  }

  return (
    <div style={overlayStyle}>
      <form style={cardStyle} onSubmit={submit}>
        <div
          style={{
            fontSize: 17,
            fontWeight: 600,
            letterSpacing: "-0.01em",
            marginBottom: 6,
          }}
        >
          Connect to Codemux
        </div>
        <div
          style={{
            fontSize: 13,
            lineHeight: 1.5,
            color: "var(--muted-foreground, #9a9a97)",
            marginBottom: 18,
          }}
        >
          Paste the pairing link or code generated on the desktop app to pair
          this device.
        </div>
        <input
          type="text"
          value={value}
          autoFocus
          disabled={busy}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Pairing link or code"
          aria-label="Pairing link or code"
          style={{
            width: "100%",
            boxSizing: "border-box",
            padding: "10px 12px",
            fontSize: 13,
            fontFamily: "var(--font-mono, ui-monospace, monospace)",
            color: "var(--foreground, #e8e8e8)",
            background: "var(--background, #0C0C0E)",
            border: "1px solid var(--input, rgba(255,255,255,0.15))",
            borderRadius: 9,
            outline: "none",
          }}
        />
        {error && (
          <div
            role="alert"
            style={{
              marginTop: 12,
              fontSize: 12.5,
              lineHeight: 1.45,
              color: "oklch(0.72 0.16 22)",
            }}
          >
            {error}
          </div>
        )}
        <button
          type="submit"
          disabled={busy}
          style={{
            marginTop: 18,
            width: "100%",
            padding: "10px 14px",
            fontSize: 13.5,
            fontWeight: 600,
            cursor: busy ? "default" : "pointer",
            color: "#0a0a0a",
            background: "var(--accent-ember, oklch(0.705 0.152 47))",
            border: "none",
            borderRadius: 9,
            opacity: busy ? 0.7 : 1,
          }}
        >
          {busy ? "Connecting…" : "Connect"}
        </button>
        <div
          style={{
            marginTop: 16,
            fontSize: 11.5,
            color: "var(--muted-foreground, #9a9a97)",
            textAlign: "center",
          }}
        >
          {props.host}
        </div>
      </form>
    </div>
  );
}

function ConnectingView(props: {
  host: string;
  waiting: boolean;
}): React.ReactElement {
  return (
    <div style={overlayStyle}>
      <style>
        {"@keyframes codemux-remote-spin{to{transform:rotate(360deg)}}"}
      </style>
      <div style={{ textAlign: "center", maxWidth: 340 }}>
        <div
          style={{
            width: 26,
            height: 26,
            margin: "0 auto 18px",
            borderRadius: "50%",
            border: "2.5px solid var(--border, rgba(255,255,255,0.15))",
            borderTopColor: "var(--accent-ember, oklch(0.705 0.152 47))",
            animation: "codemux-remote-spin 0.8s linear infinite",
          }}
        />
        <div style={{ fontSize: 14.5, fontWeight: 600, marginBottom: 6 }}>
          {props.waiting ? "Waiting for approval" : "Connecting"}
        </div>
        <div
          style={{
            fontSize: 12.5,
            lineHeight: 1.5,
            color: "var(--muted-foreground, #9a9a97)",
          }}
        >
          {props.waiting
            ? `Approve this device on the desktop app to finish connecting to ${props.host}.`
            : `Reaching ${props.host}…`}
        </div>
      </div>
    </div>
  );
}

/** Owns a single React root in a dedicated overlay element so the pairing
 *  UI never fights the app's `#root` React tree. */
class BootstrapOverlay {
  private container: HTMLDivElement | null = null;
  private root: ReactDOM.Root | null = null;

  private ensure(): ReactDOM.Root {
    if (this.root) return this.root;
    dismissSplash();
    const el = document.createElement("div");
    el.id = "codemux-remote-bootstrap";
    document.body.appendChild(el);
    this.container = el;
    this.root = ReactDOM.createRoot(el);
    return this.root;
  }

  render(node: React.ReactElement): void {
    this.ensure().render(<React.StrictMode>{node}</React.StrictMode>);
  }

  remove(): void {
    this.root?.unmount();
    this.container?.remove();
    this.root = null;
    this.container = null;
  }
}

// ── Orchestration ───────────────────────────────────────────────────

export async function bootstrapRemote(): Promise<void> {
  const baseUrl = window.location.origin;
  const host = window.location.host;
  const overlay = new BootstrapOverlay();
  const connection = useRemoteConnectionStore.getState();
  const appVersion = await fetchAppVersion(baseUrl);

  // Pre-mount: the connect loop drives re-pairing. Post-mount ("live"): a
  // revocation flips the indicator to offline and falls back to pairing.
  let phase: "connecting" | "live" = "connecting";

  function handleStatus(status: ConnectionStatus): void {
    // Connected → a quiet chip in the title bar's right cluster; a drop
    // degrades to the loud, centered "Reconnecting…" banner in its place.
    if (status === "reconnecting") connection.setReconnecting(host);
    else if (status === "connected") connection.setConnected(host);
  }
  function handleUnauthorized(): void {
    clearSession();
    if (phase === "live") {
      connection.setOffline("Remote access revoked");
      window.setTimeout(() => window.location.reload(), 1600);
    }
    // While still connecting, the loop below re-prompts for pairing.
  }

  // 1. Deep-link pairing.
  let waiting = false;
  const fragToken = readPairToken();
  if (fragToken) {
    clearPairFragment();
    try {
      const res = await pairDevice(baseUrl, fragToken);
      waiting = !res.approved;
    } catch {
      // Fall through to the manual pairing screen.
    }
  }

  // 2. Connect, prompting for pairing whenever no valid session exists.
  for (;;) {
    let cameFromPairing = false;
    if (!loadSession()) {
      const result = await new Promise<PairResult>((resolve) => {
        overlay.render(
          <PairingForm baseUrl={baseUrl} host={host} onPaired={resolve} />,
        );
      });
      waiting = !result.approved;
      cameFromPairing = true;
    }

    const { transport } = installShim({
      baseUrl,
      appVersion,
      getToken: () => loadSession()?.sessionToken ?? null,
      onStatusChange: handleStatus,
      onUnauthorized: handleUnauthorized,
      // Prefetch the bulk state snapshot in parallel with the WS handshake so
      // the app's first `get_app_state` resolves without a socket round-trip
      // (and re-sync on every reconnect). Failures fall back to the WS path.
      fetchSnapshot: () =>
        fetchSnapshot(baseUrl, () => loadSession()?.sessionToken ?? null),
    });

    if (waiting || cameFromPairing) {
      overlay.render(<ConnectingView host={host} waiting={waiting} />);
    }

    try {
      await transport.connect();
      break;
    } catch {
      // Session invalid/rejected → clear and re-prompt from the top.
      clearSession();
      waiting = false;
      transport.close();
    }
  }

  overlay.remove();
  phase = "live";
}
