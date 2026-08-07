/**
 * Settings → Source Control.
 *
 * Two jobs, in the order a user hits them:
 *
 * 1. **Diagnostics.** When PR/MR UI is missing the cause is almost always
 *    a CLI that is absent or signed out, and those have different fixes.
 *    Each row therefore states which of the two it is and shows the exact
 *    command or URL that resolves it.
 * 2. **Self-hosted classification.** Detection reads a remote's hostname.
 *    `git.acme.internal` reveals nothing, so this is where a user says
 *    what that server runs. Written to synced settings, which backend
 *    detection treats as its highest-priority input.
 *
 * Products with no adapter are still listed, dimmed. Omitting them would
 * leave a user wondering whether Codemux failed to detect their host or
 * simply does not support it.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, Loader2, Plus, RotateCw, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  ALL_PROVIDER_KINDS,
  CUSTOM_HOST_KINDS,
  resolveProvider,
  type ProviderKind,
  type ProviderPresentation,
} from "@/lib/source-control";
import { discoverSourceControl } from "@/tauri/commands";
import type { ProviderCapabilities, ProviderDiagnostic } from "@/tauri/types";

/** Placeholder for a product the backend did not report on at all. */
const EMPTY_CAPABILITIES: ProviderCapabilities = {
  has_pull_requests: false,
  has_checks: false,
  has_issues: false,
  has_inline_comments: false,
  has_review_threads: false,
  has_deployments: false,
  has_reviews: false,
  has_fork_pr_fetch: false,
};
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { SubsectionHeader } from "./settings-primitives";

// ── Diagnostics ──────────────────────────────────────────────────────

type RowTone = "ready" | "attention" | "inert";

function toneOf(row: ProviderDiagnostic): RowTone {
  if (!row.supported) return "inert";
  return row.cli_installed && row.authenticated ? "ready" : "attention";
}

const TONE_DOT: Record<RowTone, string> = {
  ready: "bg-success",
  attention: "bg-warning",
  inert: "bg-muted-foreground/40",
};

/**
 * What this product's adapter actually serves, in the order a user cares
 * about. Read off the backend's declared capabilities rather than
 * restated here, so a product that does not serve a surface (GitLab and
 * deployment environments, today) says so without this file knowing why.
 */
function servedFeatures(
  row: ProviderDiagnostic,
  provider: ProviderPresentation,
): string[] {
  const caps = row.capabilities;
  if (!caps) return [];
  const labels: [boolean, string][] = [
    [caps.has_pull_requests, provider.nounPlural],
    [caps.has_issues, "issues"],
    [caps.has_checks, "checks"],
    [caps.has_inline_comments, "inline review"],
    [caps.has_deployments, "deployments"],
  ];
  return labels.filter(([on]) => on).map(([, label]) => label);
}

/** "a, b and c" — an Oxford-comma-free list, because these are short. */
function formatList(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

function statusLabel(row: ProviderDiagnostic): string {
  if (!row.supported) return "Not yet supported";
  if (!row.cli_installed) return "CLI not installed";
  if (!row.authenticated) return "Not signed in";
  return "Ready";
}

/** Mask an account name until the user asks for it. Length is preserved
 *  (capped) so the field still reads as populated. */
function maskAccount(account: string): string {
  return "•".repeat(Math.min(Math.max(account.length, 3), 12));
}

function AccountValue({ account }: { account: string }) {
  const [revealed, setRevealed] = useState(false);
  return (
    <button
      type="button"
      onClick={() => setRevealed((v) => !v)}
      title={revealed ? "Hide account" : "Click to reveal account"}
      aria-label={revealed ? "Hide account name" : "Reveal account name"}
      className="font-mono text-[12px] text-foreground/90 rounded px-1 -mx-1 hover:bg-muted/60 transition-colors"
    >
      {revealed ? account : maskAccount(account)}
    </button>
  );
}

function ProviderRow({
  provider,
  row,
}: {
  provider: ProviderPresentation;
  row: ProviderDiagnostic;
}) {
  const [open, setOpen] = useState(false);
  const tone = toneOf(row);
  const { Icon } = provider;

  // The single actionable sentence for a non-ready state. Install beats
  // sign-in: there is no point telling someone to run a command whose
  // binary they do not have. The not-installed line names the binary
  // itself, which is why the row above it is suppressed in that case —
  // repeating "glab" twice reads as noise, not emphasis.
  const fixIt =
    tone !== "attention"
      ? null
      : !row.cli_installed
        ? { prefix: `Install ${provider.cli} from `, code: provider.installUrl }
        : { prefix: "Run ", code: provider.loginCommand };

  return (
    <div
      data-testid={`source-control-row-${row.kind}`}
      data-status={tone}
      className={cn(
        "rounded-lg border border-border/60 bg-muted/30 px-3 py-2.5",
        tone === "inert" && "opacity-55",
      )}
    >
      <div className="flex items-center gap-2.5 min-w-0">
        <span
          aria-hidden
          className={cn("size-1.5 shrink-0 rounded-full", TONE_DOT[tone])}
        />
        <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <span className="text-[13px] font-medium text-foreground truncate">
          {provider.name}
        </span>
        <span className="ml-auto shrink-0 text-[11px] text-muted-foreground/80">
          {statusLabel(row)}
        </span>
        {row.detail && (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            aria-label={open ? "Hide details" : "Show details"}
            aria-expanded={open}
            className="shrink-0 rounded p-0.5 text-muted-foreground/60 hover:text-foreground hover:bg-muted/60 transition-colors"
          >
            <ChevronRight
              className={cn(
                "size-3.5 transition-transform",
                open && "rotate-90",
              )}
              aria-hidden
            />
          </button>
        )}
      </div>

      {row.supported && row.cli_installed && (
        <div className="mt-1.5 pl-[1.375rem] flex flex-wrap items-center gap-x-3 gap-y-1">
          <span className="font-mono text-[11px] text-muted-foreground/70">
            {row.cli_version ?? provider.cli}
          </span>
          {row.account && <AccountValue account={row.account} />}
        </div>
      )}

      {row.supported && servedFeatures(row, provider).length > 0 && (
        <p className="mt-1.5 pl-[1.375rem] text-[11px] text-muted-foreground/70">
          Serves {formatList(servedFeatures(row, provider))}
        </p>
      )}

      {fixIt?.code && (
        <p className="mt-1.5 pl-[1.375rem] text-[11px] text-muted-foreground/80">
          {fixIt.prefix}
          <code className="font-mono text-foreground/90 break-all">
            {fixIt.code}
          </code>
        </p>
      )}

      {open && row.detail && (
        <p className="mt-2 pl-[1.375rem] text-[11px] leading-relaxed text-muted-foreground/70 max-w-prose">
          {row.detail}
        </p>
      )}
    </div>
  );
}

// ── Custom hosts ─────────────────────────────────────────────────────

/**
 * Reduce user input to the bare hostname detection compares against.
 *
 * People paste what they have — a full clone URL, a URL with a path, a
 * host with a trailing slash. All of those name the same server, so
 * accept them rather than rejecting on formatting.
 */
export function normalizeHostInput(raw: string): string | null {
  let value = raw.trim().toLowerCase();
  if (value === "") return null;
  // scheme://…
  const schemeSplit = value.split("://");
  if (schemeSplit.length > 1) value = schemeSplit[1];
  // user@host or scp-style git@host:path
  const atIndex = value.lastIndexOf("@");
  if (atIndex >= 0) value = value.slice(atIndex + 1);
  // strip path / query / port
  value = value.split(/[/:?#]/)[0];
  if (value === "") return null;
  // A hostname, not a sentence. Also rejects the wildcard-ish input a
  // user might try, which detection has no way to honour.
  if (!/^[a-z0-9.-]+$/.test(value)) return null;
  return value;
}

function CustomHostsEditor() {
  const customHosts = useSyncedSettingsStore(
    (s) => s.settings.source_control?.custom_hosts,
  );
  const updateSyncedSetting = useSyncedSettingsStore((s) => s.updateSetting);

  const [draftHost, setDraftHost] = useState("");
  const [draftKind, setDraftKind] = useState<ProviderKind>("gitlab");
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const entries = useMemo(
    () => Object.entries(customHosts ?? {}).sort(([a], [b]) => a.localeCompare(b)),
    [customHosts],
  );

  const write = useCallback(
    (next: Record<string, string>) => {
      void updateSyncedSetting("source_control", "custom_hosts", next).catch(
        () => {},
      );
    },
    [updateSyncedSetting],
  );

  const handleAdd = useCallback(() => {
    const host = normalizeHostInput(draftHost);
    if (!host) {
      setError("Enter a hostname, for example git.acme.internal");
      return;
    }
    write({ ...(customHosts ?? {}), [host]: draftKind });
    setDraftHost("");
    setAdding(false);
    setError(null);
  }, [draftHost, draftKind, customHosts, write]);

  const handleRemove = useCallback(
    (host: string) => {
      const next = { ...(customHosts ?? {}) };
      delete next[host];
      write(next);
    },
    [customHosts, write],
  );

  return (
    <div className="space-y-1.5">
      {entries.map(([host, kind]) => (
        <div
          key={host}
          data-testid={`custom-host-${host}`}
          className="group/host flex items-center gap-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2"
        >
          <span className="font-mono text-[12px] text-foreground truncate min-w-0 flex-1">
            {host}
          </span>
          <Select
            value={resolveProvider(kind).kind}
            onValueChange={(v) =>
              write({ ...(customHosts ?? {}), [host]: v })
            }
          >
            <SelectTrigger
              className="h-7 w-[7.5rem] shrink-0 text-[12px]"
              aria-label={`Product for ${host}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {CUSTOM_HOST_KINDS.map((k) => (
                <SelectItem key={k} value={k}>
                  {resolveProvider(k).name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={() => handleRemove(host)}
            aria-label={`Remove ${host}`}
            className="shrink-0 opacity-0 group-hover/host:opacity-100 focus-visible:opacity-100 transition-opacity hover:bg-destructive/10 hover:text-destructive"
          >
            <X className="size-3.5" />
          </Button>
        </div>
      ))}

      {adding ? (
        <div className="space-y-3 rounded-lg border border-border/60 bg-muted/30 p-3">
          <div className="flex flex-wrap items-end gap-3">
            <div className="space-y-1.5 min-w-[12rem] flex-1">
              <Label
                htmlFor="custom-host-name"
                className="text-[11px] text-muted-foreground/85 font-normal"
              >
                Hostname
              </Label>
              <Input
                id="custom-host-name"
                placeholder="git.acme.internal"
                value={draftHost}
                onChange={(e) => {
                  setDraftHost(e.target.value);
                  if (error) setError(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleAdd();
                }}
                autoFocus
                className="h-8 text-[13px] font-mono"
              />
            </div>
            <div className="space-y-1.5">
              <Label
                htmlFor="custom-host-kind"
                className="text-[11px] text-muted-foreground/85 font-normal"
              >
                Runs
              </Label>
              <Select
                value={draftKind}
                onValueChange={(v) => setDraftKind(v as ProviderKind)}
              >
                <SelectTrigger
                  id="custom-host-kind"
                  className="h-8 w-[7.5rem] text-[13px]"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {CUSTOM_HOST_KINDS.map((k) => (
                    <SelectItem key={k} value={k}>
                      {resolveProvider(k).name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          {error && (
            <p className="text-[11px] text-destructive">{error}</p>
          )}
          <div className="flex justify-end gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-3 text-[12px]"
              onClick={() => {
                setAdding(false);
                setDraftHost("");
                setError(null);
              }}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              className="h-7 px-3 text-[12px]"
              onClick={handleAdd}
            >
              Add
            </Button>
          </div>
        </div>
      ) : (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setAdding(true)}
          className="w-full justify-start gap-2 h-8 px-2.5 text-[13px] text-muted-foreground hover:text-foreground hover:bg-muted/40 border border-dashed border-border/60"
        >
          <Plus className="size-3.5" />
          Add self-hosted server
        </Button>
      )}
    </div>
  );
}

// ── Section ──────────────────────────────────────────────────────────

export function SourceControlSection() {
  const [rows, setRows] = useState<ProviderDiagnostic[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const rescan = useCallback(async () => {
    setScanning(true);
    setError(null);
    try {
      setRows(await discoverSourceControl());
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void rescan();
    // Mount only — the Rescan button drives every subsequent probe.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Render in the map's canonical order and fill gaps, so a backend that
  // returns fewer rows (older build, partial probe) still shows every
  // product rather than silently dropping one. Where a backend row
  // exists it is used verbatim — the adapter is what decides whether a
  // product is supported and what it serves; the map below only supplies
  // copy for a row that never arrived.
  const ordered = useMemo(() => {
    const byKind = new Map((rows ?? []).map((r) => [r.kind, r]));
    return ALL_PROVIDER_KINDS.map((kind) => {
      const provider = resolveProvider(kind);
      const row: ProviderDiagnostic = byKind.get(kind) ?? {
        kind,
        supported: provider.supported,
        cli_installed: false,
        cli_version: null,
        authenticated: false,
        account: null,
        detail: null,
        capabilities: EMPTY_CAPABILITIES,
      };
      return { provider, row };
    });
  }, [rows]);

  return (
    <div>
      <SubsectionHeader
        title="Providers"
        description="Codemux drives each product through its own command-line tool, using the credentials that tool already holds. Nothing is stored here."
        action={
          <Button
            variant="outline"
            size="sm"
            onClick={() => void rescan()}
            disabled={scanning}
            aria-label="Rescan source control providers"
          >
            <RotateCw
              className={cn("mr-1 size-3", scanning && "animate-spin")}
              aria-hidden
            />
            Rescan
          </Button>
        }
      />

      {error && (
        <p
          data-testid="source-control-error"
          className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-[11px] text-destructive"
        >
          Could not probe source control providers: {error}
        </p>
      )}

      {rows === null && scanning ? (
        <div className="flex items-center gap-2 py-6 text-[13px] text-muted-foreground">
          <Loader2 className="size-4 animate-spin" aria-hidden />
          Checking installed tooling…
        </div>
      ) : (
        <div className="space-y-1.5">
          {ordered.map(({ provider, row }) => (
            <ProviderRow key={row.kind} provider={provider} row={row} />
          ))}
        </div>
      )}

      <section className="mt-10">
        <SubsectionHeader
          title="Self-hosted servers"
          description="Tell Codemux which product a server runs when its hostname doesn't say — a self-hosted instance on your own domain, for example. Everything else is detected automatically."
        />
        <CustomHostsEditor />
      </section>
    </div>
  );
}
