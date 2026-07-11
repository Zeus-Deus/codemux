import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  Check,
  Copy,
  Laptop,
  Link2,
  MonitorSmartphone,
  RefreshCw,
  Server,
  ShieldAlert,
  ShieldCheck,
  Smartphone,
  Tablet,
  Trash2,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import {
  webRemoteApproveSession,
  webRemoteCreatePairing,
  webRemoteDisable,
  webRemoteEnable,
  webRemoteListEndpoints,
  webRemoteRejectSession,
  webRemoteRevokeSession,
  webRemoteSetConfig,
  webRemoteStatus,
} from "@/tauri/commands";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import type {
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

import { useQrSvg } from "./use-qr-svg";
import {
  approvedSessions,
  composePairUrl,
  connectedSessionCount,
  describeDevice,
  endpointSecurityHint,
  formatCountdown,
  groupEndpoints,
  msUntil,
  newlyPendingSessionIds,
  pendingSessions,
  pickPrimaryEndpoint,
  relativeTime,
  validatePort,
  type DeviceKind,
  type EndpointGroupView,
} from "./remote-access-utils";

// ── Clipboard (secure-context aware) ─────────────────────────────────
//
// This panel renders on the desktop (always a secure context) AND on the
// remote web client, which may be a plain-HTTP origin where
// `navigator.clipboard` is undefined. Fall back to the legacy execCommand
// path there so "Copy" still works.
async function copyText(text: string): Promise<boolean> {
  try {
    if (window.isSecureContext && navigator?.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the execCommand path */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

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
        const ok = await copyText(text);
        if (ok) {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1400);
          toast.success("Copied to clipboard");
        } else {
          toast.error("Couldn't copy — select and copy manually.");
        }
      }}
    >
      {copied ? (
        <Check className="h-3.5 w-3.5 text-status-open" />
      ) : (
        <Copy className="h-3.5 w-3.5" />
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
    <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
      {children}
    </p>
  );
}

// ── Endpoint row ─────────────────────────────────────────────────────

function EndpointRow({ endpoint }: { endpoint: WebRemoteEndpoint }) {
  const hint = endpointSecurityHint(endpoint);
  return (
    <div className="flex items-start gap-3 py-2.5">
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <code className="truncate font-mono text-[12.5px] text-foreground">
            {endpoint.url}
          </code>
          {endpoint.recommended && (
            <Badge
              variant="outline"
              className="border-accent-ember/30 bg-accent-ember/10 text-[10px] font-medium text-accent-ember"
            >
              Recommended
            </Badge>
          )}
          {hint.secure ? (
            <Badge
              variant="outline"
              className="gap-1 border-status-open/30 bg-status-open/10 text-[10px] text-status-open"
            >
              <ShieldCheck className="h-3 w-3" />
              {hint.badge}
            </Badge>
          ) : (
            <Badge
              variant="outline"
              className="gap-1 border-status-working/30 bg-status-working/10 text-[10px] text-status-working"
            >
              <ShieldAlert className="h-3 w-3" />
              {hint.badge}
            </Badge>
          )}
        </div>
        <p className="text-[11.5px] leading-relaxed text-muted-foreground/80">
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
        <summary className="flex cursor-pointer list-none items-center gap-1.5 text-[12.5px] font-semibold text-foreground marker:content-none">
          <span className="text-muted-foreground/70 transition-transform group-open:rotate-90">
            ›
          </span>
          {group.title}
          <span className="font-normal text-muted-foreground/70">
            ({group.endpoints.length})
          </span>
        </summary>
        <div className="mt-1.5 space-y-1">
          <p className="text-[11.5px] leading-relaxed text-muted-foreground/70">
            {group.explanation}
          </p>
          <EndpointGroupRows endpoints={group.endpoints} />
        </div>
      </details>
    );
  }
  return (
    <div className="space-y-1">
      <p className="text-[12.5px] font-semibold text-foreground">
        {group.title}
      </p>
      <p className="text-[11.5px] leading-relaxed text-muted-foreground/70">
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
        "rounded-full border px-2.5 py-1 text-[11px] font-medium transition-colors",
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
              <span className="text-[11px] text-neutral-400">No endpoint</span>
            )}
          </div>
        </div>

        <div className="min-w-0 flex-1 space-y-3">
          <div>
            <div className="flex items-center justify-between gap-2">
              <p className="text-[13px] font-semibold text-foreground">
                Scan or share this link
              </p>
              <span
                className={cn(
                  "font-mono text-[11.5px] tabular-nums",
                  expired ? "text-status-attention" : "text-muted-foreground",
                )}
              >
                {expired ? "Expired" : `Expires in ${formatCountdown(remainingMs)}`}
              </span>
            </div>
            <p className="mt-1 text-[11.5px] leading-relaxed text-muted-foreground/80">
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
                  <summary className="flex w-fit cursor-pointer list-none items-center gap-1 text-[11px] font-medium text-muted-foreground marker:content-none hover:text-foreground">
                    <span className="transition-transform group-open:rotate-90">
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
              <Link2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <code className="min-w-0 flex-1 truncate font-mono text-[12px] text-foreground">
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
              className="gap-1.5"
              disabled={regenerating}
              onClick={onRegenerate}
            >
              <RefreshCw
                className={cn("h-3.5 w-3.5", regenerating && "animate-spin")}
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
      <DeviceIcon kind={d.kind} className="h-4 w-4 shrink-0 text-status-working" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold text-foreground">
          {d.title}
        </p>
        <p className="truncate text-[11.5px] text-muted-foreground/80">
          {d.platform || "Unknown platform"} · asked to connect{" "}
          {relativeTime(session.created_at)}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button
          type="button"
          size="sm"
          className="h-7 gap-1.5 bg-status-open/90 text-white hover:bg-status-open"
          disabled={busy}
          onClick={onApprove}
        >
          <Check className="h-3.5 w-3.5" />
          Approve
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="h-7 text-muted-foreground hover:text-status-attention"
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
  return (
    <div className="flex items-center gap-3 py-3">
      <div className="relative shrink-0">
        <DeviceIcon kind={d.kind} className="size-4 text-muted-foreground" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <p className="truncate text-[13px] font-semibold text-foreground">
            {d.title}
          </p>
          {session.connected ? (
            <span className="flex items-center gap-1 text-[11px] font-medium text-status-open">
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-status-open" />
              Connected
            </span>
          ) : (
            <span className="flex items-center gap-1 text-[11px] text-muted-foreground/70">
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground/40" />
              Offline
            </span>
          )}
        </div>
        <p className="truncate text-[11.5px] text-muted-foreground/80">
          {d.platform || "Unknown platform"} · paired{" "}
          {relativeTime(session.created_at)} · last seen{" "}
          {relativeTime(session.last_seen_at)}
        </p>
      </div>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="h-7 shrink-0 gap-1.5 text-muted-foreground hover:text-status-attention"
        disabled={busy}
        onClick={onRevoke}
      >
        <Trash2 className="h-3.5 w-3.5" />
        Revoke
      </Button>
    </div>
  );
}

// ── Main section ─────────────────────────────────────────────────────

export function RemoteAccessSection() {
  const [status, setStatus] = useState<WebRemoteStatus | null>(null);
  const [endpoints, setEndpoints] = useState<WebRemoteEndpoint[]>([]);
  const [pairing, setPairing] = useState<WebRemotePairingInfo | null>(null);
  const [portDraft, setPortDraft] = useState("");
  const [togglePending, setTogglePending] = useState(false);
  const [portPending, setPortPending] = useState(false);
  const [approvalPending, setApprovalPending] = useState(false);
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
            description: "Approve it below to grant access.",
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

  const enabled = status?.enabled ?? false;
  const running = status?.running ?? false;
  const requireApproval = status?.require_approval ?? false;
  const pending = useMemo(() => pendingSessions(status), [status]);
  const approved = useMemo(() => approvedSessions(status), [status]);
  const connectedCount = connectedSessionCount(status);

  const portValidation = validatePort(portDraft);
  const portDirty = status != null && portDraft !== String(status.port);

  const handleToggle = useCallback(
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
            ? "Remote access is on — pair a device to connect."
            : "Remote access is off. The server is no longer listening.",
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

  const handleApplyPort = useCallback(async () => {
    if (!portValidation.valid || portValidation.value == null) return;
    setPortPending(true);
    try {
      const result = await webRemoteSetConfig({ port: portValidation.value });
      applyStatus(result, { detectPending: false });
      setPortDraft(String(result.port));
      if (result.running) void refreshEndpoints();
      // A port change invalidates any composed pairing URL.
      setPairing(null);
      toast.success(`Port set to ${result.port}.`);
    } catch (err) {
      console.error("[remote-access] set port failed:", err);
      toast.error(`Couldn't change the port: ${String(err)}`);
    } finally {
      setPortPending(false);
    }
  }, [applyStatus, portValidation, refreshEndpoints]);

  const handleToggleApproval = useCallback(
    async (next: boolean) => {
      setApprovalPending(true);
      try {
        const result = await webRemoteSetConfig({ requireApproval: next });
        applyStatus(result, { detectPending: false });
        toast.success(
          next
            ? "Approval mode on — new devices wait for you to approve them."
            : "Approval mode off — a valid pairing link connects immediately.",
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
      toast.success("Revoked every paired device.");
    } catch (err) {
      toast.error(`Couldn't revoke every device: ${String(err)}`);
    } finally {
      setRevokingAll(false);
    }
  }, [applyStatus, approved]);

  return (
    <div className="space-y-8">
      {/* Header */}
      <div>
        <div className="flex items-center gap-2">
          <MonitorSmartphone className="size-4 text-accent-ember" />
          <h2 className="text-[21px] font-bold tracking-tight text-foreground">
            Remote Access
          </h2>
        </div>
        <p className="mt-1.5 max-w-prose text-[13.5px] leading-relaxed text-muted-foreground/80">
          Open this desktop to a browser on another device — a laptop or phone
          on your network or mesh VPN — and drive the same projects, sessions,
          and agents from there.
        </p>
      </div>

      {/* Master toggle + exposure warning */}
      <div className="space-y-3 rounded-lg border border-warning/30 bg-warning/5 p-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0 flex-1 space-y-1.5">
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-medium">Enable remote access</h3>
              {running && (
                <Badge
                  variant="outline"
                  className="gap-1 border-status-open/30 bg-status-open/10 text-[10px] text-status-open"
                >
                  <span className="inline-block h-1.5 w-1.5 rounded-full bg-status-open" />
                  Listening on {status?.port}
                </Badge>
              )}
            </div>
            <p className="text-xs leading-relaxed text-muted-foreground">
              Turning this on starts a server that listens on{" "}
              <span className="font-medium text-foreground">
                every network interface
              </span>{" "}
              of this machine. Anyone you pair gets full control of this
              computer — open terminals, run agents, and read or edit your files,
              exactly as if they were sitting here. Pair only devices you trust,
              and revoke them the moment you're done.
            </p>
          </div>
          <Switch
            checked={enabled}
            onCheckedChange={handleToggle}
            disabled={togglePending}
            aria-label="Toggle remote access"
          />
        </div>
      </div>

      {enabled && (
        <>
          {/* Server config */}
          <section className="space-y-4">
            <SubHeading>Server</SubHeading>

            <div className="flex items-end justify-between gap-4">
              <div className="min-w-0 flex-1 space-y-1">
                <label
                  htmlFor="web-remote-port"
                  className="block text-[13px] font-medium leading-none text-foreground"
                >
                  Port
                </label>
                <p className="text-[12px] leading-relaxed text-muted-foreground/85">
                  The port the server binds. Changing it rebinds immediately and
                  invalidates any open pairing link.
                </p>
                {portDraft !== "" && !portValidation.valid && (
                  <p className="text-[11.5px] text-status-attention">
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
                  size="sm"
                  className="h-9"
                  disabled={!portDirty || !portValidation.valid || portPending}
                  onClick={handleApplyPort}
                >
                  Apply
                </Button>
              </div>
            </div>

            <div className="flex items-center justify-between gap-8 border-t border-border/60 pt-4">
              <div className="min-w-0 space-y-1">
                <p className="text-[13px] font-medium leading-tight text-foreground">
                  Require approval for new devices
                </p>
                <p className="text-[12px] leading-relaxed text-muted-foreground/80">
                  When on, a device that opens a valid pairing link waits here
                  until you approve it. When off, a valid link connects right
                  away.
                </p>
              </div>
              <Switch
                checked={requireApproval}
                onCheckedChange={handleToggleApproval}
                disabled={approvalPending}
                aria-label="Toggle approval mode"
              />
            </div>
          </section>

          {/* Endpoints */}
          <section className="space-y-2">
            <SubHeading>Reachable at</SubHeading>
            {endpoints.length === 0 ? (
              <p className="py-2 text-[12.5px] text-muted-foreground/70">
                {running
                  ? "No reachable endpoints found."
                  : "Starting the server…"}
              </p>
            ) : (
              <GroupedEndpoints endpoints={endpoints} />
            )}
          </section>

          {/* Pairing */}
          <section className="space-y-3">
            <SubHeading>Pair a device</SubHeading>
            {pairing ? (
              <PairingPanel
                pairing={pairing}
                endpoints={endpoints}
                regenerating={pairingPending}
                onRegenerate={handleCreatePairing}
              />
            ) : (
              <div className="flex items-center justify-between gap-4 rounded-lg border border-border/60 bg-muted/30 p-4">
                <p className="text-[12.5px] leading-relaxed text-muted-foreground/85">
                  Create a one-time link, then scan its QR code or open it on the
                  other device to pair.
                </p>
                <Button
                  type="button"
                  size="sm"
                  className="shrink-0 gap-1.5"
                  disabled={pairingPending}
                  onClick={handleCreatePairing}
                >
                  <Link2 className="h-3.5 w-3.5" />
                  Create pairing link
                </Button>
              </div>
            )}
          </section>

          {/* Pending approvals */}
          {pending.length > 0 && (
            <section className="space-y-2.5">
              <div className="flex items-center gap-2">
                <SubHeading>Waiting for approval</SubHeading>
                <Badge
                  variant="outline"
                  className="border-status-working/30 bg-status-working/10 text-[10px] text-status-working"
                >
                  {pending.length}
                </Badge>
              </div>
              <div className="space-y-2">
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
            </section>
          )}

          {/* Paired devices */}
          <section className="space-y-2">
            <div className="flex items-center justify-between gap-4">
              <div className="flex items-center gap-2">
                <SubHeading>Paired devices</SubHeading>
                {connectedCount > 0 && (
                  <span className="text-[11px] font-medium text-status-open">
                    {connectedCount} connected
                  </span>
                )}
              </div>
              {approved.length > 0 && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1.5 text-muted-foreground hover:text-status-attention"
                  disabled={revokingAll}
                  onClick={handleRevokeAll}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  Revoke all
                </Button>
              )}
            </div>
            {approved.length === 0 ? (
              <div className="flex items-center gap-2.5 rounded-lg border border-dashed border-border/60 px-3.5 py-4 text-[12.5px] text-muted-foreground/70">
                <Server className="h-4 w-4" />
                No devices paired yet. Create a pairing link above to connect
                one.
              </div>
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
    </div>
  );
}
