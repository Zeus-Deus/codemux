import { ArrowUp, Plus, Square } from "lucide-react";

import { cn } from "@/lib/utils";
import type { ChatMode } from "@/stores/agent-chat-store";
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
  /** Active composer mode. The footer no longer renders a mode
   *  selector or pill (both moved out of the footer in Stage 3
   *  refactor — modes live in the `+` popup, pill renders above the
   *  textarea); the value is still needed here to disable the
   *  permission picker when a mode commandeers permissions. */
  mode: ChatMode;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  controlsDisabled: boolean;
  /** Step 8 Stage 3 — toggles the attach popup. Optional so existing
   *  call sites keep compiling; when omitted the `+` button is hidden. */
  onAttachClick?: () => void;
  /** Whether the attach popup is currently open. Drives the button's
   *  pressed visual state. */
  attachOpen?: boolean;
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
  mode,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onSubmit,
  onStop,
  controlsDisabled,
  onAttachClick,
  attachOpen = false,
}: Props) {
  const modeIsActive = mode !== "default";

  return (
    <div className="flex items-center gap-1.5 px-3 pb-2 pt-1">
      <div className="flex flex-wrap items-center gap-1.5 min-w-0">
        {onAttachClick && (
          <button
            type="button"
            onClick={onAttachClick}
            disabled={controlsDisabled}
            data-testid="composer-attach-button"
            data-open={attachOpen || undefined}
            className={cn(
              // Visually paired with the Send button: same circle
              // diameter, same icon weight. Subtle muted fill so
              // the bold filled Send still reads as the primary
              // action on the right.
              "inline-flex h-7 w-7 items-center justify-center rounded-full",
              "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
              "data-[open=true]:bg-foreground/10 data-[open=true]:text-foreground",
              "disabled:opacity-40 disabled:pointer-events-none",
            )}
            aria-label="Attach"
            aria-expanded={attachOpen}
            title="Attach (file, folder, mode, …)"
          >
            <Plus className="h-3.5 w-3.5" strokeWidth={2.5} />
          </button>
        )}

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
        {/* Permission picker stays visible when a mode pill is
            active — kept on-screen for discoverability — but goes
            disabled so users can't override the pill's setting and
            create a conflicting state. The pill still commandeers
            the live SDK permissionMode; this control re-enables the
            moment the pill is removed. */}
        <PermissionModePicker
          modes={permissionModes}
          value={permissionMode}
          onChange={onPermissionModeChange}
          disabled={controlsDisabled || modeIsActive}
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
