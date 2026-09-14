import {
  ArrowUp,
  Check,
  ListTodo,
  LoaderCircle,
  Plus,
  Square,
} from "lucide-react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";

import { cn } from "@/lib/utils";
import type { ChatMode } from "@/stores/agent-chat-store";
import type { ContextUsageSnapshot } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ContextUsageMeter } from "./ContextUsageMeter";
import { ModelPicker } from "./pickers/ModelPicker";
import { MultiProviderModelPicker } from "./pickers/MultiProviderModelPicker";
import { PermissionModePicker } from "./pickers/PermissionModePicker";
import { ReasoningPicker } from "./pickers/ReasoningPicker";

/** 34px circle shared by attach / send / stop so the row's two ends sit on
 *  one optical baseline, concentric with the 44px pill (5px inset). */
const ROUND_CONTROL =
  "inline-flex h-[34px] w-[34px] shrink-0 items-center justify-center rounded-full";

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
  /** Active composer mode. Needed here to disable the permission picker
   *  when a mode pill commandeers permissions. */
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
  /** Disables provider/model/runtime configuration without disabling the
   *  attachment affordance. This lets an active provider turn keep accepting
   *  queued follow-ups while its native session configuration is frozen. */
  configurationDisabled?: boolean;
  /** Toggles the attach popup. When omitted the `+` button is hidden. */
  onAttachClick?: () => void;
  /** Whether the attach popup is currently open. */
  attachOpen?: boolean;
  /** Imperative model-picker open request from `/model`. */
  modelPickerOpenSignal?: number;
  /** Latest context-window occupancy for the thread. `null` renders no
   *  meter. */
  contextUsage?: ContextUsageSnapshot | null;
  contextUsageSeedMaxTokens?: number | null;
  contextUsageProviderLabel?: string | null;
  tasks?: { completed: number; total: number; running?: boolean } | null;
  tasksOpen?: boolean;
  onTasksClick?: () => void;
  /** Content for the flexible gap between the attach button and the
   *  right-pinned controls: the placeholder while the pill is collapsed,
   *  "Enter to queue" while expanded. */
  gap?: ReactNode;
  /** Pointer-down on the gap — the composer focuses its textarea. */
  onGapPointerDown?: (e: ReactPointerEvent<HTMLDivElement>) => void;
  /** The context ring lives in the expanded card only. Defaults to true. */
  showContextMeter?: boolean;
  /** Width ladder: model label shortens to its leaf name. */
  modelLeafLabel?: boolean;
  /** Width ladder: effort drops its text label. */
  effortIconOnly?: boolean;
  /** Width ladder: access drops its text label. */
  accessIconOnly?: boolean;
  /** Width ladder: effort + access leave the row for the `+` menu. */
  configInMenu?: boolean;
}

/**
 * The composer's single controls row. Identical geometry in the collapsed
 * pill and the expanded card, so nothing moves horizontally when the
 * textarea opens above it: attach pinned left, model / effort / access and
 * send pinned right, a flexible gap in between.
 */
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
  configurationDisabled = controlsDisabled,
  onAttachClick,
  attachOpen = false,
  modelPickerOpenSignal,
  contextUsage = null,
  contextUsageSeedMaxTokens = null,
  contextUsageProviderLabel = null,
  tasks = null,
  tasksOpen = false,
  onTasksClick,
  gap = null,
  onGapPointerDown,
  showContextMeter = true,
  modelLeafLabel = false,
  effortIconOnly = false,
  accessIconOnly = false,
  configInMenu = false,
}: Props) {
  const modeIsActive = mode !== "default";

  return (
    <div
      data-testid="composer-controls-row"
      className="flex h-[42px] shrink-0 items-center gap-1 px-1"
    >
      {onAttachClick && (
        <button
          type="button"
          onClick={onAttachClick}
          disabled={controlsDisabled}
          data-testid="composer-attach-button"
          data-open={attachOpen || undefined}
          className={cn(
            ROUND_CONTROL,
            // Transparent base border keeps the diameter fixed so the open
            // state's ember border causes no 1px shift.
            "border border-transparent",
            "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
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

      <div
        data-testid="composer-gap"
        onPointerDown={onGapPointerDown}
        // Reserve the meter's space even while collapsed, so it cannot
        // overlap settings or shift them when the composer expands.
        className="flex h-full min-w-[50px] flex-1 cursor-text items-center gap-2 pl-2"
      >
        <div className="flex min-w-0 flex-1 items-center">{gap}</div>
        {/* The ring rides the gap's trailing edge rather than the right
            cluster, so it can appear in the expanded card without nudging
            the pinned controls off their collapsed x positions. Its own
            clicks must not fall through to the gap's focus handler. */}
        {showContextMeter && (
          <div
            className="flex shrink-0 items-center"
            onPointerDown={(e) => e.stopPropagation()}
          >
            <ContextUsageMeter
              usage={contextUsage}
              seedMaxTokens={contextUsageSeedMaxTokens}
              providerLabel={contextUsageProviderLabel}
            />
          </div>
        )}
      </div>

      <div className="flex min-w-0 items-center gap-1">
        {tasks && tasks.total > 0 && onTasksClick && (
          <>
            {/* Reports run state rather than reading as a setting: amber +
                spinner while a step is in flight, green + check when the
                plan is complete, muted checklist before the run starts. */}
            <button
              type="button"
              onClick={onTasksClick}
              data-testid="composer-tasks-toggle"
              aria-pressed={tasksOpen}
              aria-label={
                modelLeafLabel
                  ? `Tasks: ${tasks.completed} of ${tasks.total} complete`
                  : undefined
              }
              className={cn(
                "inline-flex h-[34px] shrink-0 items-center gap-1.5 rounded-lg border-0 px-2.5 text-sm font-medium leading-none transition-colors",
                modelLeafLabel && "w-[34px] justify-center px-0",
                tasks.running
                  ? "bg-status-working/8 text-status-working hover:bg-status-working/15"
                  : tasks.completed === tasks.total
                    ? "bg-status-open/8 text-status-open hover:bg-status-open/15"
                    : tasksOpen
                      ? "bg-accent-ember/15 text-accent-ember"
                      : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
              )}
              title={`${tasks.completed} of ${tasks.total} tasks complete`}
            >
              {tasks.running ? (
                <LoaderCircle className="size-3.5 animate-spin" aria-hidden />
              ) : tasks.completed === tasks.total ? (
                <Check className="size-3.5" aria-hidden />
              ) : (
                <ListTodo className="size-3.5" aria-hidden />
              )}
              {!modelLeafLabel && (
                <>
                  <span>Tasks</span>
                  <span className="text-label tabular-nums opacity-70">
                    {tasks.completed}/{tasks.total}
                  </span>
                </>
              )}
            </button>
            <span className="mx-0.5 h-4 w-px shrink-0 bg-border/50" aria-hidden />
          </>
        )}

        {showProviderPicker ? (
          <MultiProviderModelPicker
            provider={provider}
            model={model}
            onProviderModelChange={onProviderModelChange}
            disabled={configurationDisabled}
            openSignal={modelPickerOpenSignal}
            leafLabel={modelLeafLabel}
          />
        ) : (
          <ModelPicker
            provider={provider}
            value={model}
            onChange={onModelChange}
            disabled={configurationDisabled}
            openSignal={modelPickerOpenSignal}
            leafLabel={modelLeafLabel}
          />
        )}
        {!configInMenu && (
          <>
            <ReasoningPicker
              model={activeModel}
              effortValue={effort}
              contextWindowValue={contextWindow}
              labelMap={effortLabelMap}
              ultrathinkInBodyText={ultrathinkInBodyText}
              fastMode={fastMode}
              onEffortChange={onEffortChange}
              onContextWindowChange={onContextWindowChange}
              onFastModeChange={onFastModeChange}
              disabled={configurationDisabled}
              withSeparator
              iconOnly={effortIconOnly}
            />
            {/* Stays visible while a mode pill is active (discoverability)
                but disabled, so it can't fight the pill's setting. */}
            <PermissionModePicker
              modes={permissionModes}
              value={permissionMode}
              onChange={onPermissionModeChange}
              disabled={configurationDisabled || modeIsActive}
              withSeparator
              iconOnly={accessIconOnly}
            />
          </>
        )}

        {streaming && showStopButton ? (
          <button
            type="button"
            onClick={onStop}
            className={cn(
              ROUND_CONTROL,
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
              ROUND_CONTROL,
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
