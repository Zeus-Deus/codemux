import { useState } from "react";
import { Brain, Check, ChevronDown, Zap } from "lucide-react";

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
import { focusCmdkOnOpen } from "./focus-cmdk-root";
import { FOOTER_TRIGGER } from "./footer-trigger";

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
  ultra: "Deepest reasoning with automatic task delegation",
  ultracode:
    "Extra-thorough reasoning plus standing multi-agent workflow orchestration",
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
  /** Premium service tier for models that advertise fast-mode support. */
  fastMode: boolean;
  /** Fires when the user picks an effort row. `"ultrathink"` carries
   *  the special meaning of "prepend to the prompt"; caller handles that. */
  onEffortChange: (nextValue: string) => void;
  /** Fires when the user picks a context-window row. */
  onContextWindowChange: (nextValue: string) => void;
  /** Fires when the user picks Standard or Fast service tier. */
  onFastModeChange: (fastMode: boolean) => void;
  disabled?: boolean;
  /** Render a leading hairline pipe. Lives inside the picker (not the
   *  footer) so the pipe disappears together with the control when the
   *  capability gate hides it — no orphaned separators. */
  withSeparator?: boolean;
}

function effortLabel(labelMap: Record<string, string>, id: string): string {
  return labelMap[id] ?? id;
}

// The provider's own catalog text wins when it ships one (Codex reports
// a blurb per effort level over `model/list`), so a level added upstream
// reads correctly without a frontend bump. The built-in map covers
// providers that report nothing.
function effortDescription(model: ChatModelInfo, id: string): string {
  return (
    model.effort_descriptions?.[id] ?? DEFAULT_EFFORT_DESCRIPTIONS[id] ?? ""
  );
}

/**
 * Combined Reasoning + Context Window + Service Tier picker.
 *
 * Replaces the separate `EffortPicker` and `ContextWindowPicker` that
 * used to render as two adjacent pills. Merged surface: one pill
 * shows "Effort · Context", one dropdown carries every model-level runtime
 * choice. Service tier lives here instead of taking a permanent footer slot.
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
 *  - Hidden when the model has no effort levels, ≤1 context-window options,
 *    AND no service-tier choice (e.g. Haiku 4.5 — nothing to pick).
 *  - The effort section only appears when the model has effort levels.
 *  - The context-window section only appears when the model has >1
 *    options.
 *  - The service-tier section only appears when the model supports fast mode.
 */
export function ReasoningPicker({
  model,
  effortValue,
  contextWindowValue,
  labelMap,
  ultrathinkInBodyText,
  fastMode,
  onEffortChange,
  onContextWindowChange,
  onFastModeChange,
  disabled,
  withSeparator,
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
  const hasServiceTierSection = model.supports_fast_mode;

  // Haiku and other models without any configurable runtime choice: hide.
  if (!hasEffortSection && !hasContextSection && !hasServiceTierSection) {
    return null;
  }

  // Option C fallback — null slice values resolve to the model's
  // default. These are the values the pill reflects and the checkmarks
  // test against.
  const currentEffort =
    effortValue && effortLevels.includes(effortValue)
      ? effortValue
      : (model.default_effort ?? effortLevels[0] ?? null);

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
    ? (contextOptions.find((o) => o.value === currentContextWindow)?.label ??
      currentContextWindow)
    : null;

  // Pill-label composition:
  //  - Both sections populated: "Effort · Context"
  //  - Only effort: just the effort label.
  //  - Only context: just the context label (edge case; no Claude
  //    model has this shape today but the branch keeps the picker
  //    future-proof).
  const reasoningLabel = (() => {
    if (effortLabelText && contextLabelText) {
      return `${effortLabelText} · ${contextLabelText}`;
    }
    return effortLabelText ?? contextLabelText ?? null;
  })();
  const triggerLabel = reasoningLabel ?? (fastMode ? "Fast" : "Standard");
  const triggerAriaLabel = reasoningLabel
    ? `Reasoning: ${reasoningLabel}; service tier: ${fastMode ? "Fast" : "Standard"}`
    : `Service tier: ${fastMode ? "Fast" : "Standard"}`;

  return (
    <>
      {withSeparator && (
        <span
          aria-hidden
          className="mx-0.5 h-4 w-px shrink-0 self-center bg-border"
        />
      )}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={disabled}
            aria-label={triggerAriaLabel}
            title={triggerAriaLabel}
            className={FOOTER_TRIGGER}
          >
            {fastMode ? (
              <Zap
                aria-hidden
                data-testid="fast-mode-indicator"
                className="h-3.5 w-3.5 fill-current text-foreground/80"
                strokeWidth={1.5}
              />
            ) : reasoningLabel ? (
              <Brain className="h-4 w-4" />
            ) : null}
            <span className="max-w-[200px] truncate">{triggerLabel}</span>
            <ChevronDown className="h-3 w-3 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          className="w-[340px] p-0"
          align="start"
          onOpenAutoFocus={focusCmdkOnOpen}
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
                  const description = effortDescription(model, level);
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
                          currentEffort === level
                            ? "opacity-100"
                            : "opacity-0",
                        )}
                      />
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}

            {hasEffortSection &&
              (hasContextSection || hasServiceTierSection) && (
                <CommandSeparator />
              )}

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

            {hasContextSection && hasServiceTierSection && (
              <CommandSeparator />
            )}

            {hasServiceTierSection && (
              <CommandGroup heading="Service tier">
                <CommandItem
                  value="service-tier:standard"
                  onSelect={() => onFastModeChange(false)}
                  className="h-auto gap-2 py-2"
                >
                  <div className="flex min-w-0 flex-1 flex-col">
                    <span className="text-xs text-foreground">
                      Standard
                      <span className="ml-1.5 text-muted-foreground/60">
                        (default)
                      </span>
                    </span>
                    <span className="text-[11px] text-muted-foreground/80">
                      Normal speed and usage rate
                    </span>
                  </div>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      fastMode ? "opacity-0" : "opacity-100",
                    )}
                  />
                </CommandItem>
                <CommandItem
                  value="service-tier:fast"
                  onSelect={() => onFastModeChange(true)}
                  className="h-auto gap-2 py-2"
                >
                  <div className="flex min-w-0 flex-1 flex-col">
                    <span className="text-xs text-foreground">Fast</span>
                    <span className="text-[11px] text-muted-foreground/80">
                      Faster output at a premium usage rate
                    </span>
                  </div>
                  <Check
                    className={cn(
                      "h-3.5 w-3.5 text-muted-foreground",
                      fastMode ? "opacity-100" : "opacity-0",
                    )}
                  />
                </CommandItem>
              </CommandGroup>
            )}
          </CommandList>
        </Command>
        </PopoverContent>
      </Popover>
    </>
  );
}
