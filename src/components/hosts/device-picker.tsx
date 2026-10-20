import { Check, Monitor, Plus, Server, Settings2 } from "lucide-react";

import {
  describeStatus,
  type DeviceTone,
} from "@/components/devices/use-device-cards";
import { CommandItem } from "@/components/ui/command";
import { cn } from "@/lib/utils";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";
import { useHostStatuses } from "@/stores/host-status-store";
import { useHosts } from "@/stores/hosts-store";
import { useLocalDeviceName } from "@/stores/local-device-store";
import { useUIStore } from "@/stores/ui-store";
import type { HostView } from "@/tauri/commands";

// One quiet line per device, like the scope strip's other menus: name, an
// inline check when selected, and its status on the right. The SSH address
// lives in the row's tooltip.
export const DEVICE_ROW =
  "flex h-8 w-full items-center gap-2.5 rounded-lg px-2 text-left text-label font-medium text-foreground data-selected:bg-surface-2";

const TONE_LABEL: Record<DeviceTone, string> = {
  online: "Online",
  updating: "Updating…",
  attention: "Needs setup",
  offline: "Offline",
  checking: "Checking…",
};

const TONE_DOT: Record<DeviceTone, string> = {
  online: "bg-status-open",
  updating: "bg-status-remote motion-safe:animate-pulse",
  attention: "bg-status-working",
  offline: "bg-muted-foreground/60",
  checking: "bg-muted-foreground/40",
};

/** The accent check. Wrapped so the highlighted row's icon recolouring
 *  (which targets the item's direct `svg` children) leaves it alone. */
export function SelectedMark() {
  return (
    <span className="flex shrink-0 text-accent-ember" aria-label="Selected">
      <Check className="size-3.5" />
    </span>
  );
}

export interface DeviceTarget {
  /** The selected device, or `null` for this device. */
  selected: HostView | null;
  /** An id the list doesn't know (not loaded yet, failed to load, or deleted
   *  elsewhere). The thread still runs on that device, so it must not read
   *  as this one. */
  unavailable: boolean;
  label: string;
}

/** Resolves a thread's `hostId` (`null` = this device) to what the strip
 *  shows for it. */
export function useDeviceTarget(hostId: number | null): DeviceTarget {
  const hosts = useHosts();
  const selected =
    hostId === null ? null : (hosts.find((h) => h.id === hostId) ?? null);
  const unavailable = hostId !== null && selected === null;
  const label = selected
    ? selected.name
    : unavailable
      ? "Device unavailable"
      : "This device";
  return { selected, unavailable, label };
}

/**
 * "Where does this thread run?" rows, rendered inside a cmdk `Command`:
 * this device first, then every configured SSH device with its live status.
 */
export function DeviceOptions({
  hostId,
  onSelectHostId,
}: {
  hostId: number | null;
  onSelectHostId: (hostId: number | null) => void;
}) {
  const hosts = useHosts();
  const statuses = useHostStatuses();
  const localName = useLocalDeviceName();
  // Read once per render; the status store's event keeps `statuses` fresh,
  // and a stale "last seen" age isn't shown in this compact list.
  const now = Date.now();

  return (
    <>
      <CommandItem
        value="this-device"
        onSelect={() => onSelectHostId(null)}
        showCheckmark={false}
        className={DEVICE_ROW}
        title="Runs on this computer"
      >
        <Monitor className="size-3.5 text-muted-foreground" />
        <span className="min-w-0 truncate">{localName ?? "This device"}</span>
        {hostId === null && <SelectedMark />}
        <span className="ml-auto shrink-0 text-caption text-muted-foreground/70">
          this device
        </span>
      </CommandItem>
      {hosts.map((host) => {
        const tone = describeStatus(statuses[host.id] ?? null, now).tone;
        return (
          <CommandItem
            key={host.id}
            value={`device-${host.id}`}
            onSelect={() => onSelectHostId(host.id)}
            showCheckmark={false}
            className={DEVICE_ROW}
            title={`SSH ${host.ssh_target} · ${TONE_LABEL[tone]}`}
            data-host-id={host.id}
          >
            <Server
              className={cn(
                "size-3.5",
                tone === "online" ? "text-muted-foreground" : "text-muted-foreground/50",
              )}
            />
            <span
              className={cn(
                "min-w-0 truncate",
                tone === "online" ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {host.name}
            </span>
            {hostId === host.id && <SelectedMark />}
            <span className="ml-auto flex shrink-0 items-center gap-1.5 text-caption text-muted-foreground/70">
              <span
                aria-hidden
                className={cn("size-1.5 rounded-full", TONE_DOT[tone])}
              />
              {TONE_LABEL[tone].toLowerCase()}
            </span>
          </CommandItem>
        );
      })}
    </>
  );
}

/** "Add a device…" (none configured yet) or "Manage devices…", so adding
 *  one stays a click away from the composer. */
export function DeviceMenuFooter({ onDone }: { onDone: () => void }) {
  const hosts = useHosts();
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const openDeviceSettings = () => {
    onDone();
    if (hosts.length === 0) useAddDeviceDialogStore.getState().setOpen(true);
    setShowSettings(true, "hosts");
  };
  return (
    <button
      type="button"
      onClick={openDeviceSettings}
      className="flex h-8 w-full items-center gap-2.5 rounded-lg px-2 text-left text-label text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
    >
      {hosts.length === 0 ? (
        <Plus className="size-3.5" />
      ) : (
        <Settings2 className="size-3.5" />
      )}
      {hosts.length === 0 ? "Add a device…" : "Manage devices…"}
    </button>
  );
}
