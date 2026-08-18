import { TriangleAlert, X } from "lucide-react";

import {
  selectVisibleHealthReport,
  useProviderHealth,
  useProviderHealthProbe,
} from "@/stores/provider-health-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { cn } from "@/lib/utils";

const PROVIDER_LABEL: Record<AgentChatProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  opencode: "OpenCode",
};

/**
 * Probe-backed provider status banner for chat surfaces.
 *
 * Renders nothing while the active provider's local runtime is healthy
 * (or not yet probed); renders a dismissible red/amber banner when the
 * provider CANNOT run a session (CLI missing, broken install, not
 * authenticated) so the user learns before sending a message into a
 * spinner that can never answer. Mounting this component is also what
 * schedules the probe (TTL-cached in the provider-health store).
 *
 * Dismissal is per failure identity: closing the banner hides THIS
 * status+message; a different failure — or the same one after a
 * recovery — banners again.
 */
export function ProviderStatusNotice({
  provider,
}: {
  provider: AgentChatProviderKind;
}) {
  useProviderHealthProbe(provider);
  const report = useProviderHealth((s) =>
    selectVisibleHealthReport(s, provider),
  );
  const dismiss = useProviderHealth((s) => s.dismiss);
  if (!report) return null;
  const label = PROVIDER_LABEL[report.provider];
  const isError = report.status === "error";
  return (
    // Floating overlay pinned near the pane top (below the floating
    // titlebar band in GUI chrome) so surfacing/dismissing the banner
    // never shifts the transcript. Host surfaces provide `relative`.
    <div className="pointer-events-none absolute inset-x-0 top-11 z-20 flex justify-center px-4">
      <div
        role="alert"
        data-testid="provider-status-notice"
        className={cn(
          // Shrinks to its content (capped) rather than filling the
          // column — a compact floating chip, not a full-width slab.
          "pointer-events-auto relative isolate max-w-[440px] overflow-hidden rounded-lg border text-[12px] shadow-md",
          isError
            ? "border-destructive/30 text-destructive"
            : "border-warning/30 text-warning",
        )}
      >
        {/* Opaque base + status tint as separate layers: the banner
            floats over arbitrary transcript content (including bright
            images), so a bare `/10` tint alone is unreadable. */}
        <div
          aria-hidden
          className="absolute inset-0 -z-20 bg-background/90 backdrop-blur-md"
        />
        <div
          aria-hidden
          className={cn(
            "absolute inset-0 -z-10",
            isError ? "bg-destructive/10" : "bg-warning/10",
          )}
        />
        <div className="flex items-center gap-2 py-1.5 pl-3 pr-1.5">
          <TriangleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden />
          <div
            className="min-w-0 select-text line-clamp-2"
            title={report.message ?? undefined}
          >
            <span className="font-medium">{label}</span>
            <span className="mx-1.5 opacity-60">·</span>
            <span className="opacity-90">
              {report.message ?? `${label} is unavailable.`}
            </span>
          </div>
          <button
            type="button"
            aria-label={`Dismiss ${label} provider status`}
            onClick={() => dismiss(provider)}
            className="shrink-0 rounded p-0.5 opacity-60 transition-opacity hover:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
          >
            <X className="h-3.5 w-3.5" aria-hidden />
          </button>
        </div>
      </div>
    </div>
  );
}
