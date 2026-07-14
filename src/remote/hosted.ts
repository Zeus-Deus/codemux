/**
 * Hosted-relay bootstrap flow logic (framework-free, unit-testable).
 *
 * Drives the state machine the hosted web client walks before the real app
 * mounts: **sign in → list devices → pick a device → connect**. The React
 * surface (`hosted-bootstrap.tsx`) renders {@link HostedState} and forwards user
 * actions; every side-effecting operation (account sign-in, the iroh connect) is
 * injected as {@link HostedFlowDeps} so this module has no dependency on the
 * network, wasm, or React and can be tested end-to-end with fakes.
 */
import type { RegisteredDevice } from "./device-registry";

/** Where in the flow the user is. */
export type HostedPhase =
  | "signin" // account email/password
  | "loading" // fetching the device list
  | "devices" // pick a device (may be empty)
  | "connecting" // opening the iroh connection to the selected device
  | "connected"; // app is live (bootstrap resolves)

/** Sub-status while `connecting`, mirrored into the UI copy. */
export type HostedConnectStatus =
  | "connecting"
  | "waiting-approval" // desktop requires approval; polling
  | "offline-retrying"; // grant mint failing (device offline); backing off

export interface HostedState {
  phase: HostedPhase;
  /** Devices from the last successful list (empty array ⇒ "no devices"). */
  devices: RegisteredDevice[];
  /** The device being connected, when `phase === "connecting"|"connected"`. */
  selected: RegisteredDevice | null;
  /** Sign-in / connect error copy to surface, or `null`. */
  error: string | null;
  /** Sub-status detail while connecting. */
  connectStatus: HostedConnectStatus;
  /** A sign-in request is in flight (disables the form). */
  busy: boolean;
}

/** Callbacks the injected `connect` uses to report intermediate states. */
export interface HostedConnectHandlers {
  /** The desktop reported the session is pending approval. */
  onPending(): void;
  /** A grant mint is failing (device offline / transient); still retrying. */
  onOfflineRetry(): void;
  /** Back to a plain "connecting…" (e.g. a retry cleared the offline state). */
  onConnecting(): void;
}

/** Why a `connect` attempt ended unrecoverably. */
export type HostedConnectFailure =
  /** Account session invalid → return to sign-in. */
  | { reason: "unauthorized"; message: string }
  /** Grant/handshake rejected or wasm missing → return to the device list. */
  | { reason: "rejected"; message: string };

export interface HostedFlowDeps {
  /**
   * Authenticate the account (derive AuthSecret + sign in) and return the
   * account's registered devices. Throws with a user-facing message on failure.
   */
  signIn(email: string, password: string): Promise<RegisteredDevice[]>;
  /**
   * Connect the app to `device` over iroh. Resolves once the connection is live
   * (the app can mount). Uses `handlers` to surface pending/offline states.
   * Rejects with a {@link HostedConnectFailure} when the attempt is
   * unrecoverable.
   */
  connect(
    device: RegisteredDevice,
    handlers: HostedConnectHandlers,
  ): Promise<void>;
  /**
   * Kick off the GitHub OAuth redirect. Side-effecting (navigates the browser
   * away); the flow just marks itself busy first. Optional so existing callers
   * that only use email/password need not provide it.
   */
  beginGithubSignIn?(): void;
  /**
   * Finish a GitHub OAuth return: exchange the single-use `code` for the account
   * bearer, adopt it, and return the account's devices. Throws with a
   * user-facing message on failure. Optional (see {@link beginGithubSignIn}).
   */
  completeGithubSignIn?(code: string): Promise<RegisteredDevice[]>;
}

type Listener = (state: HostedState) => void;

const INITIAL: HostedState = {
  phase: "signin",
  devices: [],
  selected: null,
  error: null,
  connectStatus: "connecting",
  busy: false,
};

/**
 * The hosted bootstrap controller. Holds {@link HostedState}, exposes the two
 * user actions ({@link submitSignIn}, {@link select}) plus navigation, and
 * notifies subscribers on every transition.
 */
export class HostedFlow {
  private state: HostedState = { ...INITIAL };
  private readonly listeners = new Set<Listener>();

  constructor(private readonly deps: HostedFlowDeps) {}

  getState(): HostedState {
    return this.state;
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Attempt account sign-in; on success move to the device list. */
  async submitSignIn(email: string, password: string): Promise<void> {
    if (this.state.busy) return;
    this.set({ busy: true, error: null });
    let devices: RegisteredDevice[];
    try {
      devices = await this.deps.signIn(email, password);
    } catch (err) {
      this.set({
        busy: false,
        error: err instanceof Error ? err.message : "Sign-in failed.",
      });
      return;
    }
    this.set({ busy: false, phase: "devices", devices, error: null });
  }

  /**
   * Begin GitHub OAuth. Marks the form busy (so it disables while the browser
   * navigates away) and delegates the redirect to the injected dep.
   */
  startGithubSignIn(): void {
    if (this.state.busy) return;
    if (!this.deps.beginGithubSignIn) return;
    this.set({ busy: true, error: null });
    this.deps.beginGithubSignIn();
  }

  /**
   * Resume after a GitHub OAuth return: exchange the code, then move to the
   * device list on success or back to sign-in (with the reason) on failure.
   * Mirrors {@link submitSignIn} so both auth paths converge on the same state.
   */
  async resumeGithubSignIn(code: string): Promise<void> {
    if (this.state.busy) return;
    if (!this.deps.completeGithubSignIn) return;
    this.set({ busy: true, error: null, phase: "signin" });
    let devices: RegisteredDevice[];
    try {
      devices = await this.deps.completeGithubSignIn(code);
    } catch (err) {
      this.set({
        busy: false,
        phase: "signin",
        error: err instanceof Error ? err.message : "GitHub sign-in failed.",
      });
      return;
    }
    this.set({ busy: false, phase: "devices", devices, error: null });
  }

  /** Surface an error on the sign-in screen (e.g. a failed OAuth return). */
  failSignIn(message: string): void {
    this.set({ phase: "signin", busy: false, error: message });
  }

  /** Select a device and connect the app to it over iroh. */
  async select(device: RegisteredDevice): Promise<void> {
    this.set({
      phase: "connecting",
      selected: device,
      connectStatus: "connecting",
      error: null,
    });
    const handlers: HostedConnectHandlers = {
      onPending: () => this.set({ connectStatus: "waiting-approval" }),
      onOfflineRetry: () => this.set({ connectStatus: "offline-retrying" }),
      onConnecting: () => this.set({ connectStatus: "connecting" }),
    };
    try {
      await this.deps.connect(device, handlers);
    } catch (err) {
      this.onConnectFailed(err);
      return;
    }
    this.set({ phase: "connected", error: null });
  }

  /** Return to the device list (e.g. from a connect error). */
  backToDevices(): void {
    this.set({ phase: "devices", selected: null, error: null });
  }

  /** Return to sign-in (session expired / user chose to switch account). */
  reset(): void {
    this.state = { ...INITIAL };
    this.emit();
  }

  private onConnectFailed(err: unknown): void {
    const failure = asFailure(err);
    if (failure.reason === "unauthorized") {
      // Session gone → back to sign-in with the reason.
      this.state = { ...INITIAL, error: failure.message };
      this.emit();
      return;
    }
    // Rejected grant / wasm missing / device gone → back to the picker.
    this.set({ phase: "devices", selected: null, error: failure.message });
  }

  private set(patch: Partial<HostedState>): void {
    this.state = { ...this.state, ...patch };
    this.emit();
  }

  private emit(): void {
    for (const fn of this.listeners) fn(this.state);
  }
}

function asFailure(err: unknown): HostedConnectFailure {
  if (
    err &&
    typeof err === "object" &&
    "reason" in err &&
    (err as { reason: unknown }).reason === "unauthorized"
  ) {
    return {
      reason: "unauthorized",
      message:
        (err as { message?: string }).message ??
        "Your session expired. Sign in again.",
    };
  }
  return {
    reason: "rejected",
    message: err instanceof Error ? err.message : "Could not connect.",
  };
}

/**
 * Detect the hosted origin: an explicit build flag, an `?hosted` query param, or
 * the canonical hosted hostname. Injectable location/env for tests.
 */
export function isHostedOrigin(
  loc: { hostname: string; search: string } = window.location,
  env: Record<string, string | boolean | undefined> = import.meta.env as never,
): boolean {
  if (env?.VITE_CODEMUX_HOSTED === "true" || env?.VITE_CODEMUX_HOSTED === true) {
    return true;
  }
  try {
    if (new URLSearchParams(loc.search).has("hosted")) return true;
  } catch {
    /* ignore malformed search */
  }
  return loc.hostname === "app.codemux.org";
}

/** Control-plane base URL. Overridable at build time; defaults to production. */
export function apiBaseUrl(
  env: Record<string, string | undefined> = import.meta.env as never,
): string {
  const configured = env?.VITE_CODEMUX_API_BASE;
  return (configured && configured.replace(/\/+$/, "")) || "https://api.codemux.org";
}
