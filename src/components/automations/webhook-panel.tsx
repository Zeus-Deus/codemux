import { useCallback, useEffect, useState } from "react";

import { Copy, Eye, EyeOff, KeyRound, RefreshCw, ShieldCheck, Webhook } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";
import {
  automationsRotateWebhookToken,
  automationsSetWebhookSignature,
  automationsWebhookCredentials,
  secretsDelete,
  secretsStore,
  webRemoteListEndpoints,
  webRemoteStatus,
  type AutomationView,
  type SecretMeta,
  type WebhookCredentials,
  type WebhookSignature,
} from "@/tauri/commands";
import type { WebRemoteEndpoint } from "@/tauri/types";

/** Path prefix the desktop's Remote access listener serves webhooks on. */
export const WEBHOOK_PATH_PREFIX = "/hooks/automations";

/** Placeholders a webhook automation's prompt can use, for form hints. */
export const WEBHOOK_PLACEHOLDERS = [
  "{{body.path}}",
  "{{headers.name}}",
  "{{query.name}}",
  "{{body}}",
  "{{request}}",
];

const MASK = "••••••••••••";

/** Signature setups people reach for most. `custom` keeps whatever the
 *  user typed. */
const PRESETS: Record<string, { label: string; signature: WebhookSignature | null }> = {
  github: {
    label: "GitHub",
    signature: { header: "x-hub-signature-256", encoding: "hex", prefix: "sha256=" },
  },
  custom: { label: "Custom HMAC-SHA256", signature: null },
};

function errorText(err: unknown): string {
  return typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
}

const ENDPOINT_GROUPS: Record<string, string> = {
  this_device: "This device",
  local_network: "Local network",
  tailscale: "Tailscale",
};

/** Short picker label: where the address works, then the host. */
function endpointLabel(endpoint: WebRemoteEndpoint): string {
  return `${ENDPOINT_GROUPS[endpoint.group] ?? "Other"} · ${endpoint.host}`;
}

/** Endpoints a webhook sender can reach: plain HTTP(S) addresses the
 *  listener actually serves under its bind scope. */
export function webhookEndpoints(
  endpoints: WebRemoteEndpoint[],
  bindScope: string | undefined,
): WebRemoteEndpoint[] {
  return endpoints.filter((endpoint) => {
    if (!/^https?:\/\//.test(endpoint.url)) return false;
    // The listener binds IPv4 (`0.0.0.0` or `127.0.0.1`); only the tailscale
    // scope also binds the tailnet's own addresses, which may be IPv6.
    const ipv6 = endpoint.host.includes(":");
    if (ipv6 && !(bindScope === "tailscale" && endpoint.group === "tailscale")) return false;
    if (bindScope === "loopback") return endpoint.group === "this_device";
    if (bindScope === "tailscale") {
      return endpoint.group === "this_device" || endpoint.group === "tailscale";
    }
    return true;
  });
}

function preferredEndpoint(endpoints: WebRemoteEndpoint[]): WebRemoteEndpoint | null {
  return (
    endpoints.find((e) => e.recommended) ??
    endpoints.find((e) => e.group !== "this_device") ??
    endpoints[0] ??
    null
  );
}

/** URL, token rotation and signature check for a webhook automation. The
 *  URL token and the signing secret are desktop-only: a paired browser is
 *  refused both, and the panel says so instead of failing silently. */
export function WebhookPanel({
  automation,
  onChanged,
}: {
  automation: AutomationView;
  onChanged: (updated: AutomationView) => void;
}) {
  const [credentials, setCredentials] = useState<WebhookCredentials | null>(null);
  const [endpoints, setEndpoints] = useState<WebRemoteEndpoint[]>([]);
  const [endpointUrl, setEndpointUrl] = useState<string | null>(null);
  const [listening, setListening] = useState<boolean | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editingSignature, setEditingSignature] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setRevealed(false);
    setEditingSignature(false);
    setError(null);
    automationsWebhookCredentials(automation.id)
      .then((fresh) => !cancelled && setCredentials(fresh))
      .catch((err) => !cancelled && setError(errorText(err)));
    Promise.all([webRemoteStatus(), webRemoteListEndpoints()])
      .then(([status, list]) => {
        if (cancelled) return;
        const usable = webhookEndpoints(list, status.bind_scope);
        setListening(status.enabled && status.running);
        setEndpoints(usable);
        setEndpointUrl(preferredEndpoint(usable)?.url ?? null);
      })
      .catch(() => !cancelled && setListening(false));
    return () => {
      cancelled = true;
    };
  }, [automation.id]);

  const path = credentials
    ? `${WEBHOOK_PATH_PREFIX}/${credentials.webhook_id}/${revealed ? credentials.token : MASK}`
    : null;
  const base = endpointUrl ?? "http://<this-machine>:4377";
  const fullUrl = credentials
    ? `${base}${WEBHOOK_PATH_PREFIX}/${credentials.webhook_id}/${credentials.token}`
    : null;

  const copy = useCallback(async () => {
    if (!fullUrl) return;
    try {
      await navigator.clipboard.writeText(fullUrl);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      setError(errorText(err));
    }
  }, [fullUrl]);

  const rotate = useCallback(async () => {
    if (
      !window.confirm(
        "Issue a new webhook URL? The current one stops working immediately, so update every sender.",
      )
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setCredentials(await automationsRotateWebhookToken(automation.id));
      setRevealed(false);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }, [automation.id]);

  const removeSignature = useCallback(async () => {
    setBusy(true);
    setError(null);
    const oldSecret = automation.webhook?.secret_ref ?? null;
    try {
      onChanged(await automationsSetWebhookSignature(automation.id, null));
      if (oldSecret) await secretsDelete(oldSecret).catch(() => undefined);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }, [automation.id, automation.webhook?.secret_ref, onChanged]);

  const signature = automation.webhook?.signature ?? null;

  return (
    <div data-testid="webhook-panel" className="space-y-3">
      <Eyebrow>Webhook</Eyebrow>

      <div className="space-y-2 rounded-lg border border-border/60 bg-muted/30 p-3">
        <div className="flex items-center gap-2">
          <Webhook className="size-3.5 shrink-0 text-muted-foreground/70" />
          <span className="text-body-sm text-muted-foreground/85">
            POST to this URL to start a run
          </span>
          {endpoints.length > 1 && (
            <Select value={endpointUrl ?? undefined} onValueChange={setEndpointUrl}>
              <SelectTrigger
                aria-label="Address senders use"
                className="ml-auto h-7 w-auto max-w-[45%] gap-1.5 text-label"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {endpoints.map((endpoint) => (
                  <SelectItem key={endpoint.url} value={endpoint.url}>
                    {endpointLabel(endpoint)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        </div>
        <div className="flex items-center gap-1.5">
          <code
            data-testid="webhook-url"
            className="min-w-0 flex-1 truncate rounded-md border border-border/50 bg-background/60 px-2 py-1.5 font-mono text-label text-foreground/85 select-text"
          >
            {path ? `${base}${path}` : "Loading…"}
          </code>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-7"
            disabled={!credentials}
            aria-label={revealed ? "Hide token" : "Show token"}
            onClick={() => setRevealed((v) => !v)}
          >
            {revealed ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-7"
            disabled={!credentials}
            aria-label="Copy webhook URL"
            onClick={() => void copy()}
          >
            <Copy className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-7"
            disabled={!credentials || busy}
            aria-label="Issue a new URL"
            title="Issue a new URL"
            onClick={() => void rotate()}
          >
            <RefreshCw className="size-3.5" />
          </Button>
        </div>
        {copied && <p className="text-label text-success">Copied. Treat it like a password.</p>}
        {listening === false && (
          <p data-testid="webhook-listener-off" className="text-label leading-relaxed text-warning">
            Remote access is off, so nothing can reach this URL yet.{" "}
            <button
              type="button"
              className="underline underline-offset-2 hover:text-foreground"
              onClick={() => useUIStore.getState().setShowSettings(true, "remote_access")}
            >
              Turn it on in Settings
            </button>{" "}
            and keep this machine awake. Senders outside your network need a tunnel or Tailscale.
          </p>
        )}
      </div>

      <div className="rounded-lg border border-border/60 bg-muted/30 p-3">
        {editingSignature ? (
          <SignatureEditor
            automation={automation}
            onCancel={() => setEditingSignature(false)}
            onSaved={(updated) => {
              onChanged(updated);
              setEditingSignature(false);
            }}
          />
        ) : (
          <div className="flex items-center gap-2">
            <ShieldCheck
              className={cn(
                "size-3.5 shrink-0",
                signature ? "text-success" : "text-muted-foreground/50",
              )}
            />
            <div className="min-w-0 flex-1">
              <p className="text-body-sm text-foreground/90">
                {signature ? "Signature checked" : "No signature check"}
              </p>
              <p
                data-testid="webhook-signature-summary"
                className="truncate text-label text-muted-foreground/70"
              >
                {signature
                  ? `HMAC-SHA256 in ${signature.header}, ${signature.encoding} digest · secret stored encrypted`
                  : "Anyone with the URL can start a run. Add a signing secret if the sender supports one."}
              </p>
            </div>
            {signature && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => void removeSignature()}
              >
                Remove
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => setEditingSignature(true)}
            >
              <KeyRound className="size-3.5" />
              {signature ? "Change" : "Add check"}
            </Button>
          </div>
        )}
      </div>

      {error && <p className="text-body-sm text-destructive">{error}</p>}
    </div>
  );
}

/** Pick a preset (or custom header/encoding/prefix) and enter the signing
 *  secret. The secret is stored encrypted by the backend and never shown
 *  again; replacing it deletes the old one. */
function SignatureEditor({
  automation,
  onCancel,
  onSaved,
}: {
  automation: AutomationView;
  onCancel: () => void;
  onSaved: (updated: AutomationView) => void;
}) {
  const existing = automation.webhook?.signature ?? null;
  const [preset, setPreset] = useState(
    existing && existing.header !== PRESETS.github.signature?.header ? "custom" : "github",
  );
  const [header, setHeader] = useState(existing?.header ?? "x-hub-signature-256");
  const [prefix, setPrefix] = useState(existing?.prefix ?? "sha256=");
  const [encoding, setEncoding] = useState<WebhookSignature["encoding"]>(
    existing?.encoding ?? "hex",
  );
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const choosePreset = (value: string) => {
    setPreset(value);
    const signature = PRESETS[value]?.signature;
    if (signature) {
      setHeader(signature.header);
      setPrefix(signature.prefix);
      setEncoding(signature.encoding);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    const oldSecret = automation.webhook?.secret_ref ?? null;
    let stored: SecretMeta | null = null;
    try {
      stored = await secretsStore(`${automation.name} webhook`, secret);
      // Drop the plaintext from component state as soon as it is stored.
      setSecret("");
      const updated = await automationsSetWebhookSignature(automation.id, {
        header,
        encoding,
        prefix,
        secret_ref: stored.id,
      });
      if (oldSecret) await secretsDelete(oldSecret).catch(() => undefined);
      onSaved(updated);
    } catch (err) {
      // A secret stored for a check that failed to save is unused; remove it.
      if (stored) await secretsDelete(stored.id).catch(() => undefined);
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const canSave = header.trim() !== "" && secret.trim() !== "" && !busy;

  return (
    <div data-testid="webhook-signature-editor" className="space-y-3">
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-1.5">
          <Label className="text-label text-muted-foreground">Sender</Label>
          <Select value={preset} onValueChange={choosePreset}>
            <SelectTrigger className="h-8 text-body-sm">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {Object.entries(PRESETS).map(([value, entry]) => (
                <SelectItem key={value} value={value}>
                  {entry.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="webhook-secret" className="text-label text-muted-foreground">
            Signing secret
          </Label>
          <Input
            id="webhook-secret"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={existing ? "Enter a new secret to replace it" : "Paste the sender's secret"}
            className="h-8 font-mono text-body-sm"
          />
        </div>
      </div>
      {preset === "custom" && (
        <div className="grid grid-cols-3 gap-3">
          <div className="space-y-1.5">
            <Label className="text-label text-muted-foreground">Header</Label>
            <Input
              value={header}
              onChange={(e) => setHeader(e.target.value)}
              className="h-8 font-mono text-body-sm"
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-label text-muted-foreground">Prefix</Label>
            <Input
              value={prefix}
              onChange={(e) => setPrefix(e.target.value)}
              placeholder="none"
              className="h-8 font-mono text-body-sm"
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-label text-muted-foreground">Digest</Label>
            <Select
              value={encoding}
              onValueChange={(v) => setEncoding(v as WebhookSignature["encoding"])}
            >
              <SelectTrigger className="h-8 text-body-sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="hex">Hex</SelectItem>
                <SelectItem value="base64">Base64</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
      )}
      <p className="text-label leading-relaxed text-muted-foreground/70">
        Stored encrypted on this machine and never shown again. It is not added to prompts or logs.
      </p>
      {error && <p className="text-body-sm text-destructive">{error}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={!canSave}
          onClick={() => void save()}
        >
          {busy && <Spinner className="size-3.5" aria-hidden />}
          Save check
        </Button>
      </div>
    </div>
  );
}
