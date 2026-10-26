import type { MessageDelivery } from "@/lib/agent-chat/message-delivery";
import { COMPOSER_ROUND_CONTROL, COMPOSER_SEND } from "./composer-surface";
import { ChatGptPlanUsage } from "./chatgpt-plan-usage";
import { MessageDeliveryPicker } from "./MessageDeliveryPicker";
import {
  ArrowUp,
  Network,
  CornerDownRight,
  ListEnd,
  Plus,
  Square,
} from "lucide-react";
import {
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type Ref,
} from "react";

import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ChatMode } from "@/stores/agent-chat-store";
import type { ContextUsageSnapshot } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ContextUsageMeter } from "./ContextUsageMeter";
import { useStopTooltip } from "./composer-interrupt";
import { type PickerPlacement } from "./pickers/footer-trigger";
import { SCOPE_STRIP, SCOPE_STRIP_INSET } from "./pickers/ThreadScopeRow";
import { ModelPicker } from "./pickers/ModelPicker";
import { MultiProviderModelPicker } from "./pickers/MultiProviderModelPicker";
import { PermissionModePicker } from "./pickers/PermissionModePicker";
import { ReasoningPicker } from "./pickers/ReasoningPicker";

const DELIVERY_SEND_LABEL: Record<MessageDelivery, string> = {
  queue: "Queue message",
  steer: "Steer",
  interrupt: "Interrupt and send",
};

const DELIVERY_SEND_TITLE: Record<MessageDelivery, string> = {
  queue: "Send after this turn finishes",
  steer: "Guide the current task without stopping tools",
  interrupt: "Stop current work and send",
};

const ROUND_CONTROL = COMPOSER_ROUND_CONTROL;

/** The footer's tooltip, placed like the context meter's. The app Tooltip
 *  rather than a native `title`, so sweeping along the row opens each label
 *  at once after the first instead of waiting out the OS delay on every
 *  button. */
function FooterTooltip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="top" sideOffset={6}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}

interface Props {
  hermesThreadId?: string | null;
  hermesProjectPath?: string | null;
  customAgentsAllowed?: boolean;
  provider: AgentChatProviderKind;
  model: string | null;
  permissionMode: string | null;
  effort: string | null;
  contextWindow: string | null;
  serviceTier?: string;
  activeModel: ChatModelInfo | null;
  effortLabelMap: Record<string, string>;
  permissionModes: PermissionModeOption[] | null;
  ultrathinkInBodyText: boolean;
  streaming: boolean;
  canSubmit: boolean;
  /** The draft holds text to send. While a turn runs this swaps the Stop
   *  button for the queue/steer/interrupt send button — one slot, never
   *  both. Defaults to `canSubmit`. */
  hasDraft?: boolean;
  delivery?: MessageDelivery;
  supportsSteering?: boolean;
  onDeliveryChange?: (value: MessageDelivery) => void;
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
    model: string | null,
  ) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onServiceTierChange?: (serviceTier: string) => void;
  onSubmit: () => void;
  onBackgroundSubmit?: () => void;
  onStop: () => void;
  controlsDisabled: boolean;
  /** Disables provider/model/runtime configuration without disabling the
   *  attachment affordance. This lets an active provider turn keep accepting
   *  queued follow-ups while its native session configuration is frozen. */
  configurationDisabled?: boolean;
  /** Toggles the attach popup. When omitted the `+` button is hidden. */
  onAttachClick?: () => void;
  /** Disables the `+` button. Defaults to `controlsDisabled`; the menu can
   *  stay reachable for entries that don't need a ready session. */
  attachDisabled?: boolean;
  /** Whether the attach popup is currently open. */
  attachOpen?: boolean;
  /** Imperative model-picker open request from `/model`. */
  modelPickerOpenSignal?: number;
  permissionPickerOpenSignal?: number;
  /** Latest context-window occupancy for the thread. `null` renders no
   *  meter. */
  contextUsage?: ContextUsageSnapshot | null;
  contextUsageSeedMaxTokens?: number | null;
  contextUsageProviderLabel?: string | null;
  onWorkflowsClick?: () => void;
  workflowsOpen?: boolean;
  /** Content for the flexible gap between the attach button and the
   *  right-pinned controls: the placeholder while the pill is collapsed,
   *  "Enter to queue" while expanded. */
  gap?: ReactNode;
  /** Pointer-down on the gap — the composer focuses its textarea. */
  onGapPointerDown?: (e: ReactPointerEvent<HTMLDivElement>) => void;
  /** The gap element, measured by the composer to fit the row. */
  gapRef?: Ref<HTMLDivElement>;
  /** The resting pill: the row keeps only the placeholder, `+` and
   *  send / stop, and `ComposerChin` under the card carries the session
   *  settings, workflows toggle and context ring. */
  collapsed?: boolean;
  /** Width ladder: model label shortens to its leaf name. */
  modelLeafLabel?: boolean;
  /** Width ladder: provider/model control keeps only its provider mark. */
  modelIconOnly?: boolean;
  /** Width ladder: effort drops its text label. */
  effortIconOnly?: boolean;
  /** Width ladder: access and delivery drop their text labels. */
  accessIconOnly?: boolean;
  /** Width ladder: effort + access leave the row for the `+` menu. */
  configInMenu?: boolean;
}

/**
 * The composer's controls row. Expanded, the session settings lead on the
 * left and `+` / send close on the right. Collapsed to the pill, the row
 * keeps only the placeholder, `+` and send / stop; the settings drop into
 * `ComposerChin` under the card at the same x, so the model picker opens
 * from one place in both states.
 */
export function ComposerFooter({
  hermesThreadId, hermesProjectPath, customAgentsAllowed = true,
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  serviceTier = "default",
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  streaming,
  canSubmit,
  hasDraft = canSubmit,
  delivery = "queue",
  supportsSteering = false,
  onDeliveryChange,
  showProviderPicker,
  showStopButton = true,
  mode,
  onProviderModelChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onServiceTierChange = () => {},
  onSubmit,
  onBackgroundSubmit,
  onStop,
  controlsDisabled,
  configurationDisabled = controlsDisabled,
  onAttachClick,
  attachDisabled = controlsDisabled,
  attachOpen = false,
  modelPickerOpenSignal,
  permissionPickerOpenSignal,
  contextUsage = null,
  contextUsageSeedMaxTokens = null,
  contextUsageProviderLabel = null,
  onWorkflowsClick,
  workflowsOpen = false,
  gap = null,
  onGapPointerDown,
  gapRef,
  collapsed = false,
  modelLeafLabel = false,
  modelIconOnly = false,
  effortIconOnly = false,
  accessIconOnly = false,
  configInMenu = false,
}: Props) {
  const stopTooltip = useStopTooltip();

  return (
    <>
    <div
      data-testid="composer-controls-row"
      data-collapsed={collapsed || undefined}
      className={cn(
        "flex h-[42px] shrink-0 items-center gap-1 pr-1",
        // Expanded, the first trigger's icon lines up with the textarea's
        // text (20px), less the trigger's own 8px padding.
        collapsed ? "pl-1" : "pl-3",
      )}
    >
      {!collapsed && (
        <div className="flex min-w-0 items-center">
          <ComposerSettings
            hermesThreadId={hermesThreadId}
            hermesProjectPath={hermesProjectPath}
            customAgentsAllowed={customAgentsAllowed}
            provider={provider}
            model={model}
            permissionMode={permissionMode}
            effort={effort}
            contextWindow={contextWindow}
            serviceTier={serviceTier}
            activeModel={activeModel}
            effortLabelMap={effortLabelMap}
            permissionModes={permissionModes}
            ultrathinkInBodyText={ultrathinkInBodyText}
            showProviderPicker={showProviderPicker}
            mode={mode}
            onProviderModelChange={onProviderModelChange}
            onModelChange={onModelChange}
            onPermissionModeChange={onPermissionModeChange}
            onEffortChange={onEffortChange}
            onContextWindowChange={onContextWindowChange}
            onServiceTierChange={onServiceTierChange}
            configurationDisabled={configurationDisabled}
            modelPickerOpenSignal={modelPickerOpenSignal}
            permissionPickerOpenSignal={permissionPickerOpenSignal}
            placement="footer"
            modelLeafLabel={modelLeafLabel}
            modelIconOnly={modelIconOnly}
            effortIconOnly={effortIconOnly}
            accessIconOnly={accessIconOnly}
            configInMenu={configInMenu}
          />
        </div>
      )}

      <div
        ref={gapRef}
        data-testid="composer-gap"
        onPointerDown={onGapPointerDown}
        // Collapsed, the placeholder starts where the textarea's text
        // does once the card opens.
        className={cn(
          "flex h-full min-w-[50px] flex-1 cursor-text items-center gap-2",
          collapsed ? "pl-4" : "pl-2",
        )}
      >
        <div className="flex min-w-0 flex-1 items-center">{gap}</div>
        {/* The ring rides the gap's trailing edge, beside `+`. Its own
            clicks must not fall through to the gap's focus handler. */}
        {!collapsed && (
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
        {onWorkflowsClick && !collapsed && (
          <WorkflowsToggle open={workflowsOpen} onClick={onWorkflowsClick} />
        )}
        {streaming && onDeliveryChange && <MessageDeliveryPicker value={delivery} supportsSteering={supportsSteering} disabled={controlsDisabled} onChange={onDeliveryChange} iconOnly={accessIconOnly} />}
        {onAttachClick && (
          <FooterTooltip label="Attach (file, folder, mode, …)">
          <button
            type="button"
            onClick={onAttachClick}
            disabled={attachDisabled}
            data-testid="composer-attach-button"
            data-open={attachOpen || undefined}
            className={cn(
              ROUND_CONTROL,
              "text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground",
              "data-[open=true]:bg-accent-ember/15 data-[open=true]:text-accent-ember data-[open=true]:hover:bg-accent-ember/15 data-[open=true]:hover:text-accent-ember",
              "disabled:opacity-40 disabled:pointer-events-none",
            )}
            aria-label="Attach"
            aria-expanded={attachOpen}
          >
            <Plus className="size-4" />
          </button>
          </FooterTooltip>
        )}
        {streaming && onDeliveryChange && hasDraft ? (
          // One action slot, as in t3code: with a draft, the slot sends it
          // (queue / steer / interrupt); otherwise it's Stop.
          <FooterTooltip label={DELIVERY_SEND_TITLE[delivery]}>
          <button
            type="button"
            onClick={onSubmit}
            disabled={!canSubmit}
            aria-label={DELIVERY_SEND_LABEL[delivery]}
            className={cn(
              ROUND_CONTROL,
              COMPOSER_SEND,
            )}
          >
            {delivery === "queue" ? <ListEnd className="size-4" /> : delivery === "steer" ? <CornerDownRight className="size-4" /> : <ArrowUp className="size-4" />}
          </button>
          </FooterTooltip>
        ) : streaming && showStopButton ? (
          <FooterTooltip label={stopTooltip}>
          <button
            type="button"
            onClick={onStop}
            className={cn(
              ROUND_CONTROL,
              "bg-destructive/90 text-destructive-foreground shadow-xs shadow-destructive/25",
              "transition-[transform,background-color,color] duration-150 hover:scale-105 hover:bg-destructive active:scale-100",
            )}
            aria-label="Stop"
          >
            <Square className="size-3" fill="currentColor" />
          </button>
          </FooterTooltip>
        ) : (
          <FooterTooltip label={onBackgroundSubmit ? "Send (Ctrl+Enter or Ctrl-click to send in background)" : "Send"}>
          <button
            type="button"
            onClick={(e) => {
              if (e.ctrlKey && onBackgroundSubmit) onBackgroundSubmit();
              else onSubmit();
            }}
            disabled={!canSubmit || streaming}
            className={cn(
              ROUND_CONTROL,
              COMPOSER_SEND,
            )}
            aria-label="Send"
          >
            <ArrowUp className="size-4" />
          </button>
          </FooterTooltip>
        )}
      </div>
    </div>
    <ChatGptPlanUsage provider={provider} />
    </>
  );
}

function WorkflowsToggle({ open, onClick }: { open: boolean; onClick: () => void }) {
  return (
    <FooterTooltip label="Dynamic workflows">
    <button
      type="button"
      data-testid="composer-workflows-toggle"
      aria-label="Open dynamic workflows"
      aria-pressed={open}
      onClick={onClick}
      className={cn(
        "inline-flex size-[30px] shrink-0 items-center justify-center rounded-md transition-colors duration-100",
        open ? "bg-surface-3 text-foreground" : "text-muted-foreground hover:bg-surface-2 hover:text-foreground",
      )}
    >
      <Network className="size-3.5" aria-hidden />
    </button>
    </FooterTooltip>
  );
}

type ComposerChinProps = Omit<
  ComposerSettingsProps,
  | "placement"
  | "modelLeafLabel"
  | "modelIconOnly"
  | "effortIconOnly"
  | "accessIconOnly"
  | "configInMenu"
> &
  Pick<
    Props,
    | "contextUsage"
    | "contextUsageSeedMaxTokens"
    | "contextUsageProviderLabel"
    | "onWorkflowsClick"
    | "workflowsOpen"
  >;

/** Chin label steps, as in the controls row but without the last one:
 *  the chin has no `+` menu to hand effort and access to. */
const CHIN_COMPACT_STEPS = 4;

/**
 * The drawer under the resting pill: session settings on the left, the
 * workflows toggle and context ring on the right. It mirrors the strip
 * docked above the pill — 20px inset, 14px outer corners, an 18px seam
 * tucked under the card — and uses the controls row's own triggers, so a
 * picker opens from the same place whether the composer rests or not.
 *
 * Too narrow for every label, it drops them in the controls row's order
 * (model leaf name, access, effort, model) rather than moving anything
 * out of reach.
 */
export function ComposerChin({
  contextUsage = null,
  contextUsageSeedMaxTokens = null,
  contextUsageProviderLabel = null,
  onWorkflowsClick,
  workflowsOpen = false,
  ...settings
}: ComposerChinProps) {
  const rowRef = useRef<HTMLDivElement>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  const [level, setLevel] = useState(0);
  const fitKey = [
    settings.provider,
    settings.model,
    settings.effort,
    settings.contextWindow,
    settings.serviceTier,
    settings.permissionMode,
    contextUsage ? "ring" : "",
  ].join("|");
  // New labels may fit again: re-fit from full width.
  useLayoutEffect(() => setLevel(0), [fitKey]);
  // Each step re-renders and re-measures before paint, so an overflowing
  // intermediate state never shows. The settings group shrinks before the
  // trailing controls do, so its own overflow — triggers spilling over the
  // ring while the row as a whole still fits — counts as well.
  useLayoutEffect(() => {
    const overflows = (el: HTMLElement | null) =>
      !!el && el.scrollWidth > el.clientWidth;
    if (
      level < CHIN_COMPACT_STEPS &&
      (overflows(rowRef.current) || overflows(settingsRef.current))
    ) {
      setLevel(level + 1);
    }
  });
  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row || typeof ResizeObserver === "undefined") return;
    let width = row.clientWidth;
    const observer = new ResizeObserver(() => {
      if (row.clientWidth === width) return;
      width = row.clientWidth;
      setLevel(0);
    });
    observer.observe(row);
    return () => observer.disconnect();
  }, []);
  const ladder = {
    modelLeafLabel: level >= 1,
    accessIconOnly: level >= 2,
    effortIconOnly: level >= 3,
    modelIconOnly: level >= 4,
  };

  return (
    <div data-testid="composer-chin" className={cn(SCOPE_STRIP_INSET, "relative z-0")}>
      <div
        ref={rowRef}
        // The scope strip's chin, with its top 18px tucked under the card.
        className={cn(
          SCOPE_STRIP,
          "-mt-[18px] h-[52px] min-w-0 justify-start gap-1 overflow-hidden pt-[18px] pr-1 pb-1 pl-1",
        )}
      >
        <div
          ref={settingsRef}
          data-testid="composer-chin-settings"
          className="flex min-w-0 items-center"
        >
          <ComposerSettings {...settings} {...ladder} placement="footer" />
        </div>
        <div className="min-w-0 flex-1" />
        {onWorkflowsClick && (
          <WorkflowsToggle open={workflowsOpen} onClick={onWorkflowsClick} />
        )}
        <ContextUsageMeter
          usage={contextUsage}
          seedMaxTokens={contextUsageSeedMaxTokens}
          providerLabel={contextUsageProviderLabel}
        />
      </div>
    </div>
  );
}

type ComposerSettingsProps = Pick<
  Props,
  | "hermesThreadId"
  | "hermesProjectPath"
  | "customAgentsAllowed"
  | "provider"
  | "model"
  | "permissionMode"
  | "effort"
  | "contextWindow"
  | "serviceTier"
  | "activeModel"
  | "effortLabelMap"
  | "permissionModes"
  | "ultrathinkInBodyText"
  | "showProviderPicker"
  | "mode"
  | "onProviderModelChange"
  | "onModelChange"
  | "onPermissionModeChange"
  | "onEffortChange"
  | "onContextWindowChange"
  | "onServiceTierChange"
  | "configurationDisabled"
  | "modelPickerOpenSignal"
  | "permissionPickerOpenSignal"
  | "modelLeafLabel"
  | "modelIconOnly"
  | "effortIconOnly"
  | "accessIconOnly"
  | "configInMenu"
> & { placement: PickerPlacement };

/**
 * Model, effort and access pickers. They sit in the context row under the
 * composer when there is one, so they stay visible and one click away
 * without crowding the input; otherwise they sit in the controls row.
 */
export function ComposerSettings({
  hermesThreadId,
  hermesProjectPath,
  customAgentsAllowed = true,
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  serviceTier = "default",
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  showProviderPicker,
  mode,
  onProviderModelChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onServiceTierChange = () => {},
  configurationDisabled = false,
  modelPickerOpenSignal,
  permissionPickerOpenSignal,
  placement,
  modelLeafLabel = false,
  modelIconOnly = false,
  effortIconOnly = false,
  accessIconOnly = false,
  configInMenu = false,
}: ComposerSettingsProps) {
  const inFooter = placement === "footer";
  return (
    <>
      {showProviderPicker || provider === "hermes" || provider === "acp" ? (
        <MultiProviderModelPicker
          hermesThreadId={hermesThreadId}
          hermesProjectPath={hermesProjectPath}
          customAgentsAllowed={customAgentsAllowed}
          provider={provider}
          model={model}
          onProviderModelChange={onProviderModelChange}
          disabled={configurationDisabled}
          openSignal={modelPickerOpenSignal}
          leafLabel={modelLeafLabel}
          iconOnly={modelIconOnly}
          placement={placement}
        />
      ) : (
        <ModelPicker
          provider={provider}
          value={model}
          onChange={onModelChange}
          disabled={configurationDisabled}
          openSignal={modelPickerOpenSignal}
          leafLabel={modelLeafLabel}
          iconOnly={modelIconOnly}
          placement={placement}
        />
      )}
      {!configInMenu && provider !== "acp" && (
        <>
          <ReasoningPicker
            model={activeModel}
            effortValue={effort}
            contextWindowValue={contextWindow}
            labelMap={effortLabelMap}
            ultrathinkInBodyText={ultrathinkInBodyText}
            serviceTier={serviceTier}
            onEffortChange={onEffortChange}
            onContextWindowChange={onContextWindowChange}
            onServiceTierChange={onServiceTierChange}
            disabled={configurationDisabled}
            withSeparator={inFooter}
            iconOnly={effortIconOnly}
            placement={placement}
          />
          {/* Stays visible while a mode pill is active (discoverability)
              but disabled, so it can't fight the pill's setting. */}
          <PermissionModePicker
            openSignal={permissionPickerOpenSignal}
            modes={permissionModes}
            value={permissionMode}
            onChange={onPermissionModeChange}
            disabled={configurationDisabled || mode !== "default"}
            withSeparator={inFooter}
            iconOnly={accessIconOnly}
            placement={placement}
          />
        </>
      )}
    </>
  );
}
