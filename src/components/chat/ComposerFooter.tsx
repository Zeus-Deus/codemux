import {
  ArrowUp,
  Bug,
  ListTodo,
  MessageCircleQuestion,
  Plus,
  Square,
} from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import type { ChatMode } from "@/stores/agent-chat-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ModelPicker } from "./pickers/ModelPicker";
import { ModePill, type ActivePillMode } from "./pickers/ModePill";
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
  /** Composer-level Cursor-style mode pill. `default` renders the
   *  `+ Mode` dropdown trigger; any other value renders a ModePill
   *  and hides the permission picker. */
  mode: ChatMode;
  onModeActivate: (mode: ActivePillMode) => void;
  onModeRemove: () => void;
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
  mode,
  onModeActivate,
  onModeRemove,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onSubmit,
  onStop,
  controlsDisabled,
}: Props) {
  const modeIsActive = mode !== "default";

  return (
    <div className="flex items-center gap-1.5 px-3 pb-2 pt-1">
      <div className="flex flex-wrap items-center gap-1.5 min-w-0">
        {/* Mode selector: dropdown trigger when default, pill when
            active. Locked decision: picker is HIDDEN (not just
            disabled) while a pill is active so the user's mental
            model stays clean — the pill commandeers the permission
            policy. */}
        {modeIsActive ? (
          <ModePill
            mode={mode as ActivePillMode}
            onRemove={onModeRemove}
          />
        ) : (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                disabled={controlsDisabled}
                className={cn(
                  // Match the padding + gap of sibling picker triggers
                  // (ModelPicker, ReasoningPicker, PermissionModePicker)
                  // which all use `gap-1.5 px-2.5 py-1`.
                  "inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs",
                  "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
                  "disabled:opacity-50 disabled:pointer-events-none",
                )}
                aria-label="Activate mode"
                title="Activate mode (Shift+Tab to cycle)"
              >
                <Plus className="h-3 w-3" />
                <span>Mode</span>
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="text-xs">
              <DropdownMenuItem
                onClick={() => onModeActivate("plan")}
                className="text-xs gap-2"
              >
                <ListTodo className="h-3.5 w-3.5" />
                <span>Plan</span>
                <span className="ml-auto text-[10px] text-muted-foreground">
                  /plan
                </span>
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => onModeActivate("ask")}
                className="text-xs gap-2"
              >
                <MessageCircleQuestion className="h-3.5 w-3.5" />
                <span>Ask</span>
                <span className="ml-auto text-[10px] text-muted-foreground">
                  /ask
                </span>
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => onModeActivate("debug")}
                className="text-xs gap-2"
              >
                <Bug className="h-3.5 w-3.5" />
                <span>Debug</span>
                <span className="ml-auto text-[10px] text-muted-foreground">
                  /debug
                </span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
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
