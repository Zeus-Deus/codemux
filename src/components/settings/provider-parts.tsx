/**
 * Small building blocks shared by the Providers page and the provider
 * detail panels (built-in agents, Hermes, custom ACP agents).
 */
import { Copy } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { copyToClipboard, COPY_FAILED_MESSAGE } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

export type StatusTone = "ok" | "warn" | "error" | "off" | "pending";

export interface ProviderStatus {
  tone: StatusTone;
  label: string;
}

const DOT: Record<StatusTone, string> = {
  ok: "bg-success",
  warn: "bg-warning",
  error: "bg-destructive",
  off: "border border-muted-foreground/60",
  pending: "bg-muted-foreground/40",
};

/** A status dot plus its words. The words carry the meaning; the dot only
 *  helps scanning, so a theme without distinct greens and reds still reads. */
export function StatusLine({ status, className }: { status: ProviderStatus; className?: string }) {
  return (
    <span className={cn("inline-flex min-w-0 items-center gap-1.5 text-label text-muted-foreground", className)}>
      <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", DOT[status.tone])} />
      <span className="truncate">{status.label}</span>
    </span>
  );
}

/** One block of a provider's detail panel: a quiet heading, an optional
 *  action on the right, then its content. */
export function DetailSection({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description?: React.ReactNode;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section aria-label={title} className="border-t border-hairline px-5 py-4">
      <div className="mb-2.5 flex items-center justify-between gap-3">
        <h4 className="text-label font-medium text-muted-foreground">{title}</h4>
        {action}
      </div>
      {description && <p className="-mt-1 mb-3 text-body-sm text-muted-foreground">{description}</p>}
      {children}
    </section>
  );
}

/** A bordered row inside a detail section: an account, a profile, an agent
 *  setting. */
export function DetailRow({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-h-12 items-center gap-3 rounded-md border border-hairline bg-surface-1 px-3 py-2", className)}>
      {children}
    </div>
  );
}

/** A command the user runs themselves (sign in, install), with Copy. */
export function CommandRow({ command }: { command: string }) {
  return (
    <DetailRow>
      <code className="min-w-0 flex-1 truncate font-mono text-label text-foreground" title={command}>{command}</code>
      <Button
        type="button"
        variant="ghost"
        size="xs"
        onClick={() => {
          void copyToClipboard(command).then((ok) =>
            ok ? toast.success("Copied") : toast.error(COPY_FAILED_MESSAGE),
          );
        }}
      >
        <Copy />
        Copy
      </Button>
    </DetailRow>
  );
}

/** Header shared by every provider's detail panel. */
export function DetailHeader({
  logo,
  name,
  tag,
  status,
  version,
  actions,
  enabled,
  onEnabledChange,
  switchDisabled,
  switchTitle,
}: {
  logo: React.ReactNode;
  name: string;
  tag?: string;
  status: ProviderStatus;
  version?: string | null;
  actions?: React.ReactNode;
  enabled?: boolean;
  onEnabledChange?: (enabled: boolean) => void;
  switchDisabled?: boolean;
  switchTitle?: string;
}) {
  return (
    <div className="flex items-center gap-3 px-5 py-4">
      <span aria-hidden className="flex size-9 shrink-0 items-center justify-center rounded-md border border-hairline bg-surface-2">{logo}</span>
      <div className="min-w-0 flex-1">
        <h3 className="flex items-center gap-2 text-body-lg font-semibold text-foreground">
          <span className="truncate">{name}</span>
          {tag && <span className="rounded-full border border-border px-2 text-label font-normal text-muted-foreground">{tag}</span>}
        </h3>
        <div className="mt-0.5 flex items-center gap-2.5">
          <StatusLine status={status} />
          {version && <span className="font-mono text-label text-muted-foreground/70">{version}</span>}
        </div>
      </div>
      {actions}
      {onEnabledChange && (
        <Switch checked={!!enabled} aria-label={`Use ${name}`} title={switchTitle} disabled={switchDisabled} onCheckedChange={onEnabledChange} />
      )}
    </div>
  );
}
