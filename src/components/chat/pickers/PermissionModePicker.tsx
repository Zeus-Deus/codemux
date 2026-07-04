import { useState } from "react";
import { Check, ChevronDown, Lock } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import type { PermissionModeOption } from "@/tauri/types";
import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

/**
 * Capability-driven permission-mode picker.
 *
 * - `modes` comes from the active provider's
 *   `ProviderChatCapabilities.permission_modes`. Hide the picker
 *   entirely when the array is empty (provider has no permission
 *   concept) or when capabilities haven't loaded yet (unavailable).
 * - `value` is the thread slice's `permissionMode`. Fallback to the
 *   provider's default when the value isn't in `modes`.
 * - `onChange` fires with the mode's machine `value` string. The
 *   caller decides whether to restart the session (per-session
 *   granularity) or let the next turn pick it up.
 */
interface Props {
  modes: PermissionModeOption[] | null;
  value: string | null;
  onChange: (mode: string) => void;
  disabled?: boolean;
}

function modeLabel(modes: PermissionModeOption[], value: string): string {
  return modes.find((m) => m.value === value)?.label ?? value;
}

export function PermissionModePicker({
  modes,
  value,
  onChange,
  disabled,
}: Props) {
  const [open, setOpen] = useState(false);

  // Hide when capabilities aren't available or the provider has
  // declared no permission modes.
  if (!modes || modes.length === 0) return null;

  const defaultValue = modes.find((m) => m.is_default)?.value ?? modes[0].value;
  const current =
    value && modes.some((m) => m.value === value) ? value : defaultValue;
  const label = modeLabel(modes, current);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-background px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground outline-none disabled:opacity-50"
        >
          <Lock className="h-3 w-3" />
          <span className="max-w-[140px] truncate">{label}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[340px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandList className="max-h-[300px]">
            <CommandEmpty>No permission modes</CommandEmpty>
            <CommandGroup>
              {modes.map((mode) => (
                <CommandItem
                  key={mode.value}
                  value={mode.value}
                  onSelect={() => {
                    onChange(mode.value);
                    setOpen(false);
                  }}
                  className="h-auto gap-2 py-2"
                >
                  <div className="flex flex-1 flex-col min-w-0">
                    <span className="text-xs text-foreground truncate">
                      {mode.label}
                    </span>
                    <span className="text-[11px] text-muted-foreground/80 truncate">
                      {mode.description}
                    </span>
                  </div>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      current === mode.value ? "opacity-100" : "opacity-0",
                    )}
                  />
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
