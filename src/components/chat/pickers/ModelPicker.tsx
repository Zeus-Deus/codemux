import { useEffect, useRef, useState } from "react";
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
import { focusCmdkOnOpen } from "./focus-cmdk-root";
import { FOOTER_TRIGGER } from "./footer-trigger";
import { refreshProviderCapabilitiesForIntent } from "@/stores/provider-capabilities-store";

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
): Array<{ id: string; label: string; description: string | null }> {
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
  /** Imperative open request — increments each time the composer's
   *  `/model` slash command fires. `0` / `undefined` means "no
   *  request yet"; any increment pops the picker open. */
  openSignal?: number;
}

export function ModelPicker({
  provider,
  value,
  onChange,
  disabled,
  openSignal,
}: Props) {
  const [open, setOpen] = useState(false);
  const list = modelsForProvider(provider);
  const current = value ?? list[0]?.id ?? "";

  // Consume the open signal exactly once per increment. Tracking the
  // last-seen value in a ref (instead of gating on `openSignal &&
  // !disabled`) prevents the picker from spontaneously reopening every
  // time `disabled` flips back to false — e.g. on session start/restart
  // — after `/model` has been used once in the pane's lifetime.
  const lastSignal = useRef(openSignal);
  useEffect(() => {
    if (openSignal !== lastSignal.current) {
      lastSignal.current = openSignal;
      if (!disabled) setOpen(true);
    }
  }, [openSignal, disabled]);

  useEffect(() => {
    if (!open) return;
    void refreshProviderCapabilitiesForIntent(provider);
  }, [open, provider]);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className={cn(FOOTER_TRIGGER, "gap-1.5")}
        >
          <ProviderLogo provider={provider} className="h-4 w-4" />
          <span className="max-w-[140px] truncate">
            {modelLabel(provider, current)}
          </span>
          <ChevronDown
            className="-mx-0.5 h-3.5 w-3.5 opacity-70"
            strokeWidth={2.25}
          />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[300px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkOnOpen}
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
                  className="min-h-9 gap-2 text-xs"
                >
                  <ProviderLogo
                    provider={provider}
                    className="h-3.5 w-3.5 shrink-0"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="truncate">{model.label}</div>
                    {model.description ? (
                      <div
                        className="mt-0.5 truncate text-[11px] text-muted-foreground/70"
                        title={model.description}
                      >
                        {model.description}
                      </div>
                    ) : null}
                  </div>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 shrink-0 text-muted-foreground",
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
