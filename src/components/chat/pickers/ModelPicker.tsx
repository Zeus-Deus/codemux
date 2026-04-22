import { useState } from "react";
import { Check, ChevronDown, Cpu } from "lucide-react";

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

// TODO: replace with a backend-driven list once the providers expose a
// `list_models_for_chat(provider)` equivalent to list_models_for_tool.
// Hard-coded here so Step 2 can render a working picker without a new
// Tauri command. Claude ids track `src-tauri/src/agent_provider/claude/
// protocol.rs` and `src-tauri/src/commands/openflow.rs`. Codex ids
// match T3Code's `packages/contracts/src/model.ts` (the `codex` CLI
// itself does not expose a `list-models` command and accepts any
// string — IDs here are the ones known to resolve server-side).
const CLAUDE_MODELS = [
  { id: "claude-opus-4-7", label: "Claude Opus 4.7" },
  { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
];

const CODEX_MODELS = [
  { id: "gpt-5.4", label: "GPT-5.4 (Codex)" },
  { id: "gpt-5.4-mini", label: "GPT-5.4 Mini" },
  { id: "gpt-5.3-codex", label: "GPT-5.3 Codex" },
];

export function defaultModelForProvider(
  provider: AgentChatProviderKind,
): string {
  return provider === "claude" ? CLAUDE_MODELS[0].id : CODEX_MODELS[0].id;
}

export function modelsForProvider(
  provider: AgentChatProviderKind,
): Array<{ id: string; label: string }> {
  return provider === "claude" ? CLAUDE_MODELS : CODEX_MODELS;
}

export function modelLabel(
  provider: AgentChatProviderKind,
  id: string | null,
): string {
  if (!id) return modelsForProvider(provider)[0]?.label ?? "Model";
  const list = modelsForProvider(provider);
  return list.find((m) => m.id === id)?.label ?? id;
}

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
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <Cpu className="h-3 w-3" />
          <span className="max-w-[140px] truncate">
            {modelLabel(provider, current)}
          </span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-[300px] p-0" align="start">
        <Command>
          <CommandInput placeholder="Search models..." className="h-8" />
          <CommandList className="max-h-[300px]">
            <CommandEmpty>No matching models</CommandEmpty>
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
