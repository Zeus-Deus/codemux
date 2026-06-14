import { useEffect, useMemo, useState } from "react";
import { Settings, Plus } from "lucide-react";
import {
  DndContext,
  PointerSensor,
  KeyboardSensor,
  useSensor,
  useSensors,
  closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  horizontalListSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
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
import { resolveSkillBodies } from "@/lib/agent-chat/skill-tokens";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { selectActiveSkills, useSkillsStore } from "@/stores/skills-store";
import {
  useChatDraftStore,
  type DraftId,
} from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import {
  agentChatCreatePane,
  applyPreset,
  getPresets,
  reorderPresets,
  setPresetBarVisible,
} from "@/tauri/commands";
import { onPresetsChanged } from "@/tauri/events";
import type {
  LaunchMode,
  PresetStoreSnapshot,
  TerminalPreset,
} from "@/tauri/types";
import { toast } from "@/lib/toast";

/** Translate a "drop position N in the pinned-only bar" into the
 *  splice index inside the GLOBAL preset Vec the backend stores.
 *
 *  The bar shows pinned presets only, but the global list also
 *  contains unpinned items (which keep their slots). Reordering a
 *  pinned preset in the bar should slide it to a new position
 *  *relative to the other pinned items*, leaving unpinned items
 *  exactly where they are.
 *
 *  Algorithm: look at the moved preset's neighbors in the new pinned
 *  order. The global index it lands at is "just before its new
 *  next-pinned neighbor" (or "just after its new previous-pinned
 *  neighbor" if it's now last). Adjusts for the fact that
 *  `Vec::remove(current) + Vec::insert(target)` shifts indices > current
 *  down by one — so the post-removal target is `next - 1` if
 *  current < next, else `next`.
 *
 *  Returns `null` if the preset id can't be found (caller should bail).
 */
function getTargetIndexForPinnedReorder(args: {
  presets: TerminalPreset[];
  pinnedPresetIds: string[];
  presetId: string;
  targetPinnedIndex: number;
}): number | null {
  const { presets, pinnedPresetIds, presetId, targetPinnedIndex } = args;
  const currentIndex = presets.findIndex((p) => p.id === presetId);
  if (currentIndex < 0) return null;

  const previousPinnedId =
    targetPinnedIndex > 0 ? pinnedPresetIds[targetPinnedIndex - 1] : undefined;
  const nextPinnedId =
    targetPinnedIndex < pinnedPresetIds.length - 1
      ? pinnedPresetIds[targetPinnedIndex + 1]
      : undefined;

  if (nextPinnedId !== undefined) {
    const nextIndex = presets.findIndex((p) => p.id === nextPinnedId);
    if (nextIndex < 0) return null;
    return currentIndex < nextIndex ? nextIndex - 1 : nextIndex;
  }

  if (previousPinnedId !== undefined) {
    const previousIndex = presets.findIndex((p) => p.id === previousPinnedId);
    if (previousIndex < 0) return null;
    const adjustedPrev =
      currentIndex < previousIndex ? previousIndex - 1 : previousIndex;
    return adjustedPrev + 1;
  }

  return currentIndex;
}

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
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);

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

  // Local pinned order — drives the visible bar. Lets a drag re-render
  // smoothly without a server round-trip per pixel of motion. A
  // `useEffect` below reconciles this with the server snapshot when it
  // changes.
  const [localPinnedOrder, setLocalPinnedOrder] = useState<string[]>([]);

  const serverPinnedIds = useMemo(
    () =>
      (presetStore?.presets ?? [])
        .filter((p) => p.pinned)
        .map((p) => p.id),
    [presetStore],
  );

  useEffect(() => {
    setLocalPinnedOrder((current) => {
      // Replace local order whenever the server's pinned set changes
      // (different ids OR different order). A drag commits its result
      // through the reorder mutation, so the next snapshot will already
      // reflect it — this effect is the resync, not a fight with it.
      if (
        current.length === serverPinnedIds.length &&
        current.every((id, i) => id === serverPinnedIds[i])
      ) {
        return current;
      }
      return serverPinnedIds;
    });
  }, [serverPinnedIds]);

  // Click-vs-drag: only start a drag after 5px of pointer movement.
  // Below that threshold the pointerup fires `onClick` on the inner
  // button, so launching a preset still works while the wrapper is
  // also a drag handle.
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  if (!presetStore || !presetStore.bar_visible) return null;

  // Resolve the pinned presets in local order, then append any
  // server-pinned items not yet in local (newly-pinned via settings)
  // so they show up immediately without waiting for the next render.
  // Step 13 — drop chat_agent presets when the master Beta toggle is
  // off so a user who never opted in never sees the affordance. The
  // preset still exists in the store; we just hide it at the UI layer
  // (data preservation, see docs/plans/step-13-beta-toggle-research.md
  // §5).
  const allPresets = enableAgentChat
    ? presetStore.presets
    : presetStore.presets.filter((p) => p.kind !== "chat_agent");
  const presetById = new Map(allPresets.map((p) => [p.id, p]));
  const pinnedPresets: TerminalPreset[] = [];
  const seen = new Set<string>();
  for (const id of localPinnedOrder) {
    const p = presetById.get(id);
    if (p && p.pinned) {
      pinnedPresets.push(p);
      seen.add(id);
    }
  }
  for (const p of allPresets) {
    if (p.pinned && !seen.has(p.id)) {
      pinnedPresets.push(p);
    }
  }

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
      // Match CLI preset semantics: plain click opens a new tab,
      // shift+click splits the current surface. Without this, every
      // Chat Agent click would split the active pane (the backend's
      // create_agent_chat_pane defaults to split when surfaces
      // already exist) — surprising when the user expected a fresh
      // tab parallel to existing CLI preset launches.
      const launchMode: LaunchMode = e.shiftKey ? "split_pane" : "new_tab";
      void launchChatAgentOnWorkspace(preset, workspaceId, launchMode).catch((err) => {
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
    // Structured "agent launcher" presets carry their own model/reasoning
    // choice — apply it the same way the New Workspace launch picker does
    // (the prompt is already baked into the preset's command).
    const modelSelection = preset.launch_config?.model_selection ?? null;
    applyPreset(workspaceId, preset.id, mode, null, modelSelection).catch((err) => {
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

    const skillBodies = resolveSkillBodies(
      draft.inputDraft,
      selectActiveSkills(useSkillsStore.getState()),
    );

    void materializeWithPreset(
      draft,
      preset,
      draft.inputDraft,
      {
        markPromoting: state.markPromoting,
        markMaterialized: state.markMaterialized,
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
      },
      skillBodies,
    ).then((result) => {
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

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const activeId = String(active.id);
    const overId = String(over.id);
    const oldIndex = localPinnedOrder.indexOf(activeId);
    const newIndex = localPinnedOrder.indexOf(overId);
    if (oldIndex < 0 || newIndex < 0) return;

    const nextOrder = arrayMove(localPinnedOrder, oldIndex, newIndex);
    setLocalPinnedOrder(nextOrder);

    if (!presetStore) return;
    const targetIndex = getTargetIndexForPinnedReorder({
      presets: presetStore.presets,
      pinnedPresetIds: nextOrder,
      presetId: activeId,
      targetPinnedIndex: newIndex,
    });
    if (targetIndex === null) return;

    reorderPresets(activeId, targetIndex).catch((err) => {
      console.error("[preset-bar] reorderPresets failed:", err);
      // Roll back local order if the server rejected the move.
      setLocalPinnedOrder(serverPinnedIds);
    });
  };

  const setShowSettings = useUIStore.getState().setShowSettings;
  const requestNewPreset = useUIStore.getState().requestNewPreset;

  // Translate a vertical mouse-wheel delta into horizontal scrolling so
  // the bar can be panned with a plain wheel when many pinned presets
  // overflow a narrow window. `overflow-x: auto` only responds to
  // horizontal wheel / trackpad input on its own (verified on the
  // WebKit webview — a vertical wheel over the bar moved `scrollLeft`
  // by 0). `deltaX` is left to that native horizontal-scroll path.
  const handleWheel = (e: React.WheelEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (el.scrollWidth <= el.clientWidth || e.deltaY === 0) return;
    el.scrollLeft += e.deltaY;
  };

  return (
    <div
      className="flex items-center h-8 border-b border-border bg-background px-2 gap-0.5 shrink-0 overflow-x-auto"
      style={{ scrollbarWidth: "none" }}
      onWheel={handleWheel}
    >
      {/* Settings gear */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Preset settings"
          >
            <Settings className="size-4" />
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
          <DropdownMenuItem onClick={() => requestNewPreset()}>
            <Plus className="h-4 w-4" />
            <span>New Preset</span>
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => setShowSettings(true, "presets")}>
            <Settings className="h-4 w-4" />
            <span>Manage Presets</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Divider */}
      <Separator orientation="vertical" className="!h-4 !self-auto mx-0.5" />

      {/* Preset buttons — drag to reorder. The 5px activation distance
          (PointerSensor) means a normal click still launches the
          preset; only sustained drag motion engages the sort. */}
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={pinnedPresets.map((p) => p.id)}
          strategy={horizontalListSortingStrategy}
        >
          {pinnedPresets.map((preset) => {
            const buttonDisabled = isPresetDisabled(preset);
            const tooltip =
              presetDisabledTooltip(preset) ??
              (inDraftMode ? null : "Shift+click to split");
            return (
              <SortablePresetButton
                key={preset.id}
                preset={preset}
                disabled={buttonDisabled}
                tooltip={tooltip}
                onClick={(e) => handleLaunch(preset, e)}
              />
            );
          })}
        </SortableContext>
      </DndContext>

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

interface SortablePresetButtonProps {
  preset: TerminalPreset;
  disabled: boolean;
  tooltip: string | null;
  onClick: (e: React.MouseEvent) => void;
}

function SortablePresetButton({
  preset,
  disabled,
  tooltip,
  onClick,
}: SortablePresetButtonProps) {
  // `attributes` from useSortable is intentionally NOT spread — see
  // the wrapper below for why.
  const {
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: preset.id });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : undefined,
  };

  const button = (
    <Button
      variant="ghost"
      size="xs"
      className={cn(
        "gap-1.5 shrink-0",
        disabled && "opacity-40 cursor-not-allowed",
      )}
      disabled={disabled}
      aria-disabled={disabled}
      onClick={onClick}
    >
      <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
      <span className="truncate max-w-[120px]">{preset.name}</span>
    </Button>
  );

  const inner = tooltip ? (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4} className="text-xs">
        {tooltip}
      </TooltipContent>
    </Tooltip>
  ) : (
    button
  );

  // dnd-kit's `attributes` adds `role="button"` and `tabIndex=0` to
  // the wrapper. Spreading them here would create a second focusable
  // "button" with the same accessible name as the inner Button —
  // confusing for screen readers and ambiguous for tests. We strip
  // both so the inner Button stays the only semantic button; drag
  // remains pointer-driven (mouse/touch), matching the existing UX.
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...listeners}
      role={undefined}
      tabIndex={-1}
      className="shrink-0 touch-none"
    >
      {inner}
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
  launchMode: LaunchMode,
): Promise<void> {
  await agentChatCreatePane(workspaceId, "claude", null, launchMode);
}
