import { useEffect, useRef, useState } from "react";
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
import { focusCmdkOnOpen } from "./focus-cmdk-root";
import {
  FOOTER_SEPARATOR,
  FOOTER_TRIGGER,
  PICKER_COLLISION_PADDING,
  PICKER_COMMAND,
  PICKER_CONTENT,
  PICKER_GROUP,
  PICKER_LIST,
  PICKER_ROW,
  PICKER_ROW_CHECK,
  PICKER_ROW_DESCRIPTION,
  PICKER_ROW_TITLE,
} from "./footer-trigger";

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
  openSignal?: number;
  /** Render a leading hairline pipe. Lives inside the picker (not the
   *  footer) so the pipe disappears together with the control when the
   *  capability gate hides it — no orphaned separators. */
  withSeparator?: boolean;
  /** Narrow composer: keep the lock icon, drop the text label (kept as
   *  the tooltip and accessible name). */
  iconOnly?: boolean;
}

function modeLabel(modes: PermissionModeOption[], value: string): string {
  return modes.find((m) => m.value === value)?.label ?? value;
}

export function PermissionModePicker({
  modes,
  value,
  onChange,
  disabled,
  openSignal,
  withSeparator,
  iconOnly = false,
}: Props) {
  const [open, setOpen] = useState(false);
  const lastSignal = useRef(openSignal);
  useEffect(() => {
    if (openSignal !== lastSignal.current) {
      lastSignal.current = openSignal;
      if (!disabled && modes?.length) setOpen(true);
    }
  }, [openSignal, disabled, modes]);

  // Hide when capabilities aren't available or the provider has
  // declared no permission modes.
  if (!modes || modes.length === 0) return null;

  const defaultValue = modes.find((m) => m.is_default)?.value ?? modes[0].value;
  const current =
    value && modes.some((m) => m.value === value) ? value : defaultValue;
  const label = modeLabel(modes, current);

  return (
    <>
      {withSeparator && <span aria-hidden className={FOOTER_SEPARATOR} />}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={disabled}
            className={FOOTER_TRIGGER}
            aria-label={iconOnly ? `Access: ${label}` : undefined}
            title={iconOnly ? `Access: ${label}` : undefined}
          >
            <Lock className="size-3.5" />
            {!iconOnly && (
              <span className="max-w-[140px] truncate">{label}</span>
            )}
            <ChevronDown className="size-3 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          className={PICKER_CONTENT}
          align="start"
          collisionPadding={PICKER_COLLISION_PADDING}
          onOpenAutoFocus={focusCmdkOnOpen}
        >
          <Command className={PICKER_COMMAND} defaultValue={current}>
            <CommandList className={PICKER_LIST}>
              <CommandEmpty>No permission modes</CommandEmpty>
              <CommandGroup className={PICKER_GROUP}>
                {modes.map((mode) => (
                  <CommandItem
                    key={mode.value}
                    value={mode.value}
                    onSelect={() => {
                      onChange(mode.value);
                      setOpen(false);
                    }}
                    className={PICKER_ROW}
                    showCheckmark={false}
                  >
                    <div className="flex min-w-0 flex-1 flex-col gap-px">
                      <span className={cn(PICKER_ROW_TITLE, "truncate")}>
                        {mode.label}
                      </span>
                      {mode.description ? (
                        <span className={PICKER_ROW_DESCRIPTION}>
                          {mode.description}
                        </span>
                      ) : null}
                    </div>
                    <Check
                      className={cn(
                        PICKER_ROW_CHECK,
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
    </>
  );
}
