import { useState } from "react";

import { Check, ChevronDown, Monitor, Plus, Server, Settings2 } from "lucide-react";

import { focusCmdkOnOpen } from "@/components/chat/pickers/focus-cmdk-root";
import {
  describeStatus,
  type DeviceTone,
} from "@/components/devices/use-device-cards";
import { Command, CommandItem, CommandList } from "@/components/ui/command";
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";
import { useHostStatuses } from "@/stores/host-status-store";
import { useHosts } from "@/stores/hosts-store";
import { useLocalDeviceName } from "@/stores/local-device-store";
import { useUIStore } from "@/stores/ui-store";

// Same geometry as the scope strip's other controls (ThreadScopeRow's
// GHOST_BTN and the location picker's rows), so the device control reads as
// one more item in that strip rather than a foreign pill.
const TRIGGER =
  "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-md px-2 text-label font-medium text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50";
// One quiet line per device, like the strip's other popovers: name, an
// inline check when selected, and its status on the right. The SSH address
// lives in the row's tooltip.
const ROW =
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
function SelectedMark() {
  return (
    <span className="flex shrink-0 text-accent-ember" aria-label="Selected">
      <Check className="size-3.5" />
    </span>
  );
}

export interface DevicePickerProps {
  /** Selected device (`HostView.id`); `null` means this device. */
  hostId: number | null;
  onSelectHostId: (hostId: number | null) => void;
  disabled?: boolean;
}

/**
 * "Where does this thread run?" — the first control in the new-thread scope
 * strip. Lists this device and every configured SSH device with its live
 * status. Always rendered, even with no devices configured, so adding one is
 * a click away from the composer.
 */
export function DevicePicker({
  hostId,
  onSelectHostId,
  disabled,
}: DevicePickerProps) {
  const [open, setOpen] = useState(false);
  const hosts = useHosts();
  const statuses = useHostStatuses();
  const localName = useLocalDeviceName();
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  const selected =
    hostId === null ? null : (hosts.find((h) => h.id === hostId) ?? null);
  // An id the list doesn't know (not loaded yet, failed to load, or deleted
  // elsewhere) still sends to that device, so it must not read as this one.
  const unavailable = hostId !== null && selected === null;
  const label = selected
    ? selected.name
    : unavailable
      ? "Device unavailable"
      : (localName ?? "This device");

  const select = (next: number | null) => {
    setOpen(false);
    onSelectHostId(next);
  };

  const openDeviceSettings = () => {
    setOpen(false);
    if (hosts.length === 0) useAddDeviceDialogStore.getState().setOpen(true);
    setShowSettings(true, "hosts");
  };

  // Read once per render; the status store's event keeps `statuses` fresh,
  // and a stale "last seen" age isn't shown in this compact list.
  const now = Date.now();

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label={`Device: ${label}`}
          title={
            selected
              ? `Runs on ${selected.name}`
              : unavailable
                ? "Runs on a device that isn't available"
                : "Runs on this device"
          }
          className={TRIGGER}
        >
          {selected ? (
            <Server className="size-3.5 text-status-remote" />
          ) : unavailable ? (
            <Server className="size-3.5 text-muted-foreground" />
          ) : (
            <Monitor className="size-3.5 text-muted-foreground" />
          )}
          <span className="max-w-[120px] truncate">{label}</span>
          <ChevronDown className="size-3 opacity-45" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="flex max-h-[var(--radix-popover-content-available-height)] w-[244px] flex-col p-1.5"
        align="start"
        side="top"
        collisionPadding={10}
        onOpenAutoFocus={focusCmdkOnOpen}
      >
        <Command loop className="bg-transparent">
          <Eyebrow className="px-2 pb-1 pt-1">Run on</Eyebrow>
          <CommandList
            className="max-h-[260px] min-h-0 flex-1 overflow-y-auto thin-scrollbar"
            onWheel={(e) => e.stopPropagation()}
          >
            <CommandItem
              value="this-device"
              onSelect={() => select(null)}
              showCheckmark={false}
              className={ROW}
              title="Runs on this computer"
            >
              <Monitor className="size-3.5 text-muted-foreground" />
              <span className="min-w-0 truncate">{localName ?? "This device"}</span>
              {hostId === null && <SelectedMark />}
              <span className="ml-auto shrink-0 text-caption text-muted-foreground/70">
                this device
              </span>
            </CommandItem>
            {hosts.length > 0 && (
              <div aria-hidden className="mx-2 my-1 h-px bg-border/60" />
            )}
            {hosts.map((host) => {
              const tone = describeStatus(statuses[host.id] ?? null, now).tone;
              const isSelected = selected?.id === host.id;
              return (
                <CommandItem
                  key={host.id}
                  value={`device-${host.id}`}
                  onSelect={() => select(host.id)}
                  showCheckmark={false}
                  className={ROW}
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
                  {isSelected && <SelectedMark />}
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
          </CommandList>
        </Command>
        <div aria-hidden className="mx-2 my-1 h-px bg-border/60" />
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
      </PopoverContent>
    </Popover>
  );
}
