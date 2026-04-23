import { ArrowUp, Square } from "lucide-react";

import { cn } from "@/lib/utils";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ModelPicker } from "./pickers/ModelPicker";
import { PermissionModePicker } from "./pickers/PermissionModePicker";
import { ProviderPicker } from "./pickers/ProviderPicker";
import { ReasoningPicker } from "./pickers/ReasoningPicker";

interface Props {
  provider: AgentChatProviderKind;
  model: string | null;
  permissionMode: string | null;
  effort: string | null;
  contextWindow: string | null;
  activeModel: ChatModelInfo | null;
  effortLabelMap: Record<string, string>;
  permissionModes: PermissionModeOption[] | null;
  ultrathinkInBodyText: boolean;
  streaming: boolean;
  canSubmit: boolean;
  showProviderPicker: boolean;
  /** When false, hides the Stop button even while streaming. Used by
   *  the draft surface to avoid exposing a no-op Stop affordance
   *  mid-materialise. Defaults to true (existing behaviour). */
  showStopButton?: boolean;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  controlsDisabled: boolean;
}

export function ComposerFooter({
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  streaming,
  canSubmit,
  showProviderPicker,
  showStopButton = true,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onSubmit,
  onStop,
  controlsDisabled,
}: Props) {
  return (
    <div className="flex items-center gap-1.5 px-3 pb-2 pt-1">
      <div className="flex flex-wrap items-center gap-1.5 min-w-0">
        {showProviderPicker && (
          <ProviderPicker
            value={provider}
            onChange={onProviderChange}
            disabled={controlsDisabled}
          />
        )}
        <ModelPicker
          provider={provider}
          value={model}
          onChange={onModelChange}
          disabled={controlsDisabled}
        />
        <ReasoningPicker
          model={activeModel}
          effortValue={effort}
          contextWindowValue={contextWindow}
          labelMap={effortLabelMap}
          ultrathinkInBodyText={ultrathinkInBodyText}
          onEffortChange={onEffortChange}
          onContextWindowChange={onContextWindowChange}
          disabled={controlsDisabled}
        />
        <PermissionModePicker
          modes={permissionModes}
          value={permissionMode}
          onChange={onPermissionModeChange}
          disabled={controlsDisabled}
        />
      </div>
      <div className="ml-auto">
        {streaming && showStopButton ? (
          <button
            type="button"
            onClick={onStop}
            className={cn(
              "inline-flex h-7 w-7 items-center justify-center rounded-full",
              "bg-foreground text-background hover:bg-foreground/90",
            )}
            aria-label="Stop"
            title="Stop"
          >
            <Square className="h-3 w-3" fill="currentColor" />
          </button>
        ) : (
          <button
            type="button"
            onClick={onSubmit}
            disabled={!canSubmit || streaming}
            className={cn(
              "inline-flex h-7 w-7 items-center justify-center rounded-full",
              "bg-foreground text-background hover:bg-foreground/90",
              "disabled:opacity-40 disabled:cursor-not-allowed",
            )}
            aria-label="Send"
            title="Send"
          >
            <ArrowUp className="h-3.5 w-3.5" strokeWidth={2.5} />
          </button>
        )}
      </div>
    </div>
  );
}
