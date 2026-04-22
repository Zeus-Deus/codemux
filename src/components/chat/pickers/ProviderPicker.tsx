import { useState } from "react";
import { Check, ChevronDown, Layers } from "lucide-react";

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
import type { AgentChatProviderKind } from "@/tauri/types";

const PROVIDERS: Array<{ id: AgentChatProviderKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
];

export function providerLabel(id: AgentChatProviderKind): string {
  return PROVIDERS.find((p) => p.id === id)?.label ?? id;
}

interface Props {
  value: AgentChatProviderKind;
  onChange: (provider: AgentChatProviderKind) => void;
  disabled?: boolean;
}

export function ProviderPicker({ value, onChange, disabled }: Props) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <Layers className="h-3 w-3" />
          <span className="max-w-[100px] truncate">{providerLabel(value)}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-[220px] p-0" align="start">
        <Command>
          <CommandInput placeholder="Search providers..." className="h-8" />
          <CommandList className="max-h-[200px]">
            <CommandEmpty>No providers</CommandEmpty>
            <CommandGroup>
              {PROVIDERS.map((p) => (
                <CommandItem
                  key={p.id}
                  value={p.id}
                  onSelect={() => {
                    onChange(p.id);
                    setOpen(false);
                  }}
                  className="h-9 gap-2 text-xs"
                >
                  <span className="flex-1 min-w-0 truncate">{p.label}</span>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      value === p.id ? "opacity-100" : "opacity-0",
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
