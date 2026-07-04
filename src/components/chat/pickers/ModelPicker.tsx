import { useState } from "react";
import { Check, ChevronDown } from "lucide-react";

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
import type { AgentChatProviderKind } from "@/tauri/types";
import { ProviderLogo } from "../provider-logo";
import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

// The model list + default-model / label helpers moved to
// `@/lib/agent-chat/capability-defaults` in the Stage C Effort-lock
// fix. This file used to carry a hand-maintained `CLAUDE_MODELS` /
// `CODEX_MODELS` table that diverged from Rust's
// `provider-capabilities-store` truth (3 vs 5 Claude models). Re-
// export the capability-driven helpers here so existing callers
// (AgentChatPane, chat-draft-store, etc.) don't need an import path
// change.
import {
  capabilityDefaults,
  defaultModelId,
  modelsForProvider as modelsForProviderFromCaps,
  modelLabel as modelLabelFromCaps,
} from "@/lib/agent-chat/capability-defaults";

export const defaultModelForProvider = defaultModelId;

export function modelsForProvider(
  provider: AgentChatProviderKind,
): Array<{ id: string; label: string }> {
  return modelsForProviderFromCaps(provider);
}

export function modelLabel(
  provider: AgentChatProviderKind,
  id: string | null,
): string {
  if (!id) {
    const fallback = modelsForProviderFromCaps(provider)[0]?.label;
    return fallback ?? "Model";
  }
  return modelLabelFromCaps(provider, id);
}

// Re-exported for call sites that want the full seed (model + effort
// + contextWindow + permissionMode). Kept here so imports can come
// from a single picker module.
export { capabilityDefaults };

interface Props {
  provider: AgentChatProviderKind;
  value: string | null;
  onChange: (model: string) => void;
  disabled?: boolean;
}

export function ModelPicker({ provider, value, onChange, disabled }: Props) {
  const [open, setOpen] = useState(false);
  const list = modelsForProvider(provider);
  const current = value ?? list[0]?.id ?? "";

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-background px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground outline-none disabled:opacity-50"
        >
          <ProviderLogo provider={provider} className="h-3 w-3" />
          <span className="max-w-[140px] truncate">
            {modelLabel(provider, current)}
          </span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[300px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandList className="max-h-[300px]">
            <CommandEmpty>No models available</CommandEmpty>
            <CommandGroup>
              {list.map((model) => (
                <CommandItem
                  key={model.id}
                  value={model.id}
                  onSelect={() => {
                    onChange(model.id);
                    setOpen(false);
                  }}
                  className="h-9 gap-2 text-xs"
                >
                  <ProviderLogo provider={provider} className="h-3.5 w-3.5" />
                  <span className="flex-1 min-w-0 truncate">{model.label}</span>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      current === model.id ? "opacity-100" : "opacity-0",
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
