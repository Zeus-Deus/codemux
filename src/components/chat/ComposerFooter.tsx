import { ArrowUp, Plus, Square } from "lucide-react";

import { cn } from "@/lib/utils";
import type { ChatMode } from "@/stores/agent-chat-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ModelPicker } from "./pickers/ModelPicker";
import { MultiProviderModelPicker } from "./pickers/MultiProviderModelPicker";
import { PermissionModePicker } from "./pickers/PermissionModePicker";
import { ReasoningPicker } from "./pickers/ReasoningPicker";
import { SpeedPicker } from "./pickers/SpeedPicker";

interface Props {
  provider: AgentChatProviderKind;
  model: string | null;
  permissionMode: string | null;
  effort: string | null;
  contextWindow: string | null;
  fastMode?: boolean;
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
  onProviderModelChange: (
    provider: AgentChatProviderKind,
    model: string,
  ) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onFastModeChange?: (fastMode: boolean) => void;
  onSubmit: () => void;
  onStop: () => void;
  controlsDisabled: boolean;
  /** Step 8 Stage 3 — toggles the attach popup. Optional so existing
   *  call sites keep compiling; when omitted the `+` button is hidden. */
  onAttachClick?: () => void;
  /** Whether the attach popup is currently open. Drives the button's
   *  pressed visual state. */
  attachOpen?: boolean;
  /** Imperative model-picker open request from the composer's
   *  `/model` slash command. Forwarded to whichever picker variant
   *  renders. Optional; omitted by call sites that predate `/model`. */
  modelPickerOpenSignal?: number;
}

export function ComposerFooter({
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  fastMode = false,
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  streaming,
  canSubmit,
  showProviderPicker,
  showStopButton = true,
  mode,
  onProviderModelChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onFastModeChange = () => {},
  onSubmit,
  onStop,
  controlsDisabled,
  onAttachClick,
  attachOpen = false,
  modelPickerOpenSignal,
}: Props) {
  const modeIsActive = mode !== "default";

  return (
    <div className="flex items-center gap-1.5 px-2 pb-2 pt-1">
      <div className="flex flex-wrap items-center gap-1 min-w-0">
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
              "inline-flex h-8 w-8 items-center justify-center rounded-full",
              // Transparent base border keeps the circle diameter fixed
              // (border-box) so the open state can add an ember border
              // without a 1px layout shift.
              "border border-transparent",
              "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
              // Open (command menu showing): the ember active treatment
              // — ember-tinted fill, ember border, ember icon — matches
              // the redesign so the trigger reads as "armed".
              "data-[open=true]:border-accent-ember/45 data-[open=true]:bg-accent-ember/15 data-[open=true]:text-accent-ember data-[open=true]:hover:bg-accent-ember/15 data-[open=true]:hover:text-accent-ember",
              "disabled:opacity-40 disabled:pointer-events-none",
            )}
            aria-label="Attach"
            aria-expanded={attachOpen}
            title="Attach (file, folder, mode, …)"
          >
            <Plus className="h-4 w-4" strokeWidth={2.25} />
          </button>
        )}

        {/* Step 12 Stage 4 — when the unified provider+model picker is
            enabled (chat panes with `ENABLE_PROVIDER_PICKER`), render
            the new `MultiProviderModelPicker` and skip the legacy
            single-provider `ModelPicker`. `ModelPicker` remains as the
            fallback for callers that deliberately disable the unified
            provider surface. */}
        {showProviderPicker ? (
          <MultiProviderModelPicker
            provider={provider}
            model={model}
            onProviderModelChange={onProviderModelChange}
            disabled={controlsDisabled}
            openSignal={modelPickerOpenSignal}
          />
        ) : (
          <ModelPicker
            provider={provider}
            value={model}
            onChange={onModelChange}
            disabled={controlsDisabled}
            openSignal={modelPickerOpenSignal}
          />
        )}
        <ReasoningPicker
          model={activeModel}
          effortValue={effort}
          contextWindowValue={contextWindow}
          labelMap={effortLabelMap}
          ultrathinkInBodyText={ultrathinkInBodyText}
          onEffortChange={onEffortChange}
          onContextWindowChange={onContextWindowChange}
          disabled={controlsDisabled}
          withSeparator
        />
        <SpeedPicker
          model={activeModel}
          value={fastMode}
          onChange={onFastModeChange}
          disabled={controlsDisabled}
          withSeparator
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
          withSeparator
        />
        {/* Host (Local / Remote) is a *workspace* property, not a
            chat-session property: pushing a workspace to a remote
            via right-click ships every pane it contains together —
            terminals, browsers, and the chat pane. A pill in this
            row would sit next to per-session controls (model,
            effort, permission) and teach the wrong mental model.
            When agent-chat-on-remote ships, the natural place for
            the DevicePicker is `DraftChatSurface`'s zone1Override
            (alongside the project + worktree pickers) — that's where
            "where will this materialize" decisions already live. */}
      </div>
      <div className="ml-auto">
        {streaming && showStopButton ? (
          <button
            type="button"
            onClick={onStop}
            className={cn(
              // Streaming interrupt: a saturated red circle (design's
              // `color-mix(red 85%, fg)`) so the stop affordance reads
              // as the one destructive action in the row. Distinct from
              // the neutral near-white Send so an in-flight turn is
              // obvious at a glance.
              "inline-flex h-8 w-8 items-center justify-center rounded-full",
              "bg-destructive/90 text-destructive-foreground shadow-xs shadow-destructive/25",
              "transition-all duration-150 hover:scale-105 hover:bg-destructive active:scale-100",
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
              // Primary action: near-white filled circle with a soft
              // tinted glow, a 150ms hover lift (scale) and a plain
              // opacity fade when there's nothing to send.
              "inline-flex h-8 w-8 items-center justify-center rounded-full",
              "bg-primary/90 text-primary-foreground shadow-xs shadow-primary/25",
              "transition-all duration-150 hover:scale-105 hover:bg-primary active:scale-100",
              "disabled:opacity-30 disabled:shadow-none disabled:cursor-not-allowed disabled:hover:scale-100",
            )}
            aria-label="Send"
            title="Send"
          >
            <ArrowUp className="h-4 w-4" strokeWidth={2.25} />
          </button>
        )}
      </div>
    </div>
  );
}
