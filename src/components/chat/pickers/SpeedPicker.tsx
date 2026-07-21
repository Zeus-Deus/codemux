import { useState } from "react";
import { Check, ChevronDown, Zap } from "lucide-react";

import {
  Command,
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
import type { ChatModelInfo } from "@/tauri/types";

import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

interface Props {
  model: ChatModelInfo | null;
  value: boolean;
  onChange: (fastMode: boolean) => void;
  disabled?: boolean;
}

const OPTIONS = [
  {
    value: false,
    label: "Standard",
    description: "Normal speed and usage rate",
  },
  {
    value: true,
    label: "Fast",
    description: "Faster output at a premium usage rate",
  },
] as const;

/** Capability-gated provider speed picker.
 *
 * Fast mode is a billing-affecting choice, so it uses an explicit two-row
 * menu instead of a one-click toggle. The control only exists for models whose
 * live capability payload advertises support.
 */
export function SpeedPicker({ model, value, onChange, disabled }: Props) {
  const [open, setOpen] = useState(false);

  if (!model?.supports_fast_mode) return null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          data-testid="speed-picker-trigger"
          className={cn(
            "inline-flex items-center gap-1.5 rounded-lg border px-2.5 py-1 text-xs font-medium transition-colors outline-none disabled:opacity-50",
            value
              ? "border-accent-ember/45 bg-accent-ember/15 text-accent-ember hover:bg-accent-ember/20"
              : "border-border bg-background text-muted-foreground hover:bg-muted/50 hover:text-foreground",
          )}
          aria-label={`Speed: ${value ? "Fast" : "Standard"}`}
        >
          <Zap className="h-3 w-3" fill={value ? "currentColor" : "none"} />
          <span>{value ? "Fast" : "Standard"}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[300px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandList>
            <CommandGroup heading="Speed">
              {OPTIONS.map((option) => (
                <CommandItem
                  key={option.label}
                  value={`speed:${option.label}`}
                  onSelect={() => {
                    onChange(option.value);
                    setOpen(false);
                  }}
                  className="h-auto gap-2 py-2"
                >
                  <div className="flex min-w-0 flex-1 flex-col">
                    <span className="text-xs text-foreground">
                      {option.label}
                    </span>
                    <span className="text-[11px] text-muted-foreground/80">
                      {option.description}
                    </span>
                  </div>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      value === option.value ? "opacity-100" : "opacity-0",
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
