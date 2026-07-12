/**
 * Pure helpers for the Remote Access settings section.
 *
 * Kept separate from the component so the security-hint logic, pending-
 * badge derivation, device labelling and countdown formatting can be unit
 * tested without rendering React.
 */
import type {
  WebRemoteBindScope,
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

// ── Bind scope (which interfaces the server listens on) ──────────────

export interface BindScopeOption {
  value: WebRemoteBindScope;
  /** Short control label. */
  label: string;
  /** One-line explanation shown under the control. */
  detail: string;
}

/** The three access-scope choices, in the order the segmented control
 *  renders them. Kept here (not inline in the component) so the copy and
 *  ordering are unit-testable and reused by the dev mock. */
export const BIND_SCOPE_OPTIONS: BindScopeOption[] = [
  {
    value: "all",
    label: "All networks",
    detail:
      "Listens on every network interface. Anyone on your LAN or mesh with the port can reach it.",
  },
  {
    value: "tailscale",
    label: "Tailscale only",
    detail:
      "Listens only on your Tailscale address (plus this device). Recommended on untrusted networks — the port isn't exposed to the local LAN.",
  },
  {
    value: "loopback",
    label: "This device only",
    detail:
      "Listens only on 127.0.0.1. Reachable just from this computer — e.g. tunnelled in over SSH.",
  },
];

/** The effective bind scope for a status payload, defaulting to `all` when
 *  the field is absent (a config persisted before the field existed). */
export function bindScopeOf(status: WebRemoteStatus | null): WebRemoteBindScope {
  return status?.bind_scope ?? "all";
}

/** The short control label for a scope, for interpolating into copy. Falls
 *  back to the raw value if the scope is somehow unknown. */
export function bindScopeLabel(scope: WebRemoteBindScope): string {
  return BIND_SCOPE_OPTIONS.find((o) => o.value === scope)?.label ?? scope;
}

// ── Rebind cutoff prediction (web client) ────────────────────────────
//
// Changing the bind scope from a *web* client rebinds the server, dropping
// this browser's socket. Whether it can reconnect depends on the origin it
// loaded from versus what the new scope still serves:
//
//   - `all`       binds every interface → every origin survives.
//   - `tailscale` binds the tailnet address(es) *plus* loopback → a loopback
//                 or tailnet/MagicDNS origin survives; a LAN-IP origin does not.
//   - `loopback`  binds `127.0.0.1` only → only a loopback origin survives.
//
// Predicting this before applying lets us confirm an intentional cutoff up
// front (rather than leaving the user stranded on an infinite reconnect).

/** How reachable the origin this client loaded from is, relative to the
 *  server's bind scopes. */
export type OriginReach = "loopback" | "tailnet" | "lan";

/**
 * Classify the host portion of the origin the web client loaded from
 * (`window.location.hostname`) into an {@link OriginReach} bucket. IPv6
 * literals may arrive bracket-wrapped (`[::1]`), so brackets are stripped
 * first. Anything not recognisably loopback or tailnet is treated as `lan`
 * — the most conservative bucket, so an unclassifiable origin errs toward
 * warning about a cutoff rather than silently stranding the device.
 */
export function classifyOriginHost(hostname: string): OriginReach {
  const host = (hostname || "")
    .trim()
    .replace(/^\[/, "")
    .replace(/\]$/, "")
    .toLowerCase();
  // Loopback: localhost, IPv4 127.0.0.0/8, IPv6 ::1.
  if (host === "localhost" || host.endsWith(".localhost")) return "loopback";
  if (host === "::1") return "loopback";
  if (/^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host)) return "loopback";
  // Tailscale MagicDNS name (`machine.tailnet.ts.net`).
  if (host.endsWith(".ts.net")) return "tailnet";
  // Tailscale IPv6 ULA prefix (`fd7a:115c:a1e0::/48`).
  if (host.startsWith("fd7a:115c:a1e0")) return "tailnet";
  // Tailnet CGNAT IPv4 range `100.64.0.0/10`.
  const v4 = /^(\d{1,3})\.(\d{1,3})\.\d{1,3}\.\d{1,3}$/.exec(host);
  if (v4) {
    const a = Number(v4[1]);
    const b = Number(v4[2]);
    if (a === 100 && b >= 64 && b <= 127) return "tailnet";
  }
  return "lan";
}

/** Whether an origin of the given reach is still served under `scope`. */
export function originSurvivesScope(
  reach: OriginReach,
  scope: WebRemoteBindScope,
): boolean {
  switch (scope) {
    case "all":
      return true;
    case "tailscale":
      return reach === "loopback" || reach === "tailnet";
    case "loopback":
      return reach === "loopback";
    default:
      return true;
  }
}

/** Convenience over {@link classifyOriginHost} + {@link originSurvivesScope}:
 *  would the current window origin (`hostname`) still reach the server after
 *  switching to `scope`? `false` means applying it cuts this device off. */
export function originHostSurvivesScope(
  hostname: string,
  scope: WebRemoteBindScope,
): boolean {
  return originSurvivesScope(classifyOriginHost(hostname), scope);
}

// ── Rebind-disconnect classification (web client) ────────────────────

/**
 * True when an invoke rejection is the transport's *disconnect* signal
 * rather than a backend-returned error. A port or scope change rebinds the
 * server, which drops this browser's socket before the
 * `web_remote_set_config` response can return — so on a web client the
 * invoke rejects with a transport-level "connection lost" / "not connected"
 * message even though the backend applied the change. The desktop path
 * (native IPC) never rebinds its own transport, so it never hits this.
 */
export function isRebindDisconnectError(err: unknown): boolean {
  const msg =
    err instanceof Error
      ? err.message
      : typeof err === "string"
        ? err
        : "";
  return /connection lost|not connected/i.test(msg);
}

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

// ── Endpoint grouping ────────────────────────────────────────────────

export type EndpointGroupId =
  | "this_device"
  | "local_network"
  | "tailscale"
  | "other";

export interface EndpointGroupMeta {
  id: EndpointGroupId;
  /** Section header. */
  title: string;
  /** One-line explanation shown under the header. */
  explanation: string;
  /** When true the group is tucked behind a default-closed disclosure. */
  collapsible: boolean;
}

/**
 * The display order + copy for the endpoint sections. The backend's coarse
 * `group` maps 1:1 onto these; anything unrecognised falls into "other".
 */
export const ENDPOINT_GROUPS: EndpointGroupMeta[] = [
  {
    id: "this_device",
    title: "This device",
    explanation: "Only this computer",
    collapsible: false,
  },
  {
    id: "local_network",
    title: "Local network",
    explanation: "Other devices on your home/office network",
    collapsible: false,
  },
  {
    id: "tailscale",
    title: "Tailscale",
    explanation:
      "Reach this machine from anywhere on your tailnet — set up Tailscale's HTTPS serve for a secure connection",
    collapsible: false,
  },
  {
    id: "other",
    title: "Other addresses",
    explanation: "Extra addresses that only work on some networks",
    collapsible: true,
  },
];

export interface EndpointGroupView extends EndpointGroupMeta {
  endpoints: WebRemoteEndpoint[];
}

const KNOWN_GROUP_IDS = new Set<string>(ENDPOINT_GROUPS.map((g) => g.id));

/**
 * Bucket endpoints into the display groups, preserving the backend's order
 * within each group and dropping groups that ended up empty. Any unknown
 * `group` value degrades into "other" so a future backend addition never
 * disappears from the UI.
 */
export function groupEndpoints(
  endpoints: WebRemoteEndpoint[],
): EndpointGroupView[] {
  const buckets = new Map<EndpointGroupId, WebRemoteEndpoint[]>();
  for (const e of endpoints) {
    const id: EndpointGroupId = KNOWN_GROUP_IDS.has(e.group)
      ? (e.group as EndpointGroupId)
      : "other";
    const list = buckets.get(id);
    if (list) list.push(e);
    else buckets.set(id, [e]);
  }
  return ENDPOINT_GROUPS.map((meta) => ({
    ...meta,
    endpoints: buckets.get(meta.id) ?? [],
  })).filter((g) => g.endpoints.length > 0);
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
 * reachable from the desktop itself) is the worst choice. Prefer the
 * backend's `recommended` endpoint (the best "from anywhere" option), then
 * the first reachable network endpoint, falling back to loopback only when
 * that is all that exists.
 */
export function pickPrimaryEndpoint(
  endpoints: WebRemoteEndpoint[],
): WebRemoteEndpoint | null {
  if (endpoints.length === 0) return null;
  const recommended = endpoints.find((e) => e.recommended);
  if (recommended) return recommended;
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
  // SQLite's datetime('now') yields UTC as "YYYY-MM-DD HH:MM:SS" with no
  // timezone designator; Date.parse would read that as *local* time and
  // skew the result by the machine's UTC offset. Normalize to ISO-8601 UTC.
  const normalized = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(iso)
    ? `${iso.replace(" ", "T")}Z`
    : iso;
  const t = Date.parse(normalized);
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
