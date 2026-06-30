import { useMemo } from "react";

import { Check, ChevronDown, Monitor, Server } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { useHosts } from "@/stores/hosts-store";

/**
 * Compact "where will this run" picker. Mirrors the shape of
 * superset-sh's DevicePicker pill (the only place in their UI that
 * solves the same UX problem we have): a ~140px button showing the
 * current selection, opening a dropdown with "Local Device" at the
 * top and a submenu of remote hosts below.
 *
 * The current selection model uses `host_id: number | null` where
 * `null` means "local." This matches the Rust workspace struct's
 * `host_id: Option<i64>` field exactly and removes the need for a
 * sentinel string for the local entry.
 *
 * Usage: drop into any surface where "which host" is the user's
 * choice. The new-workspace dialog and the chat new-session flow
 * both use this same component so the experience stays identical.
 */

export interface DevicePickerProps {
  /** Selected host id. `null` means "Local Device". */
  hostId: number | null;
  /** Fires whenever the user picks a new device. `null` means local. */
  onSelectHostId: (hostId: number | null) => void;
  /** Optional className passthrough so callers can adjust the trigger. */
  className?: string;
  /** Optional override label for the local entry. Defaults to
   *  "Local Device" matching superset's terminology. Some surfaces
   *  may want "This device" instead. */
  localLabel?: string;
  /** When true, the trigger renders compact-only (no label, icon
   *  only). Useful in tight headers. Off by default. */
  iconOnly?: boolean;
}

/**
 * Online-indicator dot. Local is "tautologically online" — the app
 * itself is the local host, so we don't draw a dot for it. Remote
 * hosts get either an emerald dot (reachable, last-test succeeded)
 * or a muted dot (not yet tested, or last test failed). The
 * reachability info lands when SSH transport ships in 2d; for now
 * every remote host shows as offline-style.
 */
function OnlineDot({ online }: { online: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        "inline-block size-1.5 shrink-0 rounded-full",
        online ? "bg-status-open" : "bg-muted-foreground/60",
      )}
    />
  );
}

export function DevicePicker({
  hostId,
  onSelectHostId,
  className,
  localLabel = "Local Device",
  iconOnly = false,
}: DevicePickerProps) {
  // Single shared cache across every DevicePicker + workspace
  // context menu instance. First read kicks off the lazy load;
  // subsequent reads (anywhere in the tree) hand back the cached
  // list. See `src/stores/hosts-store.ts`.
  const hosts = useHosts();

  const selectedHost = useMemo(
    () => hosts.find((h) => h.id === hostId) ?? null,
    [hosts, hostId],
  );
  const isLocal = hostId === null || !selectedHost;
  const label = isLocal ? localLabel : selectedHost.name;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={`Device: ${label}`}
          title={label}
          // Class string is intentionally identical to
          // `ProjectPicker`'s trigger so the row of pills looks
          // uniform. Don't reformat into separate string literals —
          // the previous attempt diverged enough that the pill
          // rendered taller than its neighbors. Match-by-string is
          // the most reliable diff guard.
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none",
            className,
          )}
        >
          {isLocal ? (
            <Monitor className="h-3.5 w-3.5" />
          ) : (
            <Server className="h-3.5 w-3.5" />
          )}
          {!iconOnly && (
            // Match the project picker's label shape exactly —
            // `max-w-[120px] truncate`, no flex-1. flex-1 was
            // letting the pill stretch wider than its content, so
            // the icon + label spacing read differently than the
            // neighboring project/branch pills.
            <span className="max-w-[120px] truncate">{label}</span>
          )}
          {!isLocal && (
            <OnlineDot
              online={Boolean(selectedHost && !selectedHost.dirty)}
            />
          )}
          {!iconOnly && (
            <ChevronDown className="h-2.5 w-2.5 opacity-40" />
          )}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuItem onSelect={() => onSelectHostId(null)}>
          <Monitor className="size-3.5" />
          <span className="flex-1">{localLabel}</span>
          {isLocal && <Check className="size-3.5" />}
        </DropdownMenuItem>
        {hosts.length > 0 && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <Server className="size-3.5" />
                <span>Other Hosts</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-64">
                {hosts.map((host) => {
                  const isSelected = hostId === host.id;
                  // Until the SSH probe lands (2d), we render every
                  // remote host as "offline-style" — they're
                  // configured but unverified. The dirty flag also
                  // means "hasn't reached the cloud yet," which is
                  // a useful signal of "this host is still being
                  // set up."
                  const isOnline = false;
                  return (
                    <DropdownMenuItem
                      key={host.id}
                      onSelect={() => onSelectHostId(host.id)}
                    >
                      <Server className="size-3.5" />
                      <span className="min-w-0 flex-1 truncate">
                        {host.name}
                      </span>
                      <OnlineDot online={isOnline} />
                      {isSelected && (
                        <Check className="ml-auto size-3.5 shrink-0" />
                      )}
                    </DropdownMenuItem>
                  );
                })}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
