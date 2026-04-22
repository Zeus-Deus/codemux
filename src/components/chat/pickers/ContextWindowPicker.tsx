import { useState } from "react";
import { Check, ChevronDown, Gauge } from "lucide-react";

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
import { getDefaultContextWindow } from "@/lib/agent-chat/model-resolution";
import type { ChatModelInfo } from "@/tauri/types";
import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

interface Props {
  model: ChatModelInfo | null;
  value: string | null;
  onChange: (value: string) => void;
  disabled?: boolean;
}

export function ContextWindowPicker({ model, value, onChange, disabled }: Props) {
  const [open, setOpen] = useState(false);

  if (!model) return null;

  const options = model.context_window_options;
  // Hide when the model doesn't offer a real choice.
  if (options.length <= 1) return null;

  const defaultValue = getDefaultContextWindow(model);
  const current =
    value && options.some((o) => o.value === value)
      ? value
      : defaultValue ?? options[0].value;
  const currentLabel = options.find((o) => o.value === current)?.label ?? current;

  // Per T3Code: only expose the pill when the user is on a non-default
  // option. On the default, the pill adds visual noise without
  // information. On non-default, the label becomes the signal.
  const showPill = current !== defaultValue;
  if (!showPill) return null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <Gauge className="h-3 w-3" />
          <span className="max-w-[140px] truncate">{currentLabel}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[260px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandList className="max-h-[300px]">
            <CommandEmpty>No context window options</CommandEmpty>
            <CommandGroup>
              {options.map((option) => {
                const isDefault = option.value === defaultValue;
                return (
                  <CommandItem
                    key={option.value}
                    value={option.value}
                    onSelect={() => {
                      onChange(option.value);
                      setOpen(false);
                    }}
                    className="h-9 gap-2 text-xs"
                  >
                    <span className="flex-1 min-w-0 truncate">
                      {option.label}
                      {isDefault ? (
                        <span className="ml-1.5 text-muted-foreground/60">
                          (default)
                        </span>
                      ) : null}
                    </span>
                    <Check
                      className={cn(
                        "h-3.5 w-3.5 text-muted-foreground",
                        current === option.value ? "opacity-100" : "opacity-0",
                      )}
                    />
                  </CommandItem>
                );
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
