import { useMemo, useState } from "react";
import { Brain, Check, ChevronDown } from "lucide-react";

import {
  Command,
  CommandGroup,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { focusCmdkRootOnOpen } from "@/components/chat/pickers/focus-cmdk-root";
import { cn } from "@/lib/utils";
import type { ReasoningOption } from "@/lib/launch-models";

interface Props {
  /** Reasoning levels the selected model accepts. Empty → no effort section. */
  reasoningOptions: ReasoningOption[];
  /** Explicit reasoning override, or `null` to fall back to the model default. */
  selectedReasoning: string | null;
  /** The selected model's default effort (drives the resolved/checked value
   *  when nothing is explicitly picked). */
  defaultReasoning: string | null;
  onReasoningChange: (reasoning: string) => void;
  /** Context-window options for the selected model. Empty → no context section. */
  contextOptions: ReasoningOption[];
  selectedContext: string | null;
  defaultContext: string | null;
  onContextChange: (context: string) => void;
  disabled?: boolean;
  /** Extra classes for the trigger button — lets a settings-form host
   *  override the default rounded-full pill to match nearby fields. */
  triggerClassName?: string;
}

/**
 * Launch-time Reasoning + Context Window picker — the sibling pill to
 * `LaunchModelPicker`, mirroring the chat composer's `ReasoningPicker`
 * (`src/components/chat/pickers/ReasoningPicker.tsx`).
 *
 * Reasoning and context are attributes *of a chosen model*, so they live
 * in their own pill that appears next to the model pill once a concrete
 * model is selected. The dialog feeds already-gated option lists (empty
 * for OpenCode/Gemini at launch, and on "Default" where there is no
 * concrete model), so this component simply renders what it's given.
 *
 * Hidden entirely when the model has neither effort levels nor context
 * options (e.g. Haiku) — same "nothing to configure" behaviour as the
 * chat `ReasoningPicker`.
 *
 * Null-selection fallback mirrors the chat picker: a `null` override
 * resolves to the model default for the pill label and the checkmark, so
 * the pill reads e.g. "High · 1M" the moment a model is picked, without
 * forcing an explicit choice.
 */
export function LaunchReasoningPicker({
  reasoningOptions,
  selectedReasoning,
  defaultReasoning,
  onReasoningChange,
  contextOptions,
  selectedContext,
  defaultContext,
  onContextChange,
  disabled,
  triggerClassName,
}: Props) {
  const [open, setOpen] = useState(false);

  const hasEffort = reasoningOptions.length > 0;
  const hasContext = contextOptions.length > 0;

  // Resolve the current value: an explicit, still-supported override wins;
  // otherwise fall back to the model default.
  const currentReasoning =
    (selectedReasoning &&
      reasoningOptions.some((o) => o.value === selectedReasoning) &&
      selectedReasoning) ||
    defaultReasoning ||
    null;
  const currentContext =
    (selectedContext &&
      contextOptions.some((o) => o.value === selectedContext) &&
      selectedContext) ||
    defaultContext ||
    null;

  const pillLabel = useMemo(() => {
    const parts: string[] = [];
    const r = reasoningOptions.find((o) => o.value === currentReasoning);
    if (r) parts.push(r.label);
    const c = contextOptions.find((o) => o.value === currentContext);
    if (c) parts.push(c.label);
    return parts.join(" · ");
  }, [reasoningOptions, contextOptions, currentReasoning, currentContext]);

  // Nothing to configure for this model — hide the pill (e.g. Haiku, or
  // OpenCode/Gemini whose reasoning can't be set at launch, or "Default").
  if (!hasEffort && !hasContext) return null;

  const renderRow = (
    opt: ReasoningOption,
    current: string | null,
    onPick: (value: string) => void,
  ) => (
    <CommandItem
      key={opt.value}
      value={opt.value}
      onSelect={() => onPick(opt.value)}
      className="h-9 gap-2 py-2"
    >
      <span className="flex-1 min-w-0 truncate text-xs text-foreground">
        {opt.label}
      </span>
      <Check
        className={cn(
          "h-3.5 w-3.5 text-muted-foreground",
          current === opt.value ? "opacity-100" : "opacity-0",
        )}
      />
    </CommandItem>
  );

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Select reasoning and context"
          disabled={disabled}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors outline-none hover:bg-muted hover:text-foreground disabled:opacity-50",
            triggerClassName,
          )}
        >
          <Brain className="h-3 w-3" />
          {pillLabel ? (
            <span className="max-w-[160px] truncate">{pillLabel}</span>
          ) : null}
          <ChevronDown className="h-2.5 w-2.5 opacity-40 ml-auto" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[260px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command shouldFilter={false}>
          {/* This popover is portaled outside the enclosing modal Dialog, so
              its wheel events land outside that dialog's scroll-lock
              (`react-remove-scroll`) and get swallowed — freezing the list.
              Stopping propagation keeps them from reaching the lock. Same
              fix `launch-model-picker.tsx` uses for its sibling list. */}
          <CommandList
            className="max-h-[340px]"
            onWheel={(e) => e.stopPropagation()}
          >
            {hasEffort ? (
              <CommandGroup heading="Reasoning">
                {reasoningOptions.map((opt) =>
                  renderRow(opt, currentReasoning, onReasoningChange),
                )}
              </CommandGroup>
            ) : null}

            {hasEffort && hasContext ? <CommandSeparator /> : null}

            {hasContext ? (
              <CommandGroup heading="Context Window">
                {contextOptions.map((opt) =>
                  renderRow(opt, currentContext, onContextChange),
                )}
              </CommandGroup>
            ) : null}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
