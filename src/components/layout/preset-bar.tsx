import { useEffect, useState } from "react";
import { Settings } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuCheckboxItem,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
import { Button } from "@/components/ui/button";
import { PresetIcon } from "@/components/icons/preset-icon";
import { RunButton } from "./run-button";
import { cn } from "@/lib/utils";
import { materializeWithPreset } from "@/lib/agent-chat/materialize";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  useChatDraftStore,
  type DraftId,
} from "@/stores/chat-draft-store";
import { useUIStore } from "@/stores/ui-store";
import {
  agentChatCreatePane,
  applyPreset,
  getPresets,
  setPresetBarVisible,
} from "@/tauri/commands";
import { onPresetsChanged } from "@/tauri/events";
import type {
  PresetStoreSnapshot,
  TerminalPreset,
} from "@/tauri/types";
import { toast } from "@/lib/toast";

interface PresetBarProps {
  /** Real workspace id, or `null` when the bar is rendered above a
   *  draft surface. When null, `draftId` must be set. */
  workspaceId: string | null;
  /** Draft id when the bar sits above a draft surface. Clicking a
   *  preset then commits the draft (spawning only that preset's
   *  pane) via `materializeWithPreset`. */
  draftId?: DraftId;
  /** When true, all preset buttons render greyed-out and non-
   *  clickable. Used during a draft's materialise window so the user
   *  cannot swap presets mid-flight. */
  disabled?: boolean;
}

export function PresetBar({
  workspaceId,
  draftId,
  disabled = false,
}: PresetBarProps) {
  const [presetStore, setPresetStore] = useState<PresetStoreSnapshot | null>(null);

  useEffect(() => {
    getPresets().then((s) => setPresetStore(s)).catch(console.error);
    const unlisten = onPresetsChanged((snapshot) => setPresetStore(snapshot));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Read the active draft so we can (a) gate CLI presets on Home
  // drafts (Task 7b) and (b) materialise via a preset click. Zustand
  // returns `null` when draftId is omitted, so this is a no-op on
  // workspace surfaces.
  const activeDraft = useChatDraftStore((s) =>
    draftId ? s.draftsById[draftId] ?? null : null,
  );

  if (!presetStore || !presetStore.bar_visible) return null;

  const pinnedPresets = presetStore.presets.filter((p) => p.pinned);
  const inDraftMode = draftId != null;
  const isHomeDraft = inDraftMode && activeDraft?.target.kind === "home";

  function isPresetDisabled(_preset: TerminalPreset): boolean {
    if (disabled) return true;
    // On a Home draft every preset is disabled: CLI presets need
    // project context (launching `claude` at ~ is useless), and a
    // Chat Agent click here would spawn a duplicate chat on top of
    // the draft the user is already composing against. Picking a
    // project via Zone 1 flips the target to `project` and
    // re-enables the row.
    if (isHomeDraft) return true;
    return false;
  }

  function presetDisabledTooltip(preset: TerminalPreset): string | null {
    if (disabled) return null;
    if (isHomeDraft) {
      if (preset.kind === "chat_agent") {
        return "You're already in a chat — pick a project to add another.";
      }
      return "Select a project first to launch CLI agents.";
    }
    return null;
  }

  const handleLaunchWorkspace = (preset: TerminalPreset, e: React.MouseEvent) => {
    if (!workspaceId) return;
    if (preset.kind === "chat_agent") {
      // Spawn an agent_chat pane as a sibling in the current
      // workspace. Uses the same primitives as `materializeAndSend`
      // but with a freshly-minted thread id (no draft involved).
      void launchChatAgentOnWorkspace(preset, workspaceId).catch((err) => {
        const message =
          typeof err === "string"
            ? err
            : err instanceof Error
              ? err.message
              : String(err);
        toast.error(`${preset.name}: ${message}`);
        console.error("[preset-bar] chat_agent launch failed:", err);
      });
      return;
    }

    const mode = e.shiftKey ? ("split_pane" as const) : undefined;
    applyPreset(workspaceId, preset.id, mode).catch((err) => {
      // Backend returns `Err("{binary} is not installed")` (or other strings)
      // from `apply_preset` when `command_binary_exists` fails. Previously this
      // was swallowed into `console.error` and the user saw nothing — clicking
      // a preset for an uninstalled CLI appeared to do nothing at all.
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : String(err);
      toast.error(`${preset.name}: ${message}`);
      console.error("[preset-bar] applyPreset failed:", err);
    });
  };

  const handleLaunchDraft = (preset: TerminalPreset) => {
    if (!draftId) return;
    // Re-read fresh state at click time so same-tick composer
    // keystrokes are captured as the initial prompt.
    const state = useChatDraftStore.getState();
    const draft = state.draftsById[draftId];
    if (!draft) return;
    const chat = useAgentChatStore.getState();

    void materializeWithPreset(draft, preset, draft.inputDraft, {
      markPromoting: state.markPromoting,
      markPromoted: state.markPromoted,
      markSendFailed: state.markSendFailed,
      ensureThread: chat.ensureThread,
      appendUserMessage: chat.appendUserMessage,
      setModel: chat.setModel,
      setPermissionMode: chat.setPermissionMode,
      setSessionLaunchMode: chat.setSessionLaunchMode,
      setEffort: chat.setEffort,
      setContextWindow: chat.setContextWindow,
      setMode: chat.setMode,
    }).then((result) => {
      if (result.success) {
        // Same transition cleanup DraftChatSurface uses on composer
        // submit: clear the active draft immediately so the router
        // swaps to the live workspace; sweep the draft entry after
        // the 5s grace period.
        const draftIdToClear = draft.draftId;
        state.setActiveDraft(null);
        setTimeout(() => state.clearDraft(draftIdToClear), 5000);
      } else {
        toast.error(`${preset.name}: ${result.error ?? "Send failed"}`);
      }
    });
  };

  const handleLaunch = (preset: TerminalPreset, e: React.MouseEvent) => {
    if (isPresetDisabled(preset)) return;
    if (inDraftMode) {
      handleLaunchDraft(preset);
    } else {
      handleLaunchWorkspace(preset, e);
    }
  };

  const handleToggleBar = (checked: boolean) => {
    setPresetBarVisible(checked).catch(console.error);
  };

  const setShowSettings = useUIStore.getState().setShowSettings;

  return (
    <div
      className="flex items-center h-8 border-b border-border bg-background px-2 gap-0.5 shrink-0 overflow-x-auto"
      style={{ scrollbarWidth: "none" }}
    >
      {/* Settings gear */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Preset settings"
          >
            <Settings className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-52">
          <DropdownMenuCheckboxItem
            checked={presetStore.bar_visible}
            onCheckedChange={handleToggleBar}
          >
            Show Preset Bar
          </DropdownMenuCheckboxItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setShowSettings(true, "presets")}>
            <Settings className="h-4 w-4" />
            <span>Manage Presets</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Divider */}
      <Separator orientation="vertical" className="!h-4 !self-auto mx-0.5" />

      {/* Preset buttons */}
      {pinnedPresets.map((preset) => {
        const buttonDisabled = isPresetDisabled(preset);
        const tooltip =
          presetDisabledTooltip(preset) ??
          (inDraftMode ? null : "Shift+click to split");
        const button = (
          <Button
            variant="ghost"
            size="xs"
            className={cn(
              "gap-1.5 shrink-0",
              buttonDisabled && "opacity-40 cursor-not-allowed",
            )}
            disabled={buttonDisabled}
            aria-disabled={buttonDisabled}
            onClick={(e) => handleLaunch(preset, e)}
          >
            <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
            <span className="truncate max-w-[120px]">{preset.name}</span>
          </Button>
        );
        if (!tooltip) {
          return <span key={preset.id}>{button}</span>;
        }
        return (
          <Tooltip key={preset.id}>
            <TooltipTrigger asChild>{button}</TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="text-xs">
              {tooltip}
            </TooltipContent>
          </Tooltip>
        );
      })}

      {/* Spacer pushes run button right */}
      <div className="flex-1 min-w-0" />

      {/* Run button — only meaningful on real workspaces. */}
      {workspaceId && (
        <>
          <Separator orientation="vertical" className="!h-4 !self-auto mx-0.5" />
          <RunButton workspaceId={workspaceId} />
        </>
      )}
    </div>
  );
}

/** Spawn a ChatAgent preset on an existing workspace (no draft).
 *
 *  Kept at module scope so the click handler stays flat. We only
 *  call `agentChatCreatePane` here — the mounted `AgentChatPane`
 *  component starts its own session on first render (see
 *  AgentChatPane.tsx:157-202), so no explicit `start_session` call
 *  is needed at this layer. Provider defaults to Claude per the
 *  locked decision.
 */
async function launchChatAgentOnWorkspace(
  _preset: TerminalPreset,
  workspaceId: string,
): Promise<void> {
  await agentChatCreatePane(workspaceId, "claude", null);
}
