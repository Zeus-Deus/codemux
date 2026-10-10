import { useCallback, useEffect, useRef, useState } from "react";
import { Copy, Loader2 } from "lucide-react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { useUIStore } from "@/stores/ui-store";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import {
  agentConnectorApprove, agentConnectorDeny, agentConnectorRevoke,
  agentConnectorSetConfig, agentConnectorStatus,
} from "@/tauri/commands";
import { onAgentConnectorChanged, type UnlistenFn } from "@/tauri/events";
import type { AgentConnectorAccess, AgentConnectorClient, AgentConnectorPending, AgentConnectorStatus } from "@/tauri/types";

/** Inbound connections are independent of the agent's outbound MCP servers. */
export function OutsideAssistantsSection() {
  if (isRemoteClient()) {
    return (
      <section className="mt-6 border-t border-border/50 pt-6">
        <h3 className="text-body font-semibold">Outside assistants</h3>
        <p className="mt-1 text-body-sm text-muted-foreground">Manage connections from the desktop</p>
      </section>
    );
  }
  return <NativeOutsideAssistantsSection />;
}

function NativeOutsideAssistantsSection() {
  const [status, setStatus] = useState<AgentConnectorStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [subscriptionError, setSubscriptionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [verified, setVerified] = useState(false);
  const [origin, setOrigin] = useState("");
  const mounted = useRef(false);
  const generation = useRef(0);
  const mutating = useRef(false);
  const originEdited = useRef(false);
  const fresh = useRef(false);
  const eventDuringMutation = useRef(false);

  const reload = useCallback(async (preserveError = false) => {
    const request = ++generation.current;
    fresh.current = false;
    setVerified(false);
    if (!preserveError) setError(null);
    try {
      const next = await agentConnectorStatus();
      if (!mounted.current || request !== generation.current) return;
      setStatus(next);
      fresh.current = true;
      setVerified(true);
      if (!originEdited.current) setOrigin(next.publicOrigin ?? "");
    } catch {
      if (mounted.current && request === generation.current) {
        setError("Unable to verify connections. Refresh before making changes.");
      }
    }
  }, []);

  const invalidate = useCallback(() => {
    if (!mounted.current) return;
    if (mutating.current) {
      eventDuringMutation.current = true;
      ++generation.current;
      fresh.current = false;
      setVerified(false);
    } else void reload();
  }, [reload]);

  useEffect(() => {
    if (!status) return;
    const expiry = Math.min(...[...status.pending, ...status.clients].map((row) => Date.parse(row.expiresAt)).filter(Number.isFinite));
    if (!Number.isFinite(expiry)) return;
    const timer = window.setTimeout(invalidate, Math.max(0, expiry - Date.now()));
    return () => window.clearTimeout(timer);
  }, [status, invalidate]);

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    const unlisten: UnlistenFn[] = [];
    const onChange = () => { if (!disposed) invalidate(); };
    // Both event streams are invalidation only; never consume their payloads.
    const subscriptions = [onAgentConnectorChanged(onChange), onWebRemoteStateChanged(onChange)];
    void Promise.all(subscriptions.map((subscription) => subscription.then((dispose) => {
      if (disposed) { dispose(); return; }
      unlisten.push(dispose);
    }).catch(() => {
      if (!disposed) setSubscriptionError("Live connection updates are unavailable. Reopen Settings to retry.");
    }))).then(() => {
      // Subscribe before reading, so a change cannot fall into a blind gap.
      if (!disposed) void reload();
    });
    window.addEventListener("focus", onChange);
    const onVisible = () => { if (document.visibilityState === "visible") onChange(); };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      disposed = true; mounted.current = false; fresh.current = false; ++generation.current;
      window.removeEventListener("focus", onChange);
      document.removeEventListener("visibilitychange", onVisible);
      for (const dispose of unlisten) dispose();
    };
  }, [reload, invalidate]);

  const mutate = async (action: () => Promise<unknown>, failure: string, afterWrite?: () => void) => {
    if (!status || !fresh.current || mutating.current) return;
    if ([...status.pending, ...status.clients].some((row) => Date.parse(row.expiresAt) <= Date.now())) {
      invalidate();
      return;
    }
    mutating.current = true;
    ++generation.current;
    setBusy(true);
    fresh.current = false;
    setVerified(false);
    setError(null);
    try {
      await action();
      if (mounted.current) afterWrite?.();
    } catch {
      if (mounted.current) setError(failure);
    } finally {
      if (mounted.current) {
        do {
          eventDuringMutation.current = false;
          await reload(true);
        } while (eventDuringMutation.current && mounted.current);
        if (mounted.current) setBusy(false);
      }
      mutating.current = false;
    }
  };

  const updateConfig = (enabled: boolean, publicOrigin: string | null, savedOrigin = false) => mutate(
    () => agentConnectorSetConfig({ enabled, publicOrigin }),
    "Unable to update HTTP connections. Check your desktop configuration and retry.",
    () => { if (savedOrigin) originEdited.current = false; },
  );

  const saveOrigin = () => {
    if (!status) return;
    if (origin !== "" && !isHttpsOrigin(origin)) {
      setError("Use an HTTPS origin only — no credentials, path, query or fragment.");
      return;
    }
    void updateConfig(status.enabled, origin || null, true);
  };
  const locked = busy || !verified;
  return (
    <section aria-labelledby="outside-assistants-heading" className="mt-6 border-t border-border/50 pt-6">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 id="outside-assistants-heading" className="text-body font-semibold">Outside assistants</h3>
          <p className="mt-1 text-body-sm text-muted-foreground">Let another assistant use CodeMux tools. Separate from the MCP servers used by in-app agents.</p>
        </div>
        <Button variant="ghost" size="sm" disabled={busy} aria-label="Refresh outside assistants" onClick={() => { setError(null); void reload(); }}>Refresh</Button>
      </div>
      {error && <p role="alert" className="mt-3 text-label text-destructive">{error}</p>}
      {subscriptionError && <p role="alert" className="mt-3 text-label text-destructive">{subscriptionError}</p>}
      {!status ? (
        !error && <p className="mt-3 flex items-center gap-2 text-body-sm text-muted-foreground"><Loader2 className="size-3.5 animate-spin" aria-hidden />Loading connections…</p>
      ) : (
        <div className="mt-3 divide-y divide-border/40 rounded-md border border-border/50">
          <div className="space-y-1.5 p-3">
            <p className="text-body-sm font-medium">Local assistant · STDIO</p>
            <CopySetup key={status.localCommand} value={status.localCommand} label="Copy local command" />
            <p className="text-label text-muted-foreground">Use this command in a local MCP client. No HTTP listener needed.</p>
          </div>
          <div className="space-y-3 p-3">
            <div className="flex items-center justify-between gap-3">
              <div>
                <p className="text-body-sm font-medium">HTTP MCP connections</p>
                <p className="mt-0.5 text-label text-muted-foreground">{status.enabled ? "Enabled" : "Off"} · Uses the existing Remote Access listener; never starts or exposes it automatically.</p>
              </div>
              <Switch aria-label="Enable HTTP MCP connections" checked={status.enabled} disabled={locked} onCheckedChange={(enabled) => void updateConfig(enabled, status.publicOrigin)} />
            </div>
            {!verified && <p role="status" className="text-label text-muted-foreground">{busy ? "Verifying changes…" : "Checking current state…"} Showing last verified settings.</p>}
            {!status.listenerRunning && (
              <div className="flex flex-wrap items-center justify-between gap-2">
                <p className="text-label text-muted-foreground">Remote Access listener is offline. Enable its HTTP listener to accept connections.</p>
                <Button variant="outline" size="sm" onClick={() => useUIStore.getState().setShowSettings(true, "remote_access")}>Open Remote Access</Button>
              </div>
            )}
            {verified && status.enabled && status.listenerRunning && status.mcpUrl && <CopySetup key={status.mcpUrl} value={status.mcpUrl} label="Copy MCP URL" />}
            <p className="text-label text-muted-foreground">Add the MCP URL in your assistant, start its OAuth connection, approve the request here on this desktop, then press Continue in the authorization browser.</p>
            <div className="space-y-1.5">
              <label htmlFor="connector-public-origin" className="text-label font-medium">Public HTTPS origin</label>
              <div className="flex items-center gap-2">
                <Input id="connector-public-origin" value={origin} placeholder="https://codemux.example.com" disabled={locked} onChange={(e) => { originEdited.current = true; setOrigin(e.target.value); }} aria-describedby="connector-origin-help" />
                <Button variant="outline" size="sm" disabled={locked} onClick={saveOrigin}>Save origin</Button>
              </div>
              <p id="connector-origin-help" className="text-label leading-relaxed text-muted-foreground">Hosted assistants need a reachable HTTPS reverse proxy to this listener. Saving an origin does not set up or verify the proxy. The iroh relay does not support hosted HTTP MCP. Leave blank for local HTTP only.</p>
            </div>
          </div>
          {status.pending.length > 0 && (
            <div className="space-y-3 p-3">
              <h4 className="text-body-sm font-medium">Authorization requests</h4>
              {status.pending.map((request) => (
                <PendingConnection key={request.id} request={request} locked={locked} canApprove={status.enabled && status.listenerRunning}
                  onApprove={(access) => void mutate(() => agentConnectorApprove(request.id, access), "Unable to approve this request. Refresh and retry.")}
                  onDeny={() => void mutate(() => agentConnectorDeny(request.id), "Unable to deny this request. Refresh and retry.")} />
              ))}
            </div>
          )}
          <div className="space-y-3 p-3">
            <h4 className="text-body-sm font-medium">Approved assistants</h4>
            {status.clients.length === 0 ? <p className="text-label text-muted-foreground">No approved assistants.</p> : status.clients.map((client) => (
              <GrantedConnection key={client.id} client={client} locked={locked}
                onRevoke={() => void mutate(() => agentConnectorRevoke(client.id), "Unable to revoke access. Refresh and retry.")} />
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

const ACCESS_LABELS: Record<AgentConnectorAccess, string> = {
  read_only: "Read-only", supervised: "Supervised", full_access: "Full access",
};

function PendingConnection({ request, locked, canApprove, onApprove, onDeny }: {
  request: AgentConnectorPending; locked: boolean; canApprove: boolean;
  onApprove: (access: AgentConnectorAccess) => void; onDeny: () => void;
}) {
  const [selectedAccess, setAccess] = useState<AgentConnectorAccess>("read_only");
  const undecided = request.phase === "awaiting_approval";
  const access = request.phase === "approved_awaiting_client" ? request.access : selectedAccess;
  return (
    <div className="space-y-2" role="group" aria-label={`Connection request from ${request.clientName}`}>
      <div>
        <p className="break-all text-body-sm font-medium">{request.clientName}</p>
        <p className="break-all text-label text-muted-foreground">Registered callback origin: {request.callbackOrigin}</p>
        <p className="text-label text-muted-foreground">Client name and callback origin are self-asserted, not proof of identity.</p>
        <p className="text-caption text-muted-foreground">Requested {displayDate(request.requestedAt)}</p>
      </div>
      <p className="text-label text-muted-foreground">Applies to all CodeMux workspaces and visible conversations, not just this project.</p>
      <p className="text-label text-muted-foreground">Conversation reads include visible user and top-level assistant messages, not reasoning or tool payloads.</p>
      {request.phase !== "denied" && <p className={access === "full_access" ? "text-label text-destructive" : "text-label text-muted-foreground"}>
        {access === "read_only" ? "Can read workspace state and visible conversations; cannot launch workers, send messages or change state." : access === "supervised"
          ? "Can start workers and send messages in supervised modes; some modes allow file edits. You handle approval requests in CodeMux; the assistant cannot approve its own requests."
          : "Warning: Full access can run commands and change files with your account’s permissions, not a project-confined sandbox. Approve only an assistant you trust."}
      </p>}
      {request.phase === "approved_awaiting_client" && <div className="space-y-1">
        <p className="text-label font-medium">Approved · {ACCESS_LABELS[request.access]} · waiting for the assistant</p>
        <p className="text-label text-muted-foreground">Press Continue in the authorization browser, then let the assistant finish connecting. Access is not granted until its token exchange completes.</p>
      </div>}
      {request.phase === "denied" && <p className="text-label text-muted-foreground">Denied · no access was granted. The request will clear when the browser continues or it expires.</p>}
      {undecided && <div className="flex flex-wrap items-center gap-2">
        <select aria-label={`Access for ${request.clientName}`} value={access} disabled={locked}
          className="h-8 rounded-md border border-input bg-background px-2 text-label focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onChange={(event) => setAccess(event.target.value as AgentConnectorAccess)}>
          {Object.entries(ACCESS_LABELS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
        </select>
        <Button variant="outline" size="sm" disabled={locked || !canApprove} onClick={() => onApprove(access)}>Approve</Button>
        <Button variant="ghost" size="sm" disabled={locked} onClick={onDeny}>Deny</Button>
      </div>}
    </div>
  );
}

function GrantedConnection({ client, locked, onRevoke }: {
  client: AgentConnectorClient; locked: boolean; onRevoke: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  return (
    <div role="group" aria-label={`Approved assistant ${client.clientName}`} className="space-y-2">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="break-all text-body-sm font-medium">{client.clientName}</p>
          <p className="break-all text-label text-muted-foreground">{client.callbackOrigin} · {ACCESS_LABELS[client.access]}</p>
          <p className="text-caption text-muted-foreground">Approved {displayDate(client.createdAt)} · Expires {displayDate(client.expiresAt)}</p>
        </div>
        {!confirm && <Button variant="ghost" size="sm" disabled={locked} onClick={() => setConfirm(true)}>Revoke</Button>}
      </div>
      {confirm && <div className="flex flex-wrap items-center gap-2">
        <p className="flex-1 text-label">Revoke this assistant’s access? It will need approval to reconnect.</p>
        <Button variant="ghost" size="sm" disabled={locked} onClick={() => setConfirm(false)}>Cancel</Button>
        <Button variant="destructive" size="sm" disabled={locked} onClick={onRevoke}>Revoke access</Button>
      </div>}
    </div>
  );
}

function displayDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
}

function isHttpsOrigin(value: string): boolean {
  // Check the literal input before URL parsing can normalize paths or slashes.
  if (/[\u0000-\u001f\u007f]/.test(value) || !/^https:\/\/[^/?#\\\s@]+\/?$/.test(value)) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !!url.hostname && !url.username && !url.password && url.pathname === "/" && !url.search && !url.hash;
  } catch { return false; }
}

function CopySetup({ value, label }: { value: string; label: string }) {
  const [feedback, setFeedback] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  return (
    <div>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 select-text break-all rounded-md bg-muted/40 px-2 py-1.5 font-mono text-label">{value}</code>
        <Button variant="ghost" size="icon-sm" aria-label={label} disabled={copying} onClick={async () => {
          setCopying(true);
          try { setFeedback(await copyToClipboard(value) ? "Copied" : COPY_FAILED_MESSAGE); }
          catch { setFeedback(COPY_FAILED_MESSAGE); }
          finally { setCopying(false); }
        }}><Copy className="size-3.5" aria-hidden /></Button>
      </div>
      {feedback && <p role="status" className="mt-1 text-label text-muted-foreground">{feedback}</p>}
    </div>
  );
}
