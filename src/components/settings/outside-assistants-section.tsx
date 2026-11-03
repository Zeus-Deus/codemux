import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Button, InlineButton } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import {
  agentConnectorApprove, agentConnectorDeny, agentConnectorIssue, agentConnectorRevoke,
  agentConnectorRevokeAll, agentConnectorSetConfig, agentConnectorStatus,
} from "@/tauri/commands";
import { onAgentConnectorChanged, type UnlistenFn } from "@/tauri/events";
import type { AgentConnectorAccess, AgentConnectorIssuedToken, AgentConnectorStatus } from "@/tauri/types";
import {
  CopySetup, GrantedConnection, IssuedTokenCard, LocalSetupList, PendingConnection, RevokeAll, TokenForm,
} from "./outside-assistant-rows";
import { expiryTimerDelay, localSetupCommands, tailscaleServeCommand } from "./outside-assistants-setup";
import { validatePort } from "./remote-access-utils";
import { SectionHeader, SettingsCard, SubsectionHeader } from "./settings-primitives";

/** The outside-agent guide, on GitHub until the docs site has the page. */
export const OUTSIDE_AGENTS_GUIDE_URL = "https://github.com/Zeus-Deus/codemux/blob/main/docs/outside-agents.md";

/** The same rule the backend applies to a personal token's label. */
const TOKEN_LABEL = /^[A-Za-z0-9][A-Za-z0-9 ._@-]{0,63}$/;

const DESCRIPTION = "Let Claude Code, Codex, Hermes or any MCP client work with Codemux: open workspaces, start threads, wait for their results and read them.";

/** Settings → Connections → Outside assistants: inbound control of Codemux,
 *  separate from the outbound MCP servers in-app agents use. */
export function OutsideAssistantsSection() {
  if (isRemoteClient()) {
    return <SectionHeader title="Outside assistants" description="Manage connections from the desktop" />;
  }
  return <NativeOutsideAssistantsSection />;
}

function NativeOutsideAssistantsSection() {
  const [status, setStatus] = useState<AgentConnectorStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [subscriptionError, setSubscriptionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [verified, setVerified] = useState(false);
  const [origin, setOrigin] = useState("");
  const [portDraft, setPortDraft] = useState("");
  const [tokenLabel, setTokenLabel] = useState("");
  const [tokenAccess, setTokenAccess] = useState<AgentConnectorAccess>("supervised");
  // The one-time token stays in this component's state only: never a store,
  // event, toast or label. Done and unmount drop it.
  const [issued, setIssued] = useState<AgentConnectorIssuedToken | null>(null);
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
    const delay = expiryTimerDelay([...status.pending, ...status.clients].map((row) => row.expiresAt), Date.now());
    if (delay === null) return;
    const timer = window.setTimeout(invalidate, delay);
    return () => window.clearTimeout(timer);
  }, [status, invalidate]);

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    const onChange = () => { if (!disposed) invalidate(); };
    // The event is invalidation only; its payload is never read. The
    // connector owns its listener, so Remote Access changes don't matter here.
    void onAgentConnectorChanged(onChange).then((dispose) => {
      if (disposed) { dispose(); return; }
      unlisten = dispose;
    }).catch(() => {
      if (!disposed) setSubscriptionError("Live connection updates are unavailable. Reopen Settings to retry.");
    }).then(() => {
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
      unlisten?.();
    };
  }, [reload, invalidate]);

  /** Write, then trust only an authoritative read-back. */
  const mutate = async <T,>(action: () => Promise<T>, failure: string, afterWrite?: (result: T) => void) => {
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
    setNotice(null);
    try {
      const result = await action();
      if (mounted.current) afterWrite?.(result);
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

  const updateConfig = (config: { enabled: boolean; publicOrigin: string | null; port?: number }, savedOrigin = false) => mutate(
    () => agentConnectorSetConfig(config),
    "Unable to update network connections. Check your desktop configuration and retry.",
    (written) => {
      if (savedOrigin) originEdited.current = false;
      if (config.port !== undefined) setPortDraft("");
      const revoked = written.revokedGrants ?? 0;
      if (revoked > 0) {
        setNotice(`The address changed, so ${revoked === 1 ? "1 assistant was" : `${revoked} assistants were`} signed out. ${revoked === 1 ? "It needs" : "They need"} to connect again.`);
      }
    },
  );

  const saveOrigin = () => {
    if (!status) return;
    if (origin !== "" && !isHttpsOrigin(origin)) {
      setError("Use an HTTPS origin only — no credentials, path, query or fragment.");
      return;
    }
    void updateConfig({ enabled: status.enabled, publicOrigin: origin || null }, true);
  };

  const savePort = () => {
    if (!status) return;
    const port = validatePort(portDraft);
    if (port.value === null) {
      setError(port.error ?? "Use a port between 1024 and 65535.");
      return;
    }
    void updateConfig({ enabled: status.enabled, publicOrigin: status.publicOrigin, port: port.value });
  };

  const createToken = () => {
    const label = tokenLabel.trim();
    if (!TOKEN_LABEL.test(label)) {
      setError("Use a token label of up to 64 letters, digits, spaces or . _ @ -, starting with a letter or digit.");
      return;
    }
    void mutate(
      () => agentConnectorIssue({ label, access: tokenAccess }),
      "Unable to create a token. Refresh and retry.",
      (token) => { setIssued(token); setTokenLabel(""); },
    );
  };

  const locked = busy || !verified;
  const localRows = status ? localSetupCommands(status.localExecutable || "codemux") : [];
  const showAccess = !!status && (status.enabled || status.pending.length > 0 || status.clients.length > 0);
  return (
    <div>
      <SectionHeader
        title="Outside assistants"
        description={<>{DESCRIPTION}{" "}<InlineButton tone="muted" onClick={() => void openUrl(OUTSIDE_AGENTS_GUIDE_URL).catch(console.error)}>Setup guide<ExternalLink className="size-3" aria-hidden /></InlineButton></>}
        action={<Button variant="ghost" size="sm" disabled={busy} aria-label="Refresh outside assistants" onClick={() => { setError(null); void reload(); }}>Refresh</Button>}
      />
      {error && <p role="alert" className="mb-3 text-label text-destructive">{error}</p>}
      {subscriptionError && <p role="alert" className="mb-3 text-label text-destructive">{subscriptionError}</p>}
      {!status ? (
        !error && <p className="flex items-center gap-2 text-body-sm text-muted-foreground"><Spinner className="size-3.5" aria-hidden />Loading connections…</p>
      ) : (
        <div className="space-y-8">
          <div>
            <SubsectionHeader title="On this computer" description="Works while Codemux is open. Paste one command into a terminal." />
            <LocalSetupList rows={localRows} />
            <p className="mt-2 text-label leading-relaxed text-muted-foreground">
              These run with your own access; <code className="font-mono">--client</code> only names the assistant on the messages it sends.
              Hermes starts as Supervised because it relays chat messages: it can start and message threads, but approvals stay with you.
            </p>
          </div>

          <div>
            <SubsectionHeader
              title="Over the network"
              description="For an assistant on another device, or one that signs in over HTTP."
              action={<Switch aria-label="Allow network connections" checked={status.enabled} disabled={locked}
                onCheckedChange={(enabled) => void updateConfig({ enabled, publicOrigin: status.publicOrigin })} />}
            />
            {!verified && <p role="status" className="mb-2 text-label text-muted-foreground">{busy ? "Verifying changes…" : "Checking current state…"} Showing last verified settings.</p>}
            {notice && <p role="status" className="mb-2 text-label text-foreground">{notice}</p>}
            {!status.enabled ? (
              <p className="text-label text-muted-foreground">Off. Nothing listens for network connections, and assistants with access can’t connect.</p>
            ) : status.bindError ? (
              <SettingsCard className="space-y-3">
                <div>
                  <p className="text-body-sm font-medium text-destructive">Couldn’t listen on port {status.port}</p>
                  <p className="break-all text-label text-muted-foreground">{status.bindError}</p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Input aria-label="Port" inputMode="numeric" className="w-28 font-mono" placeholder={String(status.port)} value={portDraft} disabled={locked}
                    onChange={(event) => setPortDraft(event.target.value)}
                    onKeyDown={(event) => { if (event.key === "Enter") savePort(); }} />
                  <Button variant="outline" disabled={locked || portDraft.trim() === ""} onClick={savePort}>Use port</Button>
                  <Button variant="ghost" disabled={locked} onClick={() => void updateConfig({ enabled: true, publicOrigin: status.publicOrigin })}>Retry</Button>
                </div>
                <p className="text-label text-muted-foreground">Free the port and retry, or pick another one. A new port signs out every assistant that has access.</p>
              </SettingsCard>
            ) : !status.listenerRunning ? (
              <p className="flex items-center gap-2 text-label text-muted-foreground"><Spinner className="size-3.5" aria-hidden />Starting…</p>
            ) : (
              <SettingsCard className="space-y-3">
                <p className="flex items-center gap-2 text-body-sm text-foreground">
                  <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-status-open" />
                  Listening on this computer, port {status.port}
                </p>
                {status.publicOrigin && status.mcpUrl ? (
                  <div className="space-y-1.5">
                    {verified && <CopySetup key={status.mcpUrl} value={status.mcpUrl} label="Copy MCP URL" />}
                    <p className="text-label leading-relaxed text-muted-foreground">
                      Other devices use this address. Point your HTTPS proxy at{" "}
                      <code className="break-all font-mono text-foreground/90">{status.localUrl}</code>; Codemux doesn’t check the proxy.
                    </p>
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {verified && status.localUrl && <CopySetup key={status.localUrl} value={status.localUrl} label="Copy local MCP URL" />}
                    <p className="text-label text-muted-foreground">Only this computer can reach this address. To connect from another device, serve it over HTTPS, for example with Tailscale:</p>
                    <CopySetup key={status.port} value={tailscaleServeCommand(status.port)} label="Copy Tailscale command" />
                    <p className="text-label text-muted-foreground">then save the HTTPS address here.</p>
                  </div>
                )}
                <div className="space-y-1.5">
                  <label htmlFor="connector-public-origin" className="text-label font-medium">Public HTTPS origin</label>
                  <div className="flex items-center gap-2">
                    <Input id="connector-public-origin" value={origin} placeholder="https://codemux.example.com" disabled={locked} onChange={(e) => { originEdited.current = true; setOrigin(e.target.value); }} aria-describedby="connector-origin-help" />
                    <Button variant="outline" disabled={locked} onClick={saveOrigin}>Save origin</Button>
                  </div>
                  <p id="connector-origin-help" className="text-label leading-relaxed text-muted-foreground">Leave it blank to use this computer only. Changing it signs out every assistant that has access.</p>
                </div>
                <p className="text-label text-muted-foreground">Assistants that support OAuth sign in with this URL; approve them below. Bots that can’t sign in can use a token.</p>
              </SettingsCard>
            )}
          </div>

          {showAccess && (
            <div>
              <SubsectionHeader title="Assistants with access"
                action={status.clients.length > 0 ? <RevokeAll locked={locked}
                  onRevokeAll={() => void mutate(() => agentConnectorRevokeAll(), "Unable to revoke access. Refresh and retry.")} /> : undefined} />
              {!status.enabled && status.clients.length > 0 && (
                <p className="mb-2 text-label text-muted-foreground">Network connections are off, so these can’t connect until you turn them back on.</p>
              )}
              <SettingsCard className="p-0">
                {status.pending.length === 0 && status.clients.length === 0 ? (
                  <p className="px-4 py-3 text-body-sm text-muted-foreground/80">No assistants have network access yet.</p>
                ) : (
                  <ul className="divide-y divide-border/40">
                    {status.pending.map((request) => (
                      <PendingConnection key={request.id} request={request} locked={locked} canApprove={status.enabled && status.listenerRunning}
                        onApprove={(access) => void mutate(() => agentConnectorApprove(request.id, access), "Unable to approve this request. Refresh and retry.")}
                        onDeny={() => void mutate(() => agentConnectorDeny(request.id), "Unable to deny this request. Refresh and retry.")} />
                    ))}
                    {status.clients.map((client) => (
                      <GrantedConnection key={client.id} client={client} locked={locked}
                        onRevoke={() => void mutate(() => agentConnectorRevoke(client.id), "Unable to revoke access. Refresh and retry.")} />
                    ))}
                  </ul>
                )}
              </SettingsCard>
              {/* A token issued while the listener is down would be signed out by the port change that fixes it. */}
              {status.enabled && status.listenerRunning && (
                <div className="mt-3">
                  <TokenForm label={tokenLabel} access={tokenAccess} locked={locked}
                    onLabelChange={setTokenLabel} onAccessChange={setTokenAccess} onCreate={createToken} />
                </div>
              )}
              {issued && <div className="mt-3"><IssuedTokenCard issued={issued} onDone={() => setIssued(null)} /></div>}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function isHttpsOrigin(value: string): boolean {
  // Check the literal input before URL parsing can normalize paths or slashes.
  if (/[\u0000-\u001f\u007f]/.test(value) || !/^https:\/\/[^/?#\\\s@]+\/?$/.test(value)) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !!url.hostname && !url.username && !url.password && url.pathname === "/" && !url.search && !url.hash;
  } catch { return false; }
}
