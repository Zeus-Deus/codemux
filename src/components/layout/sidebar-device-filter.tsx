import { Check, ChevronsUpDown, Cloud, Layers, Monitor } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { useHosts } from "@/stores/hosts-store";

/**
 * Workspace list filter — "This device / All devices / per-host."
 * Matches the shape of superset-sh's `V2WorkspacesHeader` filter
 * dropdown so users coming from there see a familiar control.
 *
 * `null` means "All devices" (no filtering — the default).
 * `"local"` means "This device only" (host_id === null/undefined).
 * Any other string is a host id (matching `workspace.host_id`).
 *
 * The dropdown is only rendered when at least one remote host
 * exists. With zero hosts there's nothing to filter, and a
 * permanent dropdown showing only "This device" would just be
 * visual noise for the 99% case.
 */

export type DeviceFilterValue = "all" | "local" | number;

interface Props {
  value: DeviceFilterValue;
  onChange: (next: DeviceFilterValue) => void;
}

export function SidebarDeviceFilter({ value, onChange }: Props) {
  const hosts = useHosts();
  if (hosts.length === 0) {
    return null;
  }

  const selectedHost =
    typeof value === "number"
      ? hosts.find((h) => h.id === value) ?? null
      : null;
  const label =
    value === "all"
      ? "All devices"
      : value === "local"
        ? "This device"
        : selectedHost?.name ?? `Host ${value}`;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={`Filter: ${label}`}
          title={label}
          className={cn(
            "inline-flex h-6 max-w-full items-center gap-1.5",
            "rounded-md border border-border bg-background px-2",
            "text-[11px] text-foreground/80",
            "hover:bg-muted/50 focus-visible:outline-none",
            "focus-visible:ring-1 focus-visible:ring-ring",
          )}
        >
          {value === "all" ? (
            <Layers className="size-3 shrink-0" />
          ) : value === "local" ? (
            <Monitor className="size-3 shrink-0" />
          ) : (
            <Cloud className="size-3 shrink-0" />
          )}
          <span className="min-w-0 flex-1 truncate text-left">{label}</span>
          <ChevronsUpDown className="size-2.5 shrink-0 opacity-60" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <DropdownMenuItem onSelect={() => onChange("all")}>
          <Layers className="size-3.5" />
          <span className="flex-1">All devices</span>
          {value === "all" && <Check className="size-3.5" />}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onChange("local")}>
          <Monitor className="size-3.5" />
          <span className="flex-1">This device</span>
          {value === "local" && <Check className="size-3.5" />}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {hosts.map((host) => (
          <DropdownMenuItem
            key={host.id}
            onSelect={() => onChange(host.id)}
          >
            <Cloud className="size-3.5" />
            <span className="min-w-0 flex-1 truncate">{host.name}</span>
            {value === host.id && (
              <Check className="ml-auto size-3.5 shrink-0" />
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * Pure helper that filters a workspace list by the device filter
 * value. Extracted so unit tests can pin down the semantics without
 * spinning up React.
 */
export function applyDeviceFilter<
  W extends { host_id?: number | null | undefined },
>(workspaces: W[], filter: DeviceFilterValue): W[] {
  if (filter === "all") return workspaces;
  if (filter === "local") {
    return workspaces.filter(
      (w) => w.host_id === null || w.host_id === undefined,
    );
  }
  return workspaces.filter((w) => w.host_id === filter);
}
