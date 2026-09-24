import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Check,
  Copy,
  ExternalLink,
  Globe,
  Laptop,
  Link2,
  Loader2,
  MonitorSmartphone,
  RefreshCw,
  Server,
  ShieldAlert,
  ShieldCheck,
  Smartphone,
  Tablet,
  Trash2,
  Wifi,
  WifiOff,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { isHostedOrigin } from "@/remote/hosted";
import { useRemoteConnectionStore } from "@/remote/remote-connection-store";
import {
  webRemoteApproveSession,
  webRemoteCreatePairing,
  webRemoteDisable,
  webRemoteEnable,
  webRemoteListEndpoints,
  webRemoteRegistrationStatus,
  webRemoteRejectSession,
  webRemoteRetry,
  webRemoteRevokeSession,
  webRemoteSetConfig,
  webRemoteStatus,
} from "@/tauri/commands";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import type {
  WebRemoteBindScope,
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteRegistrationStatus,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

import { useQrSvg } from "./use-qr-svg";
import {
  approvedSessions,
  BIND_SCOPE_OPTIONS,
  bindScopeLabel,
  bindScopeOf,
  composePairUrl,
  connectedSessionCount,
  describeDevice,
  describeExposure,
  endpointSecurityHint,
  formatCountdown,
  groupEndpoints,
  isRebindDisconnectError,
  lanEnabledOf,
  lanExposurePhrase,
  msUntil,
  newlyPendingSessionIds,
  originHostSurvivesScope,
  pendingSessions,
  pickPrimaryEndpoint,
  relativeTime,
  validatePort,
  type DeviceKind,
  type EndpointGroupView,
} from "./remote-access-utils";
import { Eyebrow } from "@/components/ui/eyebrow";

// ── Small presentational bits ────────────────────────────────────────

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      aria-label={label}
      title={label}
      className="shrink-0 text-muted-foreground hover:text-foreground"
      onClick={async () => {
        const ok = await copyToClipboard(text);
        if (ok) {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1400);
          toast.success("Copied to clipboard");
        } else {
          toast.error(COPY_FAILED_MESSAGE);
        }
      }}
    >
      {copied ? (
        <Check className="size-3.5 text-status-open" />
      ) : (
        <Copy className="size-3.5" />
      )}
    </Button>
  );
}

function DeviceIcon({ kind, className }: { kind: DeviceKind; className?: string }) {
  const Icon =
    kind === "phone"
      ? Smartphone
      : kind === "tablet"
        ? Tablet
        : kind === "laptop"
          ? Laptop
          : kind === "desktop"
            ? MonitorSmartphone
            : MonitorSmartphone;
  return <Icon className={className} />;
}

function SubHeading({ children }: { children: React.ReactNode }) {
  return (
    <Eyebrow>
      {children}
    </Eyebrow>
  );
}

// ── Endpoint row ─────────────────────────────────────────────────────

function EndpointRow({ endpoint }: { endpoint: WebRemoteEndpoint }) {
  const hint = endpointSecurityHint(endpoint);
  return (
    <div className="flex items-start gap-3 py-2.5">
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <code className="truncate font-mono text-body text-foreground">
            {endpoint.url}
          </code>
          {endpoint.recommended && (
            <Badge
              variant="outline"
              className="border-accent-ember/30 bg-accent-ember/10 text-caption font-medium text-accent-ember"
            >
              Recommended
            </Badge>
          )}
          {hint.secure ? (
            <Badge
              variant="outline"
              className="gap-1 border-status-open/30 bg-status-open/10 text-caption text-status-open"
            >
              <ShieldCheck className="size-3" />
              {hint.badge}
            </Badge>
          ) : (
            <Badge
              variant="outline"
              className="gap-1 border-status-working/30 bg-status-working/10 text-caption text-status-working"
            >
              <ShieldAlert className="size-3" />
              {hint.badge}
            </Badge>
          )}
        </div>
        <p className="text-body-sm leading-relaxed text-muted-foreground/80">
          {hint.detail}
        </p>
      </div>
      <CopyButton text={endpoint.url} label={`Copy ${endpoint.url}`} />
    </div>
  );
}

// ── Grouped endpoint list ────────────────────────────────────────────
//
// The backend hands back a curated set already sorted into coarse groups
// (this device / local network / Tailscale / other). We render each group
// under a labelled header with a one-line explanation; the catch-all
// "other" group is tucked behind a default-closed disclosure so stray IPv6
// and link-local addresses don't clutter the common case.

function EndpointGroupRows({ endpoints }: { endpoints: WebRemoteEndpoint[] }) {
  return (
    <div className="divide-y divide-border/50">
      {endpoints.map((e) => (
        <EndpointRow key={`${e.kind}:${e.host}`} endpoint={e} />
      ))}
    </div>
  );
}

function EndpointGroupBlock({ group }: { group: EndpointGroupView }) {
  if (group.collapsible) {
    return (
      <details className="group rounded-md border border-border/50 bg-muted/20 px-3 py-2">
        <summary className="flex cursor-pointer list-none items-center gap-1.5 text-body font-semibold text-foreground marker:content-none">
          <span className="text-muted-foreground/70 transition-transform duration-150 group-open:rotate-90">
            ›
          </span>
          {group.title}
          <span className="font-normal text-muted-foreground/70">
            ({group.endpoints.length})
          </span>
        </summary>
        <div className="mt-1.5 space-y-1">
          <p className="text-body-sm leading-relaxed text-muted-foreground/70">
            {group.explanation}
          </p>
          <EndpointGroupRows endpoints={group.endpoints} />
        </div>
      </details>
    );
  }
  return (
    <div className="space-y-1">
      <p className="text-body font-semibold text-foreground">
        {group.title}
      </p>
      <p className="text-body-sm leading-relaxed text-muted-foreground/70">
        {group.explanation}
      </p>
      <EndpointGroupRows endpoints={group.endpoints} />
    </div>
  );
}

function GroupedEndpoints({
  endpoints,
}: {
  endpoints: WebRemoteEndpoint[];
}) {
  const groups = useMemo(() => groupEndpoints(endpoints), [endpoints]);
  return (
    <div className="space-y-4">
      {groups.map((g) => (
        <EndpointGroupBlock key={g.id} group={g} />
      ))}
    </div>
  );
}

// ── Pairing panel ────────────────────────────────────────────────────

function PairingPanel({
  pairing,
  endpoints,
  onRegenerate,
  regenerating,
}: {
  pairing: WebRemotePairingInfo;
  endpoints: WebRemoteEndpoint[];
  onRegenerate: () => void;
  regenerating: boolean;
}) {
  const [selectedHost, setSelectedHost] = useState<string | null>(
    () => pickPrimaryEndpoint(endpoints)?.host ?? null,
  );
  const [remainingMs, setRemainingMs] = useState(() =>
    msUntil(pairing.expires_at),
  );

  // Re-pick a sensible default whenever the endpoint set changes (e.g. the
  // server just came up and enumerated its interfaces).
  useEffect(() => {
    if (!endpoints.some((e) => e.host === selectedHost)) {
      setSelectedHost(pickPrimaryEndpoint(endpoints)?.host ?? null);
    }
  }, [endpoints, selectedHost]);

  // 1 Hz countdown.
  useEffect(() => {
    setRemainingMs(msUntil(pairing.expires_at));
    const timer = window.setInterval(() => {
      setRemainingMs(msUntil(pairing.expires_at));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [pairing.expires_at]);

  const selected =
    endpoints.find((e) => e.host === selectedHost) ??
    pickPrimaryEndpoint(endpoints);
  const fullUrl = selected ? composePairUrl(selected, pairing) : null;
  const qrSvg = useQrSvg(fullUrl);
  const expired = remainingMs <= 0;

  // Same curated grouping as the "Reachable at" list: real endpoints inline,
  // stray "other" addresses collapsed behind a disclosure. Docker/virtual
  // interfaces never reach the frontend, so they can't appear here either.
  const grouped = useMemo(() => groupEndpoints(endpoints), [endpoints]);
  const inlineEndpoints = grouped
    .filter((g) => !g.collapsible)
    .flatMap((g) => g.endpoints);
  const otherEndpoints = grouped
    .filter((g) => g.collapsible)
    .flatMap((g) => g.endpoints);

  const renderChip = (e: WebRemoteEndpoint) => (
    <button
      key={`${e.kind}:${e.host}`}
      type="button"
      onClick={() => setSelectedHost(e.host)}
      className={cn(
        "rounded-full border px-2.5 py-1 text-label font-medium transition-colors duration-150",
        e.host === selectedHost
          ? "border-accent-ember/40 bg-accent-ember/10 text-accent-ember"
          : "border-border/60 text-muted-foreground hover:text-foreground",
      )}
    >
      {e.kind === "loopback" ? "This device" : e.host}
    </button>
  );

  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
      <div className="flex flex-col gap-4 sm:flex-row">
        {/* QR plate — always dark-on-white for camera scannability. */}
        <div className="shrink-0 self-center sm:self-start">
          <div
            className={cn(
              "flex h-[168px] w-[168px] items-center justify-center rounded-lg bg-white p-2.5 shadow-sm ring-1 ring-black/5",
              expired && "opacity-30",
            )}
          >
            {qrSvg ? (
              <div
                className="h-full w-full [&>svg]:h-full [&>svg]:w-full"
                // Self-generated, trusted SVG string.
                dangerouslySetInnerHTML={{ __html: qrSvg }}
                aria-label="Pairing QR code"
                role="img"
              />
            ) : (
              <span className="text-label text-muted-foreground">No endpoint</span>
            )}
          </div>
        </div>

        <div className="min-w-0 flex-1 space-y-3">
          <div>
            <div className="flex items-center justify-between gap-2">
              <p className="text-body font-semibold text-foreground">
                Scan or share this link
              </p>
              <span
                className={cn(
                  "font-mono text-body-sm tabular-nums",
                  expired ? "text-status-attention" : "text-muted-foreground",
                )}
              >
                {expired ? "Expired" : `Expires in ${formatCountdown(remainingMs)}`}
              </span>
            </div>
            <p className="mt-1 text-body-sm leading-relaxed text-muted-foreground/80">
              One-time link — it pairs a single device, then can't be reused.
              Open it on your phone or laptop to connect.
            </p>
          </div>

          {/* Endpoint chooser — a QR is scanned from a phone, so it should
              target a reachable network address, not loopback. Grouped the
              same way as "Reachable at": inline chips for the useful
              endpoints, an "Other addresses" disclosure for the rest. */}
          {endpoints.length > 1 && (
            <div className="space-y-1.5">
              {inlineEndpoints.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {inlineEndpoints.map(renderChip)}
                </div>
              )}
              {otherEndpoints.length > 0 && (
                <details className="group">
                  <summary className="flex w-fit cursor-pointer list-none items-center gap-1 text-label font-medium text-muted-foreground marker:content-none hover:text-foreground">
                    <span className="transition-transform duration-150 group-open:rotate-90">
                      ›
                    </span>
                    Other addresses ({otherEndpoints.length})
                  </summary>
                  <div className="mt-1.5 flex flex-wrap gap-1.5">
                    {otherEndpoints.map(renderChip)}
                  </div>
                </details>
              )}
            </div>
          )}

          {fullUrl && (
            <div className="flex items-center gap-2 rounded-md border border-border/60 bg-background/60 px-2.5 py-1.5">
              <Link2 className="size-3.5 shrink-0 text-muted-foreground" />
              <code className="min-w-0 flex-1 truncate font-mono text-body-sm text-foreground">
                {fullUrl}
              </code>
              <CopyButton text={fullUrl} label="Copy pairing link" />
            </div>
          )}

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={regenerating}
              onClick={onRegenerate}
            >
              <RefreshCw
                className={cn("size-3.5", regenerating && "animate-spin")}
              />
              {expired ? "Generate new link" : "Regenerate"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Pending-approval row ─────────────────────────────────────────────

function PendingRow({
  session,
  onApprove,
  onReject,
  busy,
}: {
  session: WebRemoteSessionView;
  onApprove: () => void;
  onReject: () => void;
  busy: boolean;
}) {
  const d = describeDevice(session.name, session.user_agent);
  return (
    <div className="flex items-center gap-3 rounded-lg border border-status-working/30 bg-status-working/[0.07] px-3.5 py-3">
      <DeviceIcon kind={d.kind} className="size-4 shrink-0 text-status-working" />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <p className="truncate text-body font-semibold text-foreground">
            {d.title}
          </p>
          {session.source === "account" && (
            <Badge
              variant="outline"
              className="border-border/60 text-caption text-muted-foreground/80"
            >
              Account
            </Badge>
          )}
        </div>
        <p className="truncate text-body-sm text-muted-foreground/80">
          {d.platform || "Unknown platform"} ·{" "}
          {session.source === "account"
            ? "signed in and awaiting approval"
            : "asked to connect"}{" "}
          {relativeTime(session.created_at)}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button
          type="button"
          size="sm"
          className="bg-status-open/90 text-status-open-foreground hover:bg-status-open"
          disabled={busy}
          onClick={onApprove}
        >
          <Check className="size-3.5" />
          Approve
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="text-muted-foreground hover:text-status-attention"
          disabled={busy}
          onClick={onReject}
        >
          Reject
        </Button>
      </div>
    </div>
  );
}

// ── Paired-device row ────────────────────────────────────────────────

function DeviceRow({
  session,
  onRevoke,
  busy,
}: {
  session: WebRemoteSessionView;
  onRevoke: () => void;
  busy: boolean;
}) {
  const d = describeDevice(session.name, session.user_agent);
  const isAccount = session.source === "account";
  return (
    <div className="flex items-center gap-3 py-3">
      <div className="relative shrink-0">
        <DeviceIcon kind={d.kind} className="size-4 text-muted-foreground" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <p className="truncate text-body font-semibold text-foreground">
            {d.title}
          </p>
          <Badge
            variant="outline"
            className="border-border/60 text-caption text-muted-foreground/80"
          >
            {isAccount ? "Account" : "Paired"}
          </Badge>
          {session.connected ? (
            <span className="flex items-center gap-1 text-label font-medium text-status-open">
              <span className="inline-block size-1.5 rounded-full bg-status-open" />
              Connected
            </span>
          ) : (
            <span className="flex items-center gap-1 text-label text-muted-foreground/70">
              <span className="inline-block size-1.5 rounded-full bg-muted-foreground/40" />
              Offline
            </span>
          )}
        </div>
        <p className="truncate text-body-sm text-muted-foreground/80">
          {d.platform || "Unknown platform"} ·{" "}
          {isAccount ? "signed in" : "paired"} {relativeTime(session.created_at)}{" "}
          · last seen {relativeTime(session.last_seen_at)}
        </p>
      </div>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="shrink-0 text-muted-foreground hover:text-status-attention"
        disabled={busy}
        onClick={onRevoke}
      >
        <Trash2 className="size-3.5" />
        Revoke
      </Button>
    </div>
  );
}

// ── Ways-to-connect card ─────────────────────────────────────────────
//
// One card per way in. Each owns its switch, its settings, and its live
// state, so every control on the page is visually scoped to the one
// transport it changes.

function WayCard({
  icon: Icon,
  title,
  badge,
  description,
  checked,
  onCheckedChange,
  disabled,
  switchLabel,
  children,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  badge?: React.ReactNode;
  description: React.ReactNode;
  checked: boolean;
  onCheckedChange: (next: boolean) => void;
  disabled?: boolean;
  switchLabel: string;
  children?: React.ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className={cn(
        "rounded-lg border p-4 transition-colors duration-150",
        checked ? "border-border bg-muted/20" : "border-border/60",
      )}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 flex-1 gap-3">
          <Icon
            className={cn(
              "mt-0.5 size-4 shrink-0",
              checked ? "text-accent-ember" : "text-muted-foreground",
            )}
          />
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-body font-semibold text-foreground">{title}</h3>
              {badge}
            </div>
            <p className="text-body-sm leading-relaxed text-muted-foreground/80">
              {description}
            </p>
          </div>
        </div>
        <Switch
          checked={checked}
          onCheckedChange={onCheckedChange}
          disabled={disabled}
          aria-label={switchLabel}
        />
      </div>
      {checked && children ? (
        <div className="mt-4 space-y-4 border-t border-border/60 pt-4 sm:ml-7">
          {children}
        </div>
      ) : null}
    </section>
  );
}

type LiveTone = "ok" | "pending" | "error";

/** A dot + label reporting what a transport is actually doing right now. */
function LiveState({ tone, children }: { tone: LiveTone; children: React.ReactNode }) {
  return (
    <span
      role="status"
      className={cn(
        "inline-flex items-center gap-1.5 text-label font-medium",
        tone === "ok" && "text-status-open",
        tone === "pending" && "text-status-working",
        tone === "error" && "text-status-attention",
      )}
    >
      {tone === "pending" ? (
        <Loader2 className="size-3 animate-spin" />
      ) : (
        <span
          className={cn(
            "inline-block size-1.5 rounded-full",
            tone === "ok" ? "bg-status-open" : "bg-status-attention",
          )}
        />
      )}
      {children}
    </span>
  );
}

/** A transport that is switched on but couldn't start: the real reason, plus
 *  a retry. Replaces any optimistic "starting…" copy. */
function FailureCallout({
  title,
  reason,
  hint,
  onRetry,
  retrying,
  retryLabel,
}: {
  title: string;
  reason: string;
  hint?: string;
  onRetry: () => void;
  retrying: boolean;
  retryLabel: string;
}) {
  return (
    <div
      role="alert"
      className="flex items-start gap-2.5 rounded-lg border border-status-attention/40 bg-status-attention/[0.08] px-3.5 py-3"
    >
      <ShieldAlert className="mt-0.5 size-4 shrink-0 text-status-attention" />
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-body font-medium text-status-attention">{title}</p>
        <p className="break-words font-mono text-body-sm text-foreground/90">
          {reason}
        </p>
        {hint && (
          <p className="text-body-sm leading-relaxed text-muted-foreground/85">
            {hint}
          </p>
        )}
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="shrink-0"
        disabled={retrying}
        onClick={onRetry}
        aria-label={retryLabel}
      >
        <RefreshCw className={cn("size-3.5", retrying && "animate-spin")} />
        Retry
      </Button>
    </div>
  );
}

function AttentionNote({ children }: { children: React.ReactNode }) {
  return (
    <div
      role="status"
      className="flex items-start gap-2.5 rounded-lg border border-status-attention/40 bg-status-attention/[0.08] px-3.5 py-3 text-body leading-relaxed text-status-attention"
    >
      <ShieldAlert className="mt-0.5 size-4 shrink-0" />
      <span>{children}</span>
    </div>
  );
}

/** A labelled switch row inside a card. */
function SettingRow({
  title,
  detail,
  checked,
  onCheckedChange,
  disabled,
  switchLabel,
}: {
  title: string;
  detail: React.ReactNode;
  checked: boolean;
  onCheckedChange: (next: boolean) => void;
  disabled?: boolean;
  switchLabel: string;
}) {
  return (
    <div className="flex items-center justify-between gap-8">
      <div className="min-w-0 space-y-1">
        <p className="text-body font-medium leading-tight text-foreground">{title}</p>
        <p className="text-body-sm leading-relaxed text-muted-foreground/80">{detail}</p>
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onCheckedChange}
        disabled={disabled}
        aria-label={switchLabel}
      />
    </div>
  );
}

// ── Remote rebind lifecycle ──────────────────────────────────────────
//
// A port or scope change from a browser connected over the *network listener*
// rebinds that listener, dropping this browser's socket before
// `web_remote_set_config` can answer — so the invoke rejects with a transport
// disconnect, NOT because the change failed. Rather than surfacing that as an
// error and snapping the control back, we reflect the requested value
// optimistically and drive a small lifecycle:
//
//   applying     → set_config in flight (socket may still be up).
//   reconnecting → the expected disconnect landed; the shim's reconnect loop
//                  is re-establishing. On success we refetch status and
//                  reconcile the control to the server's persisted value.
//   cutoff       → this device can't come back (the new scope excludes its
//                  origin, or the reconnect never succeeded). A clear terminal
//                  message replaces the endless spinner.
//
// The desktop never rebinds its own IPC, and a browser on app.codemux.org
// rides the relay (which a LAN rebind doesn't touch), so neither runs this.
type RebindPhase =
  | { status: "applying" }
  | { status: "reconnecting"; kind: "port" | "scope"; requestedPort?: number }
  | { status: "cutoff"; message: string };

/** How long to wait for the reconnect loop to re-establish before declaring
 *  the device cut off. The first backoff attempt fires ~1s after the drop, so
 *  a reachable origin reconnects well inside this; an unreachable one (stale
 *  origin port, excluded scope) trips the terminal state instead of spinning
 *  forever. */
const REBIND_RECONNECT_CAP_MS = 12_000;

const SCOPE_CUTOFF_MESSAGE =
  "Access scope changed — this device can no longer reach the server. Reconnect from an allowed endpoint to continue.";

/** Where browsers signed into the account find and open this machine. */
const HOSTED_CLIENT_URL = "https://app.codemux.org";

/** A change that would disconnect the browser making it. Confirmed first. */
type PendingCutoff =
  | { kind: "scope"; scope: WebRemoteBindScope }
  | { kind: "lan" }
  | { kind: "relay" }
  | { kind: "master" };

/** Which way in this UI itself is using: the desktop's own IPC, the network
 *  listener, or the relay. Decides which changes would cut it off. */
function currentTransport(): "desktop" | "lan" | "relay" {
  if (!isRemoteClient()) return "desktop";
  return isHostedOrigin() ? "relay" : "lan";
}

// ── Main section ─────────────────────────────────────────────────────

export function RemoteAccessSection() {
  const [status, setStatus] = useState<WebRemoteStatus | null>(null);
  const [endpoints, setEndpoints] = useState<WebRemoteEndpoint[]>([]);
  const [pairing, setPairing] = useState<WebRemotePairingInfo | null>(null);
  const [portDraft, setPortDraft] = useState("");
  const [togglePending, setTogglePending] = useState(false);
  const [retryPending, setRetryPending] = useState(false);
  const [portPending, setPortPending] = useState(false);
  const [scopePending, setScopePending] = useState(false);
  // Web-client rebind lifecycle (see RebindPhase). Null on desktop / at rest.
  const [scopeOverride, setScopeOverride] = useState<WebRemoteBindScope | null>(
    null,
  );
  const [rebindPhase, setRebindPhase] = useState<RebindPhase | null>(null);
  const [pendingCutoff, setPendingCutoff] = useState<PendingCutoff | null>(null);
  // Live transport status (populated only on the web client). Drives the
  // reconnect-and-reconcile step after a rebind-induced disconnect.
  const connectionStatus = useRemoteConnectionStore((s) => s.status);
  const [approvalPending, setApprovalPending] = useState(false);
  const [accountModePending, setAccountModePending] = useState(false);
  const [trustAccountPending, setTrustAccountPending] = useState(false);
  const [relayModePending, setRelayModePending] = useState(false);
  const [lanPending, setLanPending] = useState(false);
  // Control-plane registration state for the relay (plus this machine's
  // display name). Null until read, or when the read failed.
  const [registration, setRegistration] =
    useState<WebRemoteRegistrationStatus | null>(null);
  const [pairingPending, setPairingPending] = useState(false);
  const [sessionBusy, setSessionBusy] = useState<string | null>(null);
  const [revokingAll, setRevokingAll] = useState(false);

  // Latest known sessions, used to detect *new* pending devices on each
  // live event so we can toast exactly once per arrival.
  const prevSessionsRef = useRef<WebRemoteSessionView[]>([]);

  const applyStatus = useCallback(
    (next: WebRemoteStatus, { detectPending }: { detectPending: boolean }) => {
      if (detectPending) {
        const fresh = newlyPendingSessionIds(
          prevSessionsRef.current,
          next.sessions,
        );
        for (const id of fresh) {
          const s = next.sessions.find((x) => x.id === id);
          const d = describeDevice(s?.name ?? null, s?.user_agent ?? null);
          toast.info(`${d.title} wants to connect`, {
            description: "Approve it under Devices to grant access.",
          });
        }
      }
      prevSessionsRef.current = next.sessions;
      setStatus(next);
    },
    [],
  );

  const refreshEndpoints = useCallback(async () => {
    try {
      setEndpoints(await webRemoteListEndpoints());
    } catch (err) {
      console.error("[remote-access] list endpoints failed:", err);
    }
  }, []);

  /** Settle a rebind against the server's actual persisted state: adopt the
   *  fresh status, drop the optimistic override, and clear the lifecycle. Used
   *  both when `set_config` returned normally (no disconnect) and after the
   *  socket reconnects. Silent by design — the reflected value is the feedback. */
  const reconcileAfterRebind = useCallback(
    (fresh: WebRemoteStatus) => {
      applyStatus(fresh, { detectPending: false });
      setPortDraft(String(fresh.port));
      setScopeOverride(null);
      setRebindPhase(null);
      // A rebind changes the reachable set and invalidates any composed URL.
      setPairing(null);
      if (fresh.running) void refreshEndpoints();
    },
    [applyStatus, refreshEndpoints],
  );

  /** Enter the terminal cutoff state: this device can't reconnect. Annotate
   *  the connection banner so the ongoing reconnect loop reads as a clear
   *  terminal message instead of an endless "Reconnecting…" spinner. */
  const enterCutoff = useCallback((message: string) => {
    setRebindPhase({ status: "cutoff", message });
    useRemoteConnectionStore.getState().setOffline(message);
  }, []);

  // Initial load + live subscription.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    (async () => {
      try {
        const s = await webRemoteStatus();
        if (disposed) return;
        prevSessionsRef.current = s.sessions;
        setStatus(s);
        setPortDraft(String(s.port));
        if (s.running) void refreshEndpoints();
      } catch (err) {
        console.error("[remote-access] status load failed:", err);
      }
    })();
    onWebRemoteStateChanged((next) => {
      applyStatus(next, { detectPending: true });
      if (next.running) void refreshEndpoints();
      else setEndpoints([]);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyStatus, refreshEndpoints]);

  // After a rebind-induced disconnect, the shim's reconnect loop re-establishes
  // the socket and the store flips back to "connected". Refetch the authoritative
  // status then and reconcile the optimistic control to it. Also self-heals a
  // mispredicted cutoff: if the device unexpectedly comes back, it settles.
  useEffect(() => {
    if (!rebindPhase || rebindPhase.status === "applying") return;
    if (connectionStatus !== "connected") return;
    let cancelled = false;
    (async () => {
      try {
        const fresh = await webRemoteStatus();
        if (!cancelled) reconcileAfterRebind(fresh);
      } catch {
        /* still settling — the effect re-runs on the next connected tick */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rebindPhase, connectionStatus, reconcileAfterRebind]);

  // Cap the reconnect wait: if the socket hasn't come back within the window,
  // this device is cut off (stale origin port, or a scope that excludes it).
  // Trip a clear terminal state instead of spinning indefinitely.
  useEffect(() => {
    if (rebindPhase?.status !== "reconnecting") return;
    const { kind, requestedPort } = rebindPhase;
    const timer = window.setTimeout(() => {
      enterCutoff(
        kind === "port" && requestedPort != null
          ? `Port changed to ${requestedPort}. If this device dropped, reopen Codemux at the new port.`
          : "This device didn't reconnect. Reload from an allowed endpoint to continue.",
      );
    }, REBIND_RECONNECT_CAP_MS);
    return () => window.clearTimeout(timer);
  }, [rebindPhase, enterCutoff]);

  const enabled = status?.enabled ?? false;
  const running = status?.running ?? false;
  const lanEnabled = lanEnabledOf(status);
  // Why the listener isn't bound even though it's switched on. The backend
  // keeps retrying while this is set; the card shows it instead of an
  // open-ended "starting".
  const bindError = status?.bind_error ?? null;
  const requireApproval = status?.require_approval ?? false;
  const accountModeEnabled = status?.account_mode_enabled ?? false;
  const trustAccountBrowsers = status?.trust_account_browsers ?? false;
  const accountSignedIn = status?.account_signed_in ?? false;
  const relayModeEnabled = status?.relay_mode_enabled ?? false;
  // Coarse signals the status broadcast carries. They double as the refetch
  // trigger for the richer registration read and as its fallback.
  const deviceRegistered = status?.device_registered ?? false;
  const irohNodeId = status?.iroh_node_id ?? null;
  const registrationError = status?.registration_error ?? null;
  // While a web-client rebind settles, show the requested scope optimistically
  // rather than the server's last-broadcast value.
  const bindScope = scopeOverride ?? bindScopeOf(status);
  const pending = useMemo(() => pendingSessions(status), [status]);
  const approved = useMemo(() => approvedSessions(status), [status]);
  const connectedCount = connectedSessionCount(status);
  const exposure = describeExposure(status);
  const transport = currentTransport();

  // The dedicated registration read adds the ids, the last heartbeat, the last
  // error, and this machine's display name. Refetch whenever one of the
  // broadcast signals that can move it changes. A failed read (older backend)
  // degrades to the status flags rather than surfacing a raw error.
  const relayLive = enabled && relayModeEnabled;
  useEffect(() => {
    if (!enabled) {
      setRegistration(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const next = await webRemoteRegistrationStatus();
        if (!cancelled) setRegistration(next);
      } catch (err) {
        console.error("[remote-access] registration status failed:", err);
        if (!cancelled) setRegistration(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    enabled,
    relayLive,
    deviceRegistered,
    irohNodeId,
    accountSignedIn,
    registrationError,
  ]);

  // Prefer the dedicated read, fall back to the live status broadcast.
  const relayRegistered = registration?.registered ?? deviceRegistered;
  const relayDeviceId = registration?.device_id ?? status?.device_id ?? null;
  // The hostname reads as "this machine" to a human; the device id only means
  // something when two hosts share a name.
  const machineName = registration?.name || null;
  const relayDisplayName = machineName || relayDeviceId;
  const relayNodeId = registration?.node_id ?? irohNodeId;
  const relayLastRegisteredAt = registration?.last_registered_at ?? null;
  // The live broadcast carries the current failure (including the relay
  // endpoint itself failing to start); the dedicated read is the fallback for
  // a backend that doesn't broadcast it.
  const relayLastError = relayRegistered
    ? null
    : (registrationError ?? registration?.last_error ?? null);

  const portValidation = validatePort(portDraft);
  const portDirty = status != null && portDraft !== String(status.port);
  // A web-client rebind is settling (or has cut this device off): freeze the
  // listener controls so a second change can't stack on the in-flight one.
  const rebindBusy = rebindPhase !== null;
  const primaryEndpoint = pickPrimaryEndpoint(endpoints);

  /** After a change that (re)starts the listener, say so if it didn't come up. */
  const reportLanOutcome = useCallback(
    (result: WebRemoteStatus, success: string) => {
      if (result.bind_error) {
        toast.error(`Saved, but the listener couldn't start: ${result.bind_error}`);
      } else {
        toast.success(success);
      }
    },
    [],
  );

  const applyMaster = useCallback(
    async (next: boolean) => {
      setTogglePending(true);
      try {
        const result = next ? await webRemoteEnable() : await webRemoteDisable();
        applyStatus(result, { detectPending: false });
        if (result.running) void refreshEndpoints();
        else {
          setEndpoints([]);
          setPairing(null);
        }
        toast.success(
          next
            ? "Remote access is on — choose how devices connect."
            : "Remote access is off. Nothing can reach this machine.",
        );
      } catch (err) {
        console.error("[remote-access] toggle failed:", err);
        toast.error(`Couldn't ${next ? "enable" : "disable"} remote access: ${String(err)}`);
      } finally {
        setTogglePending(false);
      }
    },
    [applyStatus, refreshEndpoints],
  );

  const handleToggle = useCallback(
    (next: boolean) => {
      // Any browser is cut off when everything turns off — confirm first.
      if (!next && transport !== "desktop") {
        setPendingCutoff({ kind: "master" });
        return;
      }
      void applyMaster(next);
    },
    [applyMaster, transport],
  );

  /** Retry a way in that is switched on but failed to start, right now
   *  instead of waiting for the backend's own retry schedule: re-attempts the
   *  listener bind (when it's on) and relay registration (when it's on). */
  const handleRetry = useCallback(
    async (what: "server" | "registration") => {
      setRetryPending(true);
      try {
        const result = await webRemoteRetry();
        applyStatus(result, { detectPending: false });
        if (result.running) void refreshEndpoints();
        if (what === "server") toast.success("Listening on your network again.");
      } catch (err) {
        console.error("[remote-access] retry failed:", err);
        if (what === "server") {
          toast.error(`Still couldn't start the server: ${String(err)}`);
        } else {
          // The rejection is about the listener; registration was retried
          // regardless and reports back through the live status.
          void webRemoteStatus()
            .then((fresh) => applyStatus(fresh, { detectPending: false }))
            .catch(() => undefined);
        }
      } finally {
        setRetryPending(false);
      }
    },
    [applyStatus, refreshEndpoints],
  );

  const handleApplyPort = useCallback(async () => {
    if (!portValidation.valid || portValidation.value == null) return;
    const nextPort = portValidation.value;
    setPortPending(true);

    // Browser on the network listener: the rebind drops this socket before
    // set_config can answer. Reflect the new port optimistically and settle on
    // reconnect, rather than treating the expected disconnect as a failure.
    if (transport === "lan") {
      setPortDraft(String(nextPort));
      setRebindPhase({ status: "applying" });
      try {
        const result = await webRemoteSetConfig({ port: nextPort });
        // No disconnect — the change didn't drop us. Settle immediately.
        reconcileAfterRebind(result);
      } catch (err) {
        if (isRebindDisconnectError(err)) {
          setRebindPhase({
            status: "reconnecting",
            kind: "port",
            requestedPort: nextPort,
          });
        } else {
          console.error("[remote-access] set port failed:", err);
          setPortDraft(String(status?.port ?? nextPort));
          setRebindPhase(null);
          toast.error(`Couldn't change the port: ${String(err)}`);
        }
      } finally {
        setPortPending(false);
      }
      return;
    }

    // Desktop (native IPC) or relay: this UI's own transport is unaffected.
    try {
      const result = await webRemoteSetConfig({ port: nextPort });
      applyStatus(result, { detectPending: false });
      setPortDraft(String(result.port));
      if (result.running) void refreshEndpoints();
      // A port change invalidates any composed pairing URL.
      setPairing(null);
      reportLanOutcome(result, `Port set to ${result.port}.`);
    } catch (err) {
      console.error("[remote-access] set port failed:", err);
      toast.error(`Couldn't change the port: ${String(err)}`);
    } finally {
      setPortPending(false);
    }
  }, [
    applyStatus,
    portValidation,
    reconcileAfterRebind,
    refreshEndpoints,
    reportLanOutcome,
    status,
    transport,
  ]);

  // Web-client scope apply: reflect optimistically, then either settle on
  // reconnect (reachable) or trip the terminal cutoff (excluded / capped out).
  // Shared by the direct path and the cutoff-confirm path.
  const applyScopeRemote = useCallback(
    async (next: WebRemoteBindScope, opts: { expectCutoff: boolean }) => {
      setScopeOverride(next);
      setRebindPhase({ status: "applying" });
      setScopePending(true);
      try {
        const result = await webRemoteSetConfig({ bindScope: next });
        // No disconnect (the change didn't require a rebind) — settle now.
        reconcileAfterRebind(result);
      } catch (err) {
        if (isRebindDisconnectError(err)) {
          // Expected: the rebind dropped this socket before answering.
          if (opts.expectCutoff) enterCutoff(SCOPE_CUTOFF_MESSAGE);
          else setRebindPhase({ status: "reconnecting", kind: "scope" });
        } else {
          // A genuine backend rejection (e.g. Tailscale scope with no tailnet
          // address): the server kept the previous scope. Drop the optimistic
          // value and surface why.
          console.error("[remote-access] set scope failed:", err);
          setScopeOverride(null);
          setRebindPhase(null);
          toast.error(`Couldn't change where the server is visible: ${String(err)}`);
        }
      } finally {
        setScopePending(false);
      }
    },
    [enterCutoff, reconcileAfterRebind],
  );

  const handleSetScope = useCallback(
    async (next: WebRemoteBindScope) => {
      if (next === bindScope) return;

      // Browser on the network listener: a scope change rebinds and drops it.
      // If the new scope would exclude the origin we loaded from, confirm the
      // intentional cutoff first; otherwise apply and settle on reconnect.
      if (transport === "lan") {
        if (!originHostSurvivesScope(window.location.hostname, next)) {
          setPendingCutoff({ kind: "scope", scope: next });
          return;
        }
        void applyScopeRemote(next, { expectCutoff: false });
        return;
      }

      // Desktop (native IPC) or relay: this UI's own transport is unaffected.
      setScopePending(true);
      try {
        const result = await webRemoteSetConfig({ bindScope: next });
        applyStatus(result, { detectPending: false });
        // A scope change rebinds, so the reachable-endpoint set changes too.
        if (result.running) void refreshEndpoints();
        // A rebind drops any composed pairing URL (the old address may be gone).
        setPairing(null);
        reportLanOutcome(
          result,
          `Now visible on ${lanExposurePhrase(next).replace(" of this machine", "")}.`,
        );
      } catch (err) {
        console.error("[remote-access] set scope failed:", err);
        // The backend keeps the previous, working scope on failure and the
        // status it broadcasts reflects that, so the control snaps back.
        toast.error(`Couldn't change where the server is visible: ${String(err)}`);
      } finally {
        setScopePending(false);
      }
    },
    [applyScopeRemote, applyStatus, bindScope, refreshEndpoints, reportLanOutcome, transport],
  );

  const applyLan = useCallback(
    async (next: boolean) => {
      setLanPending(true);
      try {
        const result = await webRemoteSetConfig({ lanEnabled: next });
        applyStatus(result, { detectPending: false });
        if (result.running) void refreshEndpoints();
        else {
          setEndpoints([]);
          setPairing(null);
        }
        if (next) reportLanOutcome(result, "Listening on your network.");
        else toast.success("Stopped listening on your network.");
      } catch (err) {
        console.error("[remote-access] set LAN listener failed:", err);
        toast.error(`Couldn't change network access: ${String(err)}`);
      } finally {
        setLanPending(false);
      }
    },
    [applyStatus, refreshEndpoints, reportLanOutcome],
  );

  const handleToggleLan = useCallback(
    (next: boolean) => {
      if (!next && transport === "lan") {
        setPendingCutoff({ kind: "lan" });
        return;
      }
      void applyLan(next);
    },
    [applyLan, transport],
  );

  const handleToggleApproval = useCallback(
    async (next: boolean) => {
      setApprovalPending(true);
      try {
        const result = await webRemoteSetConfig({ requireApproval: next });
        applyStatus(result, { detectPending: false });
        toast.success(
          next
            ? "Pairing links now wait for your approval."
            : "A valid pairing link now connects immediately.",
        );
      } catch (err) {
        console.error("[remote-access] set approval failed:", err);
        toast.error(`Couldn't change approval mode: ${String(err)}`);
      } finally {
        setApprovalPending(false);
      }
    },
    [applyStatus],
  );

  const handleToggleAccountMode = useCallback(
    async (next: boolean) => {
      setAccountModePending(true);
      try {
        const result = await webRemoteSetConfig({ accountModeEnabled: next });
        applyStatus(result, { detectPending: false });
        toast.success(
          next
            ? "Browsers on your network can now sign in with your Codemux account."
            : "Account sign-in on your network is off — only pairing links connect there.",
        );
      } catch (err) {
        console.error("[remote-access] set account mode failed:", err);
        toast.error(`Couldn't change account sign-in: ${String(err)}`);
      } finally {
        setAccountModePending(false);
      }
    },
    [applyStatus],
  );

  /** "Approve browsers on my account" is the inverse of the stored
   *  `trust_account_browsers` opt-out. */
  const handleToggleAccountApproval = useCallback(
    async (requireApprovalNext: boolean) => {
      setTrustAccountPending(true);
      try {
        const result = await webRemoteSetConfig({
          trustAccountBrowsers: !requireApprovalNext,
        });
        applyStatus(result, { detectPending: false });
        toast.success(
          requireApprovalNext
            ? "Browsers on your account now wait for your approval."
            : "Browsers on your account now connect without approval.",
        );
      } catch (err) {
        console.error("[remote-access] set trust-account failed:", err);
        toast.error(`Couldn't change account approval: ${String(err)}`);
      } finally {
        setTrustAccountPending(false);
      }
    },
    [applyStatus],
  );

  const applyRelay = useCallback(
    async (next: boolean) => {
      setRelayModePending(true);
      try {
        const result = await webRemoteSetConfig({ relayModeEnabled: next });
        applyStatus(result, { detectPending: false });
        if (next && !result.relay_running && result.registration_error) {
          toast.error(`Saved, but ${result.registration_error}`);
        } else {
          toast.success(
            next
              ? "From anywhere is on — this device is registering with your account."
              : "From anywhere is off.",
          );
        }
      } catch (err) {
        console.error("[remote-access] set relay mode failed:", err);
        toast.error(`Couldn't change from-anywhere access: ${String(err)}`);
      } finally {
        setRelayModePending(false);
      }
    },
    [applyStatus],
  );

  const handleToggleRelayMode = useCallback(
    (next: boolean) => {
      if (!next && transport === "relay") {
        setPendingCutoff({ kind: "relay" });
        return;
      }
      void applyRelay(next);
    },
    [applyRelay, transport],
  );

  const confirmCutoff = useCallback(() => {
    const cut = pendingCutoff;
    setPendingCutoff(null);
    if (!cut) return;
    if (cut.kind === "scope") void applyScopeRemote(cut.scope, { expectCutoff: true });
    else if (cut.kind === "lan") void applyLan(false);
    else if (cut.kind === "relay") void applyRelay(false);
    else void applyMaster(false);
  }, [applyLan, applyMaster, applyRelay, applyScopeRemote, pendingCutoff]);

  const handleCreatePairing = useCallback(async () => {
    setPairingPending(true);
    try {
      setPairing(await webRemoteCreatePairing());
    } catch (err) {
      console.error("[remote-access] create pairing failed:", err);
      toast.error(`Couldn't create a pairing link: ${String(err)}`);
    } finally {
      setPairingPending(false);
    }
  }, []);

  const handleApprove = useCallback(
    async (id: string) => {
      setSessionBusy(id);
      try {
        applyStatus(await webRemoteApproveSession(id), { detectPending: false });
        toast.success("Device approved.");
      } catch (err) {
        toast.error(`Couldn't approve the device: ${String(err)}`);
      } finally {
        setSessionBusy(null);
      }
    },
    [applyStatus],
  );

  const handleReject = useCallback(
    async (id: string) => {
      setSessionBusy(id);
      try {
        applyStatus(await webRemoteRejectSession(id), { detectPending: false });
        toast.success("Request rejected.");
      } catch (err) {
        toast.error(`Couldn't reject the device: ${String(err)}`);
      } finally {
        setSessionBusy(null);
      }
    },
    [applyStatus],
  );

  const handleRevoke = useCallback(
    async (id: string) => {
      setSessionBusy(id);
      try {
        applyStatus(await webRemoteRevokeSession(id), { detectPending: false });
        toast.success("Device revoked — its access is now blocked.");
      } catch (err) {
        toast.error(`Couldn't revoke the device: ${String(err)}`);
      } finally {
        setSessionBusy(null);
      }
    },
    [applyStatus],
  );

  const handleRevokeAll = useCallback(async () => {
    setRevokingAll(true);
    try {
      let last: WebRemoteStatus | null = null;
      for (const s of approved) {
        last = await webRemoteRevokeSession(s.id);
      }
      if (last) applyStatus(last, { detectPending: false });
      toast.success("Revoked every device.");
    } catch (err) {
      toast.error(`Couldn't revoke every device: ${String(err)}`);
    } finally {
      setRevokingAll(false);
    }
  }, [applyStatus, approved]);

  // Shared by both cards: the one setting that governs every browser signing
  // in with the account, whichever way in it used.
  const accountApprovalRow = (
    <SettingRow
      title="Approve browsers that sign in with your account"
      detail="A new browser on your account waits under Devices until you approve it. Applies to both ways in."
      checked={!trustAccountBrowsers}
      onCheckedChange={handleToggleAccountApproval}
      disabled={trustAccountPending}
      switchLabel="Toggle approval for account browsers"
    />
  );

  const relayCardBody = !accountSignedIn ? (
    <AttentionNote>
      This desktop isn't signed into a Codemux account, so it can't register for
      from-anywhere access. Sign in from the account menu to activate it.
    </AttentionNote>
  ) : (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        {relayRegistered ? (
          <LiveState tone="ok">Registered</LiveState>
        ) : relayLastError ? (
          <LiveState tone="error">Not registered yet</LiveState>
        ) : (
          <LiveState tone="pending">Registering…</LiveState>
        )}
        {relayDisplayName && (
          <span className="flex min-w-0 items-center gap-1 text-body-sm text-muted-foreground/80">
            as
            <code className="truncate font-mono text-foreground">{relayDisplayName}</code>
            <CopyButton text={relayDisplayName} label="Copy device name" />
          </span>
        )}
        {relayRegistered && relayLastRegisteredAt && (
          <span className="text-body-sm text-muted-foreground/70">
            · confirmed {relativeTime(relayLastRegisteredAt)}
          </span>
        )}
      </div>
      {relayLastError ? (
        <div className="flex items-start justify-between gap-3">
          <p className="min-w-0 break-words text-body-sm leading-relaxed text-status-attention">
            Last attempt failed: {relayLastError}
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="shrink-0"
            disabled={retryPending}
            onClick={() => void handleRetry("registration")}
            aria-label="Retry registering this device"
          >
            <RefreshCw className={cn("size-3.5", retryPending && "animate-spin")} />
            Retry
          </Button>
        </div>
      ) : (
        <p className="text-body-sm leading-relaxed text-muted-foreground/80">
          {relayRegistered
            ? "Listed with your account. Open app.codemux.org in any browser signed into it, then pick this device."
            : "Listing this device with your account. This takes a moment."}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          onClick={() => void openUrl(HOSTED_CLIENT_URL)}
        >
          <ExternalLink className="size-3.5" />
          Open app.codemux.org
        </Button>
      </div>
      {relayNodeId && (
        <details className="group">
          <summary className="flex w-fit cursor-pointer list-none items-center gap-1 text-label font-medium text-muted-foreground marker:content-none hover:text-foreground">
            <span className="transition-transform duration-150 group-open:rotate-90">›</span>
            Connection details
          </summary>
          <div className="mt-1.5 flex items-center gap-2">
            <span className="shrink-0 text-body-sm text-muted-foreground/70">Address</span>
            <code className="min-w-0 flex-1 truncate font-mono text-body-sm text-foreground">
              {relayNodeId}
            </code>
            <CopyButton text={relayNodeId} label="Copy device address" />
          </div>
        </details>
      )}
    </div>
  );

  return (
    // Inline-size containment: the settings scroll area sizes its content like
    // a table (to the widest min-content), so the long truncated addresses and
    // device lines would otherwise push this pane past a phone's width instead
    // of truncating.
    <div className="space-y-8 [contain:inline-size]">
      {/* Header + kill switch */}
      <div className="space-y-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <MonitorSmartphone className="size-4 text-accent-ember" />
              <h2 className="text-[1.3125rem] font-bold tracking-tight text-foreground">
                Remote Access
              </h2>
            </div>
            <p className="mt-1.5 max-w-prose text-body-lg leading-relaxed text-muted-foreground/80">
              {enabled ? (
                <>
                  {machineName ? (
                    <>
                      Reachable as{" "}
                      <span className="font-medium text-foreground">
                        “{machineName}”
                      </span>
                      .{" "}
                    </>
                  ) : null}
                  Choose how devices reach it.
                </>
              ) : (
                "Open this desktop in a browser on your phone or another computer, and drive the same projects, sessions, and agents from there."
              )}
            </p>
          </div>
          <Switch
            checked={enabled}
            onCheckedChange={handleToggle}
            disabled={togglePending}
            aria-label="Toggle remote access"
            className="mt-1.5"
          />
        </div>

        {/* Exposure — what this configuration actually opens. */}
        <div className="flex items-start gap-2.5 rounded-lg border border-warning/30 bg-warning/5 px-3.5 py-3">
          <ShieldAlert className="mt-0.5 size-4 shrink-0 text-warning" />
          <div className="space-y-1 text-label leading-relaxed text-muted-foreground">
            <p className="font-medium text-foreground">{exposure}</p>
            <p>
              Anyone you let in gets full control of this computer — open
              terminals, run agents, and read or edit your files. Let in only
              devices you trust, and revoke them when you're done.
            </p>
          </div>
        </div>
      </div>

      {enabled && (
        <>
          {/* Web-client rebind lifecycle. A port/scope change from a browser
              on the network listener drops this socket before the backend
              can answer; rather than a false error, show that the change is
              applying + reconnecting, or a clear terminal state if this
              device was cut off. */}
          {rebindPhase && rebindPhase.status !== "applying" ? (
            rebindPhase.status === "cutoff" ? (
              <div
                role="status"
                className="flex items-start gap-2.5 rounded-lg border border-status-attention/40 bg-status-attention/[0.08] px-3.5 py-3 text-body leading-relaxed text-status-attention"
              >
                <WifiOff className="mt-0.5 size-4 shrink-0" />
                <span>{rebindPhase.message}</span>
              </div>
            ) : (
              <div
                role="status"
                className="flex items-center gap-2.5 rounded-lg border border-status-working/40 bg-status-working/[0.08] px-3.5 py-3 text-body text-status-working"
              >
                <Loader2 className="size-4 shrink-0 animate-spin" />
                <span>Applying change — reconnecting to this device…</span>
              </div>
            )
          ) : null}

          <div className="space-y-3">
            <SubHeading>Ways to connect</SubHeading>

            {/* From anywhere — first and recommended: the easiest path, and
                the only one that works off your own network. */}
            <WayCard
              icon={Globe}
              title="From anywhere"
              badge={
                <Badge
                  variant="outline"
                  className="border-accent-ember/30 bg-accent-ember/10 text-caption font-medium text-accent-ember"
                >
                  Recommended
                </Badge>
              }
              description={
                <>
                  Any browser signed into{" "}
                  <span className="font-medium text-foreground">
                    your Codemux account
                  </span>
                  , on any network — no shared Wi-Fi, VPN, or port forwarding.
                  End-to-end encrypted; the relay only passes along traffic it
                  can't read.
                </>
              }
              checked={relayModeEnabled}
              onCheckedChange={handleToggleRelayMode}
              disabled={relayModePending}
              switchLabel="Toggle from-anywhere access"
            >
              {relayCardBody}
              <div className="border-t border-border/60 pt-4">{accountApprovalRow}</div>
            </WayCard>

            {/* On my network — the direct LAN / tailnet listener. Every
                listener setting lives here, next to the only thing it affects. */}
            <WayCard
              icon={Wifi}
              title="On my network"
              description="Connect directly from a device on the same network or tailnet, using a one-time pairing link."
              checked={lanEnabled}
              onCheckedChange={handleToggleLan}
              disabled={lanPending}
              switchLabel="Toggle access on my network"
            >
              {bindError ? (
                <FailureCallout
                  title="The server couldn't start listening"
                  reason={bindError}
                  hint={
                    relayModeEnabled && status?.relay_running
                      ? "Codemux keeps retrying on its own, and from-anywhere access is unaffected. Pick another port or change where it's visible below, or retry now."
                      : "Codemux keeps retrying on its own. Pick another port or change where it's visible below, or retry now."
                  }
                  onRetry={() => void handleRetry("server")}
                  retrying={retryPending}
                  retryLabel="Retry starting the server"
                />
              ) : running ? (
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                  <LiveState tone="ok">Listening on port {status?.port}</LiveState>
                  {primaryEndpoint && (
                    <span className="flex min-w-0 items-center gap-1">
                      <code className="truncate font-mono text-body-sm text-foreground">
                        {primaryEndpoint.url}
                      </code>
                      <CopyButton
                        text={primaryEndpoint.url}
                        label={`Copy ${primaryEndpoint.url} address`}
                      />
                    </span>
                  )}
                </div>
              ) : (
                <LiveState tone="pending">Starting the server…</LiveState>
              )}

              {/* Visible on — which interfaces the listener binds. Changing
                  it rebinds immediately (same path as a port change). */}
              <div className="space-y-2">
                <p className="text-body font-medium leading-none text-foreground">
                  Visible on
                </p>
                <div
                  role="radiogroup"
                  aria-label="Visible on"
                  className="inline-flex flex-wrap rounded-lg border border-border/60 bg-muted/30 p-0.5"
                >
                  {BIND_SCOPE_OPTIONS.map((opt) => {
                    const active = opt.value === bindScope;
                    return (
                      <button
                        key={opt.value}
                        type="button"
                        role="radio"
                        aria-checked={active}
                        disabled={scopePending || rebindBusy}
                        onClick={() => handleSetScope(opt.value)}
                        className={cn(
                          "rounded-md px-3 py-1.5 text-body font-medium transition-colors duration-150 disabled:opacity-60",
                          active
                            ? "bg-accent-ember/15 text-accent-ember shadow-sm"
                            : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {opt.label}
                      </button>
                    );
                  })}
                </div>
                <p className="text-body-sm leading-relaxed text-muted-foreground/85">
                  {BIND_SCOPE_OPTIONS.find((o) => o.value === bindScope)?.detail}
                </p>
              </div>

              {/* Pair a device */}
              <div className="space-y-2">
                <p className="text-body font-medium leading-none text-foreground">
                  Pair a device
                </p>
                {pairing ? (
                  <PairingPanel
                    pairing={pairing}
                    endpoints={endpoints}
                    regenerating={pairingPending}
                    onRegenerate={handleCreatePairing}
                  />
                ) : (
                  <div className="flex items-center justify-between gap-4">
                    <p className="text-body-sm leading-relaxed text-muted-foreground/85">
                      Create a one-time link, then scan its QR code or open it
                      on the other device.
                    </p>
                    <Button
                      type="button"
                      size="sm"
                      className="shrink-0"
                      disabled={pairingPending || !running}
                      onClick={handleCreatePairing}
                    >
                      <Link2 className="size-3.5" />
                      Create pairing link
                    </Button>
                  </div>
                )}
              </div>

              {endpoints.length > 0 && (
                <details className="group">
                  <summary className="flex w-fit cursor-pointer list-none items-center gap-1 text-label font-medium text-muted-foreground marker:content-none hover:text-foreground">
                    <span className="transition-transform duration-150 group-open:rotate-90">
                      ›
                    </span>
                    Reachable at ({endpoints.length})
                  </summary>
                  <div className="mt-3">
                    <GroupedEndpoints endpoints={endpoints} />
                  </div>
                </details>
              )}

              <div className="flex items-end justify-between gap-4 border-t border-border/60 pt-4">
                <div className="min-w-0 flex-1 space-y-1">
                  <label
                    htmlFor="web-remote-port"
                    className="block text-body font-medium leading-none text-foreground"
                  >
                    Port
                  </label>
                  <p className="text-body-sm leading-relaxed text-muted-foreground/85">
                    Changing it restarts the server immediately and invalidates
                    any open pairing link.
                  </p>
                  {portDraft !== "" && !portValidation.valid && (
                    <p className="text-body-sm text-status-attention">
                      {portValidation.error}
                    </p>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Input
                    id="web-remote-port"
                    inputMode="numeric"
                    value={portDraft}
                    onChange={(e) => setPortDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && portDirty && portValidation.valid) {
                        void handleApplyPort();
                      }
                    }}
                    aria-label="Server port"
                    aria-invalid={portDraft !== "" && !portValidation.valid}
                    className="h-9 w-28 font-mono"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="default"
                    disabled={
                      !portDirty || !portValidation.valid || portPending || rebindBusy
                    }
                    onClick={handleApplyPort}
                  >
                    Apply
                  </Button>
                </div>
              </div>

              <div className="border-t border-border/60 pt-4">
                <SettingRow
                  title="Approve devices that use a pairing link"
                  detail="A device that opens a valid pairing link waits under Devices until you approve it. When off, the link connects right away."
                  checked={requireApproval}
                  onCheckedChange={handleToggleApproval}
                  disabled={approvalPending}
                  switchLabel="Toggle approval mode"
                />
              </div>

              <div className="space-y-4 border-t border-border/60 pt-4">
                <SettingRow
                  title="Allow account sign-in instead of a pairing link"
                  detail={
                    <>
                      A browser that reaches this address can sign in with{" "}
                      <span className="font-medium text-foreground">
                        the Codemux account
                      </span>{" "}
                      this desktop uses — no pairing link to copy. Only affects
                      this way in; From anywhere always uses your account.
                    </>
                  }
                  checked={accountModeEnabled}
                  onCheckedChange={handleToggleAccountMode}
                  disabled={accountModePending}
                  switchLabel="Toggle account sign-in"
                />
                {accountModeEnabled && !accountSignedIn && (
                  <AttentionNote>
                    This desktop isn't signed into a Codemux account, so account
                    sign-in can't verify anyone yet. Sign in from the account
                    menu to activate it.
                  </AttentionNote>
                )}
                {accountModeEnabled && accountApprovalRow}
              </div>
            </WayCard>
          </div>

          {/* Devices — one answer to "what can reach this machine": paired
              devices and account browsers, from either way in. */}
          <section className="space-y-2">
            <div className="flex items-center justify-between gap-4">
              <div className="flex items-center gap-2">
                <SubHeading>Devices</SubHeading>
                {connectedCount > 0 && (
                  <span className="text-label font-medium text-status-open tabular-nums">
                    {connectedCount} connected
                  </span>
                )}
              </div>
              {approved.length > 0 && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="text-muted-foreground hover:text-status-attention"
                  disabled={revokingAll}
                  onClick={handleRevokeAll}
                >
                  <Trash2 className="size-3.5" />
                  Revoke all
                </Button>
              )}
            </div>

            {pending.length > 0 && (
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <p className="text-body-sm font-medium text-status-working">
                    Waiting for approval
                  </p>
                  <Badge
                    variant="outline"
                    className="border-status-working/30 bg-status-working/10 text-caption text-status-working"
                  >
                    {pending.length}
                  </Badge>
                </div>
                {pending.map((s) => (
                  <PendingRow
                    key={s.id}
                    session={s}
                    busy={sessionBusy === s.id}
                    onApprove={() => handleApprove(s.id)}
                    onReject={() => handleReject(s.id)}
                  />
                ))}
              </div>
            )}

            {approved.length === 0 ? (
              pending.length === 0 && (
                <div className="flex items-center gap-2.5 rounded-lg border border-dashed border-border/60 px-3.5 py-4 text-body text-muted-foreground/70">
                  <Server className="size-4" />
                  No devices yet. Connect one from app.codemux.org or with a
                  pairing link.
                </div>
              )
            ) : (
              <div className="divide-y divide-border/50">
                {approved.map((s) => (
                  <DeviceRow
                    key={s.id}
                    session={s}
                    busy={sessionBusy === s.id}
                    onRevoke={() => handleRevoke(s.id)}
                  />
                ))}
              </div>
            )}
          </section>
        </>
      )}

      {/* Cutoff confirm — a change that would sever the browser making it,
          with no way back on its current address. Ask first. */}
      <AlertDialog
        open={pendingCutoff !== null}
        onOpenChange={(open) => {
          if (!open) setPendingCutoff(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Disconnect this device?</AlertDialogTitle>
            <AlertDialogDescription>
              {pendingCutoff?.kind === "scope" && (
                <>
                  This will disconnect this device — you're connected over{" "}
                  <span className="font-mono text-foreground">
                    {window.location.host}
                  </span>
                  , which "{bindScopeLabel(pendingCutoff.scope)}" no longer
                  allows. You'll need to reconnect from an allowed address.
                </>
              )}
              {pendingCutoff?.kind === "lan" && (
                <>
                  You're connected over{" "}
                  <span className="font-mono text-foreground">
                    {window.location.host}
                  </span>
                  . Turning off "On my network" disconnects this device.
                </>
              )}
              {pendingCutoff?.kind === "relay" &&
                'You\'re connected through the relay. Turning off "From anywhere" disconnects this device.'}
              {pendingCutoff?.kind === "master" &&
                "Turning remote access off disconnects every device, including this one."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={confirmCutoff}>Continue</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
