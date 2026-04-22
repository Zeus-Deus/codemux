import { useState } from "react";
import { Check, ChevronDown, ShieldCheck } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";

// Mirrors the Claude CLI's permission-mode strings so values round-trip
// cleanly through agent_chat_set_permission_mode. Display labels match
// the reference screenshots (Supervised / Auto-accept edits / Full
// access); the Codex provider currently rejects unknown modes with a
// validation error — the ValidationError surfaces as a toast upstream.
export const PERMISSION_MODES: Array<{
  id: string;
  label: string;
  description: string;
}> = [
  {
    id: "default",
    label: "Supervised",
    description: "Ask before commands and file changes",
  },
  {
    id: "acceptEdits",
    label: "Auto-accept edits",
    description: "Allow file edits without approval",
  },
  {
    id: "bypassPermissions",
    label: "Full access",
    description: "Run everything without approval",
  },
];

export function permissionModeLabel(id: string | null): string {
  if (!id) return "Supervised";
  return PERMISSION_MODES.find((m) => m.id === id)?.label ?? id;
}

interface Props {
  value: string | null;
  onChange: (mode: string) => void;
  disabled?: boolean;
}

export function PermissionModePicker({ value, onChange, disabled }: Props) {
  const [open, setOpen] = useState(false);
  const current = value ?? "default";
  const label = permissionModeLabel(current);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <ShieldCheck className="h-3 w-3" />
          <span className="max-w-[140px] truncate">{label}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-[340px] p-0" align="start">
        <Command>
          <CommandInput placeholder="Search modes..." className="h-8" />
          <CommandList className="max-h-[300px]">
            <CommandEmpty>No matching modes</CommandEmpty>
            <CommandGroup>
              {PERMISSION_MODES.map((mode) => (
                <CommandItem
                  key={mode.id}
                  value={mode.id}
                  onSelect={() => {
                    onChange(mode.id);
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
                      current === mode.id ? "opacity-100" : "opacity-0",
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
