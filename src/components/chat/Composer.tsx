import { useEffect, useRef } from "react";

import { cn } from "@/lib/utils";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ComposerFooter } from "./ComposerFooter";

interface Props {
  draft: string;
  cwd: string | null;
  isHomeWorkspace?: boolean;
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
  onDraftChange: (draft: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
}

const MAX_ROWS_APPROX_PX = 32 + 7 * 20; // ~8 rows

export function Composer({
  draft,
  cwd,
  isHomeWorkspace = false,
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
  onDraftChange,
  onSubmit,
  onStop,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
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

  return (
    <div className="w-full px-4 pb-3">
      <div className="mx-auto w-full max-w-2xl">
        {isHomeWorkspace ? (
          <div className="px-2 pb-1 text-[11px] text-muted-foreground/70 truncate">
            Home
          </div>
        ) : (
          cwd && (
            <div className="px-2 pb-1 text-[11px] text-muted-foreground/70 truncate font-mono">
              {cwd}
            </div>
          )
        )}
        <div
          className={cn(
            "rounded-xl bg-muted/30 ring-1 ring-border/60 focus-within:ring-muted-foreground/60",
            "transition-shadow",
          )}
        >
          <textarea
            ref={textareaRef}
            value={draft}
            onChange={(e) => onDraftChange(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={
              sessionReady ? "Message the agent…" : "Starting session…"
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
