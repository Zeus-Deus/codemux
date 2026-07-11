/**
 * Web-remote connection indicator.
 *
 * Renders ONLY on the web-remote client (`isRemoteClient()`), driven by
 * `useRemoteConnectionStore` (the bootstrap writes it from the transport's
 * status callbacks). Two surfaces, deliberately split by loudness:
 *
 *   - `RemoteConnectionChip` — the steady `connected` state. A compact,
 *     non-overlapping chip that lives inside the title bar's right cluster
 *     (the slot the hidden native window controls free up on the web
 *     client). Quiet: a small sky dot + "Remote", host in the tooltip
 *     (and inline at wide widths). Truncates rather than colliding.
 *
 *   - `RemoteConnectionBanner` — the degraded `reconnecting` / `offline`
 *     states. A loud, centered banner overlay so a dropped/severed session
 *     stays impossible to miss (amber while backing off, red once revoked;
 *     the bootstrap reloads to the pairing screen shortly after offline).
 *
 * Desktop never runs the bootstrap, so the store stays `null` and both
 * return `null` — byte-identical to the pre-remote desktop chrome. This
 * replaces the old plain-DOM floating pill anchored bottom-left, which
 * overlapped the sidebar footer, the setup-scripts hint, and the bell.
 */
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useRemoteConnectionStore } from "@/remote/remote-connection-store";
import { isRemoteClient } from "./is-remote-client";

/**
 * Steady connected chip for the title bar's right cluster.
 *
 * `compact` matches the GUI-chrome (h-10) bar's 28px chip shape; the
 * default matches the legacy (h-9) bar's 24px chips.
 */
export function RemoteConnectionChip({ compact = false }: { compact?: boolean }) {
  const status = useRemoteConnectionStore((s) => s.status);
  const host = useRemoteConnectionStore((s) => s.host);

  if (!isRemoteClient() || status !== "connected") return null;

  const tooltip = host ? `Remote — connected to ${host}` : "Remote — connected";

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          data-testid="remote-connection-chip"
          role="status"
          aria-live="off"
          aria-label={tooltip}
          className={cn(
            "flex min-w-0 shrink items-center gap-1.5 border border-status-remote/30 bg-status-remote/10 font-medium text-status-remote",
            compact ? "h-7 rounded-[7px] px-2 text-[11px]" : "h-6 rounded-md px-2 text-[11px]",
          )}
        >
          <span
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-status-remote"
            aria-hidden
          />
          <span className="shrink-0">Remote</span>
          {host && (
            <span className="hidden max-w-[180px] truncate text-status-remote/70 lg:inline">
              · {host}
            </span>
          )}
        </div>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
        {tooltip}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Loud, centered banner for the degraded states. Mounted once near the app
 * root so it survives every view (main shell, settings, empty state) — it
 * is app-wide, not tied to the title bar. Sits just under the title bar so
 * it never covers the window-control edge, and never touches the
 * bottom-left zone the old floating pill collided with.
 */
export function RemoteConnectionBanner() {
  const status = useRemoteConnectionStore((s) => s.status);
  const host = useRemoteConnectionStore((s) => s.host);
  const offlineMessage = useRemoteConnectionStore((s) => s.offlineMessage);

  if (!isRemoteClient() || (status !== "reconnecting" && status !== "offline")) {
    return null;
  }

  const offline = status === "offline";
  const text = offline
    ? offlineMessage ?? "Remote access revoked"
    : host
      ? `Reconnecting to ${host}…`
      : "Reconnecting…";

  return (
    <div
      data-testid="remote-connection-banner"
      data-state={status}
      role="status"
      aria-live="polite"
      className={cn(
        "pointer-events-none fixed left-1/2 top-12 z-[2147483000] flex max-w-[calc(100vw-2rem)] -translate-x-1/2 items-center gap-2 rounded-full border px-3.5 py-1.5 text-xs font-semibold shadow-lg backdrop-blur",
        offline
          ? "border-status-attention/40 bg-status-attention/15 text-status-attention"
          : "border-status-working/45 bg-status-working/15 text-status-working",
      )}
    >
      <span
        className={cn(
          "cm-blink h-2 w-2 shrink-0 rounded-full",
          offline ? "bg-status-attention" : "bg-status-working",
        )}
        aria-hidden
      />
      <span className="truncate">{text}</span>
    </div>
  );
}
