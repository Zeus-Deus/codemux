import { useEffect, useRef } from "react";

import { cn } from "@/lib/utils";
import type { ChatMode } from "@/stores/agent-chat-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ComposerFooter } from "./ComposerFooter";
import type { ActivePillMode } from "./pickers/ModePill";

interface Props {
  draft: string;
  cwd: string | null;
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
  sessionReady: boolean;
  showProviderPicker: boolean;
  /** Composer-level Cursor-style mode pill. Swaps the placeholder,
   *  hides the permission picker, and toggles the mode selector
   *  (dropdown → pill). */
  mode: ChatMode;
  /** When set, renders a muted inline banner above the textarea. Used
   *  for draft-send retry affordance (§8). `null` hides the banner. */
  errorMessage?: string | null;
  /** When false, hides the Stop button even while streaming. The draft
   *  surface uses this during materialise because there is no live
   *  session to interrupt. Defaults to true. */
  showStopButton?: boolean;
  /** Optional replacement for the Zone 1 strip (the cwd / "Home"
   *  label above the textarea). The draft surface uses this to thread
   *  a project picker in when the draft target is Home. Pass `null`
   *  (or omit) to keep the default cwd label. */
  zone1Override?: React.ReactNode;
  onDraftChange: (draft: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onModeActivate: (mode: ActivePillMode) => void;
  onModeRemove: () => void;
}

const MAX_ROWS_APPROX_PX = 32 + 7 * 20; // ~8 rows

export function Composer({
  draft,
  cwd,
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
  sessionReady,
  showProviderPicker,
  mode,
  errorMessage = null,
  showStopButton = true,
  zone1Override = null,
  onDraftChange,
  onSubmit,
  onStop,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onModeActivate,
  onModeRemove,
}: Props) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Auto-grow textarea up to ~8 rows.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const desired = Math.min(el.scrollHeight, MAX_ROWS_APPROX_PX);
    el.style.height = `${desired}px`;
  }, [draft]);

  const canSubmit = sessionReady && !streaming && draft.trim().length > 0;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (canSubmit) onSubmit();
    }
  };

  // Slash-command interception — only at draft start, only when no
  // mode pill is active (can't re-activate what's already on).
  // Stages 3, 4 and 6 wire `/plan`, `/ask`, and `/debug`.
  const handleTextareaChange = (
    e: React.ChangeEvent<HTMLTextAreaElement>,
  ) => {
    const value = e.target.value;
    if (mode === "default" && value.startsWith("/")) {
      const match = value.match(/^\/(plan|ask|debug)(?:\s|$)/);
      if (match) {
        const rest = value.slice(match[0].length).trimStart();
        if (match[1] === "plan") {
          onModeActivate("plan");
        } else if (match[1] === "ask") {
          onModeActivate("ask");
        } else if (match[1] === "debug") {
          onModeActivate("debug");
        }
        onDraftChange(rest);
        return;
      }
    }
    onDraftChange(value);
  };

  return (
    <div className="w-full px-4 pb-3">
      <div className="mx-auto w-full max-w-2xl">
        {zone1Override !== null
          ? <div className="pb-1">{zone1Override}</div>
          : cwd && (
              <div className="px-3 pb-1 text-[11px] text-muted-foreground/70 truncate font-mono">
                {cwd}
              </div>
            )}
        <div
          className={cn(
            "rounded-xl bg-muted/30 ring-1 ring-border/60 focus-within:ring-muted-foreground/60",
            "transition-shadow",
          )}
        >
          {errorMessage && (
            <div
              role="alert"
              className="px-3 pt-2 text-[11px] text-destructive/90 leading-tight"
            >
              <span>Send failed: {errorMessage}. </span>
              <span className="text-muted-foreground/80">
                Press Enter to retry.
              </span>
            </div>
          )}
          <textarea
            ref={textareaRef}
            value={draft}
            onChange={handleTextareaChange}
            onKeyDown={handleKeyDown}
            placeholder={
              sessionReady ? placeholderForMode(mode) : "Starting session…"
            }
            rows={1}
            className={cn(
              "w-full resize-none bg-transparent px-3 py-2.5",
              "text-sm text-foreground placeholder:text-muted-foreground/60",
              "outline-none",
            )}
            style={{ maxHeight: `${MAX_ROWS_APPROX_PX}px` }}
          />
          <ComposerFooter
            provider={provider}
            model={model}
            permissionMode={permissionMode}
            effort={effort}
            contextWindow={contextWindow}
            activeModel={activeModel}
            effortLabelMap={effortLabelMap}
            permissionModes={permissionModes}
            ultrathinkInBodyText={ultrathinkInBodyText}
            streaming={streaming}
            canSubmit={canSubmit}
            showProviderPicker={showProviderPicker}
            showStopButton={showStopButton}
            mode={mode}
            onModeActivate={onModeActivate}
            onModeRemove={onModeRemove}
            onProviderChange={onProviderChange}
            onModelChange={onModelChange}
            onPermissionModeChange={onPermissionModeChange}
            onEffortChange={onEffortChange}
            onContextWindowChange={onContextWindowChange}
            onSubmit={onSubmit}
            onStop={onStop}
            controlsDisabled={!sessionReady}
          />
        </div>
      </div>
    </div>
  );
}

/** Placeholder text per Cursor-style mode. `default` keeps the
 *  existing copy; Plan/Ask/Debug swap in mode-specific prompts so
 *  users see at a glance what the pill changes about their next
 *  message. */
function placeholderForMode(mode: ChatMode): string {
  switch (mode) {
    case "plan":
      return "Plan and design before coding…";
    case "ask":
      return "Ask questions without making changes…";
    case "debug":
      return "Debug and troubleshoot issues…";
    case "default":
      return "Message the agent…";
  }
}
