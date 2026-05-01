import { useState } from "react";
import { Brain, Check, ChevronDown } from "lucide-react";

import {
  Command,
  CommandEmpty,
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
import { cn } from "@/lib/utils";
import type { ChatModelInfo } from "@/tauri/types";
import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

// Short description lines for each effort level. Verbs match
// PermissionModePicker's density (two-line rows).
const DEFAULT_EFFORT_DESCRIPTIONS: Record<string, string> = {
  none: "No extra reasoning",
  minimal: "Fastest, minimal reasoning",
  low: "Light reasoning",
  medium: "Balanced default",
  high: "Thorough reasoning",
  xhigh: "Extra-thorough reasoning",
  max: "Deepest reasoning",
  ultrathink:
    'Prepends "Ultrathink:" to your prompt for extra-thorough reasoning',
};

interface Props {
  /** Active chat model, or null when capabilities haven't loaded. */
  model: ChatModelInfo | null;
  /** Current selected effort from the thread slice, or null → default. */
  effortValue: string | null;
  /** Current selected context window from the thread slice, or null → default. */
  contextWindowValue: string | null;
  /** Canonical effort-label map from the provider capabilities. */
  labelMap: Record<string, string>;
  /** True when the composer's draft text contains "ultrathink" outside
   *  the canonical prefix — disables the effort section. */
  ultrathinkInBodyText: boolean;
  /** Fires when the user picks an effort row. `"ultrathink"` carries
   *  the special meaning of "prepend to the prompt"; caller handles that. */
  onEffortChange: (nextValue: string) => void;
  /** Fires when the user picks a context-window row. */
  onContextWindowChange: (nextValue: string) => void;
  disabled?: boolean;
}

function effortLabel(labelMap: Record<string, string>, id: string): string {
  return labelMap[id] ?? id;
}

function effortDescription(id: string): string {
  return DEFAULT_EFFORT_DESCRIPTIONS[id] ?? "";
}

/**
 * Combined Effort + Context Window picker.
 *
 * Replaces the separate `EffortPicker` and `ContextWindowPicker` that
 * used to render as two adjacent pills. Merged surface: one pill
 * shows "Effort · Context", one dropdown has both sections.
 *
 * Null-slice fallback (Option C of the Stage C follow-up): when the
 * slice's effort or contextWindow is null, the picker resolves to the
 * model's default and renders THAT as the "current" value (label +
 * check). The user sees a consistent pill / dropdown state without
 * needing an explicit pick. The moment the user DOES pick something,
 * the slice value wins over the fallback.
 *
 * Render rules:
 *  - Hidden when `model` is null (capabilities unavailable).
 *  - Hidden when the model has no effort levels AND ≤1 context-window
 *    options (e.g. Haiku 4.5 — nothing to pick).
 *  - The effort section only appears when the model has effort levels.
 *  - The context-window section only appears when the model has >1
 *    options.
 */
export function ReasoningPicker({
  model,
  effortValue,
  contextWindowValue,
  labelMap,
  ultrathinkInBodyText,
  onEffortChange,
  onContextWindowChange,
  disabled,
}: Props) {
  const [open, setOpen] = useState(false);

  if (!model) return null;

  const effortLevels = [
    ...model.effort_levels,
    ...model.prompt_injected_effort_levels,
  ];
  const contextOptions = model.context_window_options;
  const hasEffortSection = effortLevels.length > 0;
  const hasContextSection = contextOptions.length > 1;

  // Haiku and other models without either choice: hide entirely.
  if (!hasEffortSection && !hasContextSection) return null;

  // Option C fallback — null slice values resolve to the model's
  // default. These are the values the pill reflects and the checkmarks
  // test against.
  const currentEffort =
    effortValue && effortLevels.includes(effortValue)
      ? effortValue
      : model.default_effort ?? effortLevels[0] ?? null;

  const defaultContextWindow =
    contextOptions.find((o) => o.is_default)?.value ??
    contextOptions[0]?.value ??
    null;
  const currentContextWindow =
    contextWindowValue &&
    contextOptions.some((o) => o.value === contextWindowValue)
      ? contextWindowValue
      : defaultContextWindow;

  const effortLabelText = currentEffort
    ? effortLabel(labelMap, currentEffort)
    : null;
  const contextLabelText = currentContextWindow
    ? contextOptions.find((o) => o.value === currentContextWindow)?.label ??
      currentContextWindow
    : null;

  // Pill-label composition:
  //  - Both sections populated: "Effort · Context"
  //  - Only effort: just the effort label.
  //  - Only context: just the context label (edge case; no Claude
  //    model has this shape today but the branch keeps the picker
  //    future-proof).
  const pillLabel = (() => {
    if (effortLabelText && contextLabelText) {
      return `${effortLabelText} · ${contextLabelText}`;
    }
    return effortLabelText ?? contextLabelText ?? "";
  })();

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <Brain className="h-3 w-3" />
          <span className="max-w-[200px] truncate">{pillLabel}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[340px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          {hasEffortSection && ultrathinkInBodyText ? (
            <div className="px-3 pt-2 pb-1 text-[11px] text-muted-foreground/80">
              Your prompt contains &quot;ultrathink&quot; in the text. Remove
              it to change effort.
            </div>
          ) : null}
          <CommandList className="max-h-[420px]">
            <CommandEmpty>No reasoning options</CommandEmpty>

            {hasEffortSection && (
              <CommandGroup heading="Effort">
                {effortLevels.map((level) => {
                  const isDefault = level === model.default_effort;
                  const title = effortLabel(labelMap, level);
                  const description = effortDescription(level);
                  return (
                    <CommandItem
                      key={`effort-${level}`}
                      value={`effort:${level}`}
                      disabled={ultrathinkInBodyText}
                      onSelect={() => {
                        if (ultrathinkInBodyText) return;
                        onEffortChange(level);
                      }}
                      className="h-auto gap-2 py-2"
                    >
                      <div className="flex flex-1 flex-col min-w-0">
                        <span className="text-xs text-foreground truncate">
                          {title}
                          {isDefault ? (
                            <span className="ml-1.5 text-muted-foreground/60">
                              (default)
                            </span>
                          ) : null}
                        </span>
                        {description ? (
                          <span className="text-[11px] text-muted-foreground/80 truncate">
                            {description}
                          </span>
                        ) : null}
                      </div>
                      <Check
                        className={cn(
                          "h-3.5 w-3.5 text-muted-foreground",
                          currentEffort === level ? "opacity-100" : "opacity-0",
                        )}
                      />
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}

            {hasEffortSection && hasContextSection && <CommandSeparator />}

            {hasContextSection && (
              <CommandGroup heading="Context Window">
                {contextOptions.map((option) => {
                  const isDefault = option.is_default;
                  return (
                    <CommandItem
                      key={`ctx-${option.value}`}
                      value={`ctx:${option.value}`}
                      onSelect={() => onContextWindowChange(option.value)}
                      className="h-9 gap-2 py-2"
                    >
                      <span className="flex-1 min-w-0 truncate text-xs text-foreground">
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
                          currentContextWindow === option.value
                            ? "opacity-100"
                            : "opacity-0",
                        )}
                      />
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
