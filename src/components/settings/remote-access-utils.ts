/**
 * Pure helpers for the Remote Access settings section.
 *
 * Kept separate from the component so the security-hint logic, pending-
 * badge derivation, device labelling and countdown formatting can be unit
 * tested without rendering React.
 */
import type {
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

// ── Endpoint security ────────────────────────────────────────────────

/**
 * True when a browser loading this endpoint gets a *secure context*
 * (clipboard, notifications, service workers all work). Loopback over
 * plain HTTP qualifies (the backend reports `secure: true` for it); any
 * `https://` origin qualifies; everything else is a plain-HTTP origin
 * where those browser APIs are disabled.
 */
export function endpointIsSecure(ep: WebRemoteEndpoint): boolean {
  return ep.secure || /^https:/i.test(ep.url);
}

export interface EndpointSecurityHint {
  secure: boolean;
  /** One-word status word for the badge. */
  badge: string;
  /** Full sentence for the row's helper text. */
  detail: string;
}

/**
 * The per-endpoint security note surfaced next to each URL. Secure
 * origins are marked "Secure"; plain-HTTP origins carry the explicit
 * degradation warning (clipboard + notifications disabled) so the user
 * understands why the web client feels limited there.
 */
export function endpointSecurityHint(ep: WebRemoteEndpoint): EndpointSecurityHint {
  if (endpointIsSecure(ep)) {
    return {
      secure: true,
      badge: "Secure",
      detail: "Secure origin — clipboard and notifications work in the browser.",
    };
  }
  return {
    secure: false,
    badge: "Limited",
    detail:
      "Insecure origin — browser clipboard and notifications are disabled here. Serve over HTTPS (your mesh VPN's serve feature or a reverse proxy) to lift the limit.",
  };
}

// ── Pairing URL composition ──────────────────────────────────────────

/** Compose the full auto-pair URL for an endpoint: `<url>/#pair=<token>`. */
export function composePairUrl(
  endpoint: WebRemoteEndpoint,
  pairing: WebRemotePairingInfo,
): string {
  // `url_path` is a root-relative `/#pair=...`; endpoint.url has no
  // trailing slash, so a direct concat yields `http://host:port/#pair=...`.
  return `${endpoint.url}${pairing.url_path}`;
}

/**
 * The endpoint a QR code / primary pairing link should target. A QR is
 * meant to be scanned from a *phone*, so loopback (`127.0.0.1`, only
 * reachable from the desktop itself) is the worst choice — prefer the
 * first reachable network endpoint, falling back to loopback only when
 * that is all that exists.
 */
export function pickPrimaryEndpoint(
  endpoints: WebRemoteEndpoint[],
): WebRemoteEndpoint | null {
  if (endpoints.length === 0) return null;
  const networked = endpoints.find((e) => e.kind !== "loopback");
  return networked ?? endpoints[0];
}

// ── Session partitioning ─────────────────────────────────────────────

/** Devices still awaiting approval (approval mode on). */
export function pendingSessions(
  status: WebRemoteStatus | null,
): WebRemoteSessionView[] {
  return status?.sessions.filter((s) => !s.approved) ?? [];
}

/** Approved (paired) devices. */
export function approvedSessions(
  status: WebRemoteStatus | null,
): WebRemoteSessionView[] {
  return status?.sessions.filter((s) => s.approved) ?? [];
}

/** Count of live-connected devices — the backend's authoritative
 *  `connected_sessions` (distinct devices with a live WebSocket; connected
 *  implies approved). Read straight off the status payload rather than
 *  recomputed so the badge always matches the server's own count. */
export function connectedSessionCount(status: WebRemoteStatus | null): number {
  return status?.connected_sessions ?? 0;
}

/**
 * The ids of devices that are pending in `next` but were NOT pending in
 * `prev` — i.e. brand-new approval requests. Drives the "new device
 * requesting access" toast + badge. A device that flips from approved
 * back to pending (impossible today, but cheap to be correct about) also
 * counts.
 */
export function newlyPendingSessionIds(
  prev: WebRemoteSessionView[],
  next: WebRemoteSessionView[],
): string[] {
  const prevPending = new Set(
    prev.filter((s) => !s.approved).map((s) => s.id),
  );
  return next
    .filter((s) => !s.approved && !prevPending.has(s.id))
    .map((s) => s.id);
}

// ── Device labelling ─────────────────────────────────────────────────

export type DeviceKind = "phone" | "tablet" | "laptop" | "desktop" | "unknown";

export interface DeviceDescription {
  /** Best display name: the reported name, else a derived platform label. */
  title: string;
  /** Short OS/browser summary, e.g. "iOS · Safari". Empty when unknown. */
  platform: string;
  kind: DeviceKind;
}

/**
 * Best-effort friendly description of a paired device from the name it
 * reported at pairing time plus its `User-Agent`. Never throws; unknown
 * agents degrade to "Unknown device".
 */
export function describeDevice(
  name: string | null,
  userAgent: string | null,
): DeviceDescription {
  const ua = userAgent ?? "";
  const os = detectOs(ua);
  const browser = detectBrowser(ua);
  const kind = detectKind(ua, os);
  const platform = [os.label, browser].filter(Boolean).join(" · ");
  const title =
    (name && name.trim()) ||
    (os.label ? `${os.label} device` : "Unknown device");
  return { title, platform, kind };
}

interface OsInfo {
  label: string;
  kind: DeviceKind;
}

function detectOs(ua: string): OsInfo {
  const s = ua.toLowerCase();
  if (/iphone/.test(s)) return { label: "iOS", kind: "phone" };
  if (/ipad/.test(s)) return { label: "iPadOS", kind: "tablet" };
  if (/android/.test(s)) {
    return { label: "Android", kind: /mobile/.test(s) ? "phone" : "tablet" };
  }
  if (/mac os x|macintosh/.test(s)) return { label: "macOS", kind: "laptop" };
  if (/windows/.test(s)) return { label: "Windows", kind: "desktop" };
  if (/cros/.test(s)) return { label: "ChromeOS", kind: "laptop" };
  if (/linux/.test(s)) return { label: "Linux", kind: "desktop" };
  return { label: "", kind: "unknown" };
}

function detectBrowser(ua: string): string {
  const s = ua.toLowerCase();
  // Order matters: Edge/Chrome both contain "chrome"; Safari appears in
  // Chrome UAs too, so sniff the more specific tokens first.
  if (/edg\//.test(s)) return "Edge";
  if (/firefox\//.test(s)) return "Firefox";
  if (/chrome\//.test(s)) return "Chrome";
  if (/safari\//.test(s)) return "Safari";
  return "";
}

function detectKind(ua: string, os: OsInfo): DeviceKind {
  if (os.kind !== "unknown") return os.kind;
  return /mobile/i.test(ua) ? "phone" : "unknown";
}

// ── Time formatting ──────────────────────────────────────────────────

/** `mm:ss` for a non-negative millisecond duration; clamps at `0:00`. */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Milliseconds until `expiresAtIso`, floored at 0. `NaN`-safe (returns 0
 *  for an unparseable timestamp). */
export function msUntil(expiresAtIso: string, now: number = Date.now()): number {
  const t = Date.parse(expiresAtIso);
  if (Number.isNaN(t)) return 0;
  return Math.max(0, t - now);
}

/**
 * Compact relative time for "last seen" / "paired" rows: "just now",
 * "3m ago", "2h ago", "5d ago", falling back to a locale date for older
 * timestamps. Returns "never" for a null input.
 */
export function relativeTime(
  iso: string | null,
  now: number = Date.now(),
): string {
  if (!iso) return "never";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "unknown";
  const diff = now - t;
  if (diff < 0) return "just now";
  const sec = Math.floor(diff / 1000);
  if (sec < 45) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d ago`;
  return new Date(t).toLocaleDateString();
}

// ── Port validation ──────────────────────────────────────────────────

export interface PortValidation {
  valid: boolean;
  value: number | null;
  error: string | null;
}

/**
 * Validate a port field. Accepts 1024–65535 (ports below 1024 need root
 * on Unix and are a footgun for a user-toggled server).
 */
export function validatePort(raw: string): PortValidation {
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) {
    return { valid: false, value: null, error: "Enter a number." };
  }
  const n = Number(trimmed);
  if (n < 1024 || n > 65535) {
    return { valid: false, value: null, error: "Use a port between 1024 and 65535." };
  }
  return { valid: true, value: n, error: null };
}
