/**
 * Hosted-relay bootstrap UI + orchestration.
 *
 * Rendered when the client is served from the hosted origin (see
 * `isHostedOrigin`). It walks the {@link HostedFlow} state machine —
 * **sign in → pick a device → connect over iroh** — and, once a device is
 * selected, wires the account device-registry into {@link installShim} via the
 * iroh {@link createIrohConnection} seams so the entire existing web client then
 * runs over the iroh pipe unchanged.
 *
 * The screens reuse the standalone pairing aesthetic (shared styles exported
 * from `bootstrap.tsx`) and the Stage-A password derivation
 * (`deriveAuthSecret`), so the hosted flow looks and feels like the LAN pairing
 * flow it replaces.
 */
import React from "react";
import {
  BootstrapOverlay,
  cardStyle,
  errorStyle,
  inputStyle,
  overlayStyle,
  primaryButtonStyle,
  switchLinkStyle,
} from "./bootstrap";
import { deriveAuthSecret } from "./auth-derivation";
import {
  DeviceRegistry,
  type DeviceRegistryError,
  type RegisteredDevice,
} from "./device-registry";
import {
  apiBaseUrl,
  HostedFlow,
  type HostedConnectHandlers,
  type HostedFlowDeps,
  type HostedState,
} from "./hosted";
import { createIrohConnection } from "./iroh-connection";
import { IrohWasmUnavailableError, loadIrohDialer } from "./iroh-wasm-loader";
import { installShim } from "./shim";
import type { ConnectionStatus } from "./transport";
import { useRemoteConnectionStore } from "./remote-connection-store";

const WASM_UNAVAILABLE_COPY =
  "Relay client unavailable — build it with scripts/build-iroh-wasm.sh, then redeploy.";

/**
 * Entry point for the hosted flow. Resolves once the app is connected over iroh
 * (so `main.tsx` can mount React); before that it owns a standalone overlay for
 * sign-in / device-pick / connecting.
 */
export async function bootstrapHosted(): Promise<void> {
  const base = apiBaseUrl();
  const registry = new DeviceRegistry({ baseUrl: base });
  const overlay = new BootstrapOverlay();
  const connection = useRemoteConnectionStore.getState();

  // Warm the wasm client in the background so a missing artifact surfaces fast.
  void loadIrohDialer().catch(() => {
    /* handled at connect time */
  });

  let phase: "connecting" | "live" = "connecting";

  function handleStatus(status: ConnectionStatus, deviceName: string): void {
    if (status === "reconnecting") connection.setReconnecting(deviceName);
    else if (status === "connected") connection.setConnected(deviceName);
  }

  function handleUnauthorized(): void {
    if (phase === "live") {
      connection.setOffline("Remote access revoked");
      window.setTimeout(() => window.location.reload(), 1600);
    }
  }

  const deps: HostedFlowDeps = {
    async signIn(email, password) {
      // Derive the AuthSecret locally — the raw password never leaves the page.
      const authSecret = await deriveAuthSecret(password, email);
      await registry.signIn(email, authSecret);
      return registry.listDevices();
    },
    connect(device, handlers) {
      return connectOverIroh({
        registry,
        device,
        handlers,
        onStatus: (s) => handleStatus(s, device.name),
        onUnauthorized: handleUnauthorized,
      });
    },
  };

  const flow = new HostedFlow(deps);
  const done = new Promise<void>((resolve) => {
    flow.subscribe((state) => {
      overlay.render(<HostedScreen state={state} flow={flow} apiHost={hostOf(base)} />);
      if (state.phase === "connected") resolve();
    });
  });
  // Initial paint.
  overlay.render(
    <HostedScreen state={flow.getState()} flow={flow} apiHost={hostOf(base)} />,
  );

  await done;
  overlay.remove();
  phase = "live";
}

interface ConnectArgs {
  registry: DeviceRegistry;
  device: RegisteredDevice;
  handlers: HostedConnectHandlers;
  onStatus(status: ConnectionStatus): void;
  onUnauthorized(): void;
}

/**
 * Install the shim over an iroh connection to `device` and resolve once the
 * first live connection is up. Rejects with a `{reason}` the {@link HostedFlow}
 * classifies (unauthorized → back to sign-in; anything else → back to the
 * picker).
 */
async function connectOverIroh(args: ConnectArgs): Promise<void> {
  let dialer;
  try {
    dialer = await loadIrohDialer();
  } catch (err) {
    if (err instanceof IrohWasmUnavailableError) {
      throw new Error(WASM_UNAVAILABLE_COPY);
    }
    throw err;
  }

  const conn = createIrohConnection({
    registry: args.registry,
    deviceId: args.device.id,
    dialer,
    onPending: args.handlers.onPending,
    onMintError: (err: DeviceRegistryError) => {
      // Transient mint failures (offline/network/server) → keep the picker's
      // "offline, retrying" copy; auth failures route through onUnauthorized.
      if (
        err.kind === "offline" ||
        err.kind === "network" ||
        err.kind === "server" ||
        err.kind === "rate_limited"
      ) {
        args.handlers.onOfflineRetry();
      }
    },
  });

  return new Promise<void>((resolve, reject) => {
    const { transport } = installShim({
      baseUrl: window.location.origin,
      appVersion: null,
      getToken: () => null,
      onStatusChange: args.onStatus,
      onUnauthorized: () => {
        args.onUnauthorized();
        reject({
          reason: "unauthorized",
          message: "Your account session expired. Sign in again.",
        });
      },
      deps: { fetchImpl: conn.fetchImpl, wsFactory: conn.wsFactory },
    });
    transport.connect().then(resolve, reject);
  });
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

// ── Screens ─────────────────────────────────────────────────────────

function HostedScreen(props: {
  state: HostedState;
  flow: HostedFlow;
  apiHost: string;
}): React.ReactElement {
  const { state, flow, apiHost } = props;
  let body: React.ReactElement;
  switch (state.phase) {
    case "signin":
      body = <SignInForm state={state} flow={flow} />;
      break;
    case "loading":
      body = <Spinner title="Loading devices" detail={`Reaching ${apiHost}…`} />;
      break;
    case "devices":
      body =
        state.devices.length === 0 ? (
          <NoDevices flow={flow} error={state.error} />
        ) : (
          <DevicePicker state={state} flow={flow} />
        );
      break;
    case "connecting":
      body = <ConnectingScreen state={state} />;
      break;
    case "connected":
    default:
      body = <Spinner title="Connected" detail="Loading…" />;
      break;
  }
  return (
    <div style={overlayStyle}>
      <div style={{ width: "100%", maxWidth: 420 }}>{body}</div>
    </div>
  );
}

function SignInForm(props: {
  state: HostedState;
  flow: HostedFlow;
}): React.ReactElement {
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const busy = props.state.busy;

  function submit(e: React.FormEvent): void {
    e.preventDefault();
    if (!email.trim() || !password) return;
    void props.flow.submitSignIn(email, password);
  }

  return (
    <form style={cardStyle} onSubmit={submit}>
      <div style={titleStyle}>Sign in to connect</div>
      <div style={subtitleStyle}>
        Use your Codemux account to reach a desktop you have set up for remote
        access. Your password is stretched on this device and never sent as-is.
      </div>
      <input
        type="email"
        value={email}
        autoFocus
        autoComplete="username"
        disabled={busy}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="Email"
        aria-label="Email"
        style={inputStyle}
      />
      <input
        type="password"
        value={password}
        autoComplete="current-password"
        disabled={busy}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password"
        aria-label="Password"
        style={{ ...inputStyle, marginTop: 10 }}
      />
      {props.state.error && (
        <div role="alert" style={errorStyle}>
          {props.state.error}
        </div>
      )}
      <button type="submit" disabled={busy} style={primaryButtonStyle(busy)}>
        {busy ? "Signing in…" : "Sign in"}
      </button>
    </form>
  );
}

function DevicePicker(props: {
  state: HostedState;
  flow: HostedFlow;
}): React.ReactElement {
  return (
    <div style={cardStyle}>
      <div style={titleStyle}>Choose a device</div>
      <div style={subtitleStyle}>
        Pick the desktop you want to connect to. Only desktops you have enabled
        for remote access appear here.
      </div>
      {props.state.error && (
        <div role="alert" style={{ ...errorStyle, marginBottom: 10 }}>
          {props.state.error}
        </div>
      )}
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {props.state.devices.map((device) => (
          <button
            key={device.id}
            type="button"
            onClick={() => void props.flow.select(device)}
            style={deviceRowStyle}
          >
            <span style={{ fontWeight: 600, fontSize: 13.5 }}>{device.name}</span>
            <span
              style={{
                fontSize: 11.5,
                color: "var(--muted-foreground, #9a9a97)",
              }}
            >
              {device.platform}
              {device.lastSeenAt ? ` · ${relativeSeen(device.lastSeenAt)}` : ""}
            </span>
          </button>
        ))}
      </div>
      <button
        type="button"
        style={switchLinkStyle}
        onClick={() => props.flow.reset()}
      >
        Sign in with a different account
      </button>
    </div>
  );
}

function NoDevices(props: {
  flow: HostedFlow;
  error: string | null;
}): React.ReactElement {
  return (
    <div style={cardStyle}>
      <div style={titleStyle}>No devices yet</div>
      <div style={subtitleStyle}>
        None of your desktops are set up for remote access. On a desktop running
        Codemux, open <strong>Settings → Remote Access</strong> and turn on
        relay mode, then refresh this page.
      </div>
      {props.error && (
        <div role="alert" style={errorStyle}>
          {props.error}
        </div>
      )}
      <button
        type="button"
        style={switchLinkStyle}
        onClick={() => props.flow.reset()}
      >
        Sign in with a different account
      </button>
    </div>
  );
}

function ConnectingScreen(props: { state: HostedState }): React.ReactElement {
  const name = props.state.selected?.name ?? "your device";
  let title: string;
  let detail: string;
  switch (props.state.connectStatus) {
    case "waiting-approval":
      title = "Waiting for approval";
      detail = `Approve this browser on ${name} to finish connecting.`;
      break;
    case "offline-retrying":
      title = "Device offline";
      detail = `${name} isn't reachable right now — retrying…`;
      break;
    case "connecting":
    default:
      title = "Connecting";
      detail = `Establishing a secure connection to ${name}…`;
      break;
  }
  return <Spinner title={title} detail={detail} />;
}

function Spinner(props: { title: string; detail: string }): React.ReactElement {
  return (
    <div style={{ textAlign: "center", maxWidth: 360, margin: "0 auto" }}>
      <style>{"@keyframes codemux-hosted-spin{to{transform:rotate(360deg)}}"}</style>
      <div
        style={{
          width: 26,
          height: 26,
          margin: "0 auto 18px",
          borderRadius: "50%",
          border: "2.5px solid var(--border, rgba(255,255,255,0.15))",
          borderTopColor: "var(--accent-ember, oklch(0.705 0.152 47))",
          animation: "codemux-hosted-spin 0.8s linear infinite",
        }}
      />
      <div style={{ fontSize: 14.5, fontWeight: 600, marginBottom: 6 }}>
        {props.title}
      </div>
      <div
        style={{
          fontSize: 12.5,
          lineHeight: 1.5,
          color: "var(--muted-foreground, #9a9a97)",
        }}
      >
        {props.detail}
      </div>
    </div>
  );
}

// ── Local styles ────────────────────────────────────────────────────

const titleStyle: React.CSSProperties = {
  fontSize: 17,
  fontWeight: 600,
  letterSpacing: "-0.01em",
  marginBottom: 6,
};

const subtitleStyle: React.CSSProperties = {
  fontSize: 13,
  lineHeight: 1.5,
  color: "var(--muted-foreground, #9a9a97)",
  marginBottom: 18,
};

const deviceRowStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  alignItems: "flex-start",
  textAlign: "left",
  padding: "11px 13px",
  cursor: "pointer",
  color: "var(--foreground, #e8e8e8)",
  background: "var(--background, #0C0C0E)",
  border: "1px solid var(--input, rgba(255,255,255,0.15))",
  borderRadius: 9,
};

/** Coarse "last seen" label for the device list. */
function relativeSeen(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const secs = Math.max(0, (Date.now() - then) / 1000);
  if (secs < 90) return "online";
  if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h ago`;
  return `${Math.round(secs / 86400)}d ago`;
}
