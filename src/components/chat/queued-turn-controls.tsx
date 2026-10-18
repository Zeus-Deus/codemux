/**
 * The follow-up queue, shown as a list in the composer's top chin.
 *
 * Queued turns stay out of the transcript (see `withoutQueuedTurns`); each
 * shows up as a normal user message once it dispatches. The list sits in
 * the composer strip's shell (`ComposerStrip`'s `queue` slot): a header row
 * that collapses it, then one row per turn in dispatch order with a drag
 * grip (arrow keys move a focused grip), a one-line preview, and its
 * actions: edit, steer where the provider can, interrupt, remove.
 *
 * Edit loads the turn's text into the main composer. While it is loaded,
 * the row is highlighted and offers only Cancel; submitting the composer
 * saves the text onto the queued turn instead of sending a new one, and
 * Escape or Cancel puts the previous draft back. The turn keeps its
 * attachments: an edit changes only its words.
 *
 * A *saved* turn (`queued.saved`) was queued before the app restarted. No
 * provider holds it, so its row offers Send, which goes through the normal
 * send path, instead of Steer and Interrupt. Nothing sends it until the
 * user does.
 */
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import {
  ArrowUp,
  ChevronDown,
  Clock,
  CornerDownLeft,
  CornerUpRight,
  GripVertical,
  ImageIcon,
  Pencil,
  X,
} from "lucide-react";

import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useTauriEvent } from "@/hooks/use-tauri-event";
import { editQueuedTurn, reorderQueuedTurns } from "@/lib/agent-chat/queue-edits";
import { applyEvent } from "@/lib/agent-chat/reducer";
import { replaceTranscriptItem } from "@/lib/agent-chat/subagents";
import type { ChatThreadState, UserMessageItem } from "@/lib/agent-chat/types";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatCancelSavedQueuedTurn,
  agentChatEditQueuedTurn,
  agentChatListQueuedTurns,
  agentChatMoveQueuedTurn,
  agentChatSendSavedQueuedTurn,
  type QueuedTurnView,
} from "@/tauri/commands";
import { onAgentChatQueuedTurnsChanged } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { STRIP_ROW_HEIGHT } from "./ComposerStrip";

// ── Editing in the composer ────────────────────────────────────────────

export interface QueuedTurnEditor {
  /** The queued turn whose text is loaded in the composer. */
  editingId: string | null;
  /** It keeps images, so its words may be cleared. */
  editingHasImages: boolean;
  begin: (item: UserMessageItem) => void;
  /** Put the draft from before the edit back. */
  cancel: () => void;
  /** Save `text` onto the turn being edited (the composer's submit). */
  save: (text: string) => Promise<void>;
}

function draftOf(threadId: string): string {
  return useAgentChatStore.getState().threads[threadId]?.inputDraft ?? "";
}

/** Two drafts in one composer, the way a cancelled turn's text joins one. */
function joinDrafts(first: string, second: string): string {
  if (!first.trim()) return second;
  if (!second.trim()) return first;
  return `${first}\n${second}`;
}

/** `current` without `text`, when the store appended it as it does for a
 *  cancelled turn (on a new line, or alone in an empty composer). */
function withoutReturned(current: string, text: string): string | null {
  if (!text.trim()) return null;
  if (current === text) return "";
  return current.endsWith(`\n${text}`) ? current.slice(0, -(text.length + 1)) : null;
}

/** The composer region the queue list belongs to: its nearest ancestor
 *  holding a composer input. */
function composerRegionOf(element: HTMLElement | null): HTMLElement | null {
  let node = element;
  while (node && !node.querySelector("[data-composer-input]")) node = node.parentElement;
  return node;
}

function focusComposerNear(element: HTMLElement | null) {
  composerRegionOf(element)?.querySelector<HTMLTextAreaElement>("[data-composer-input]")?.focus();
}

export function useQueuedTurnEditor({
  threadId,
  provider,
}: {
  threadId: string | null;
  provider: AgentChatProviderKind;
}): QueuedTurnEditor {
  const setInputDraft = useAgentChatStore((s) => s.setInputDraft);
  const messages = useAgentChatStore((s) => (threadId ? s.threads[threadId]?.messages : undefined));
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingHasImages, setEditingHasImages] = useState(false);
  // The draft the edit displaced, the turn's bubble and the text it loaded,
  // and the text a save in flight submitted.
  const editRef = useRef<{
    threadId: string;
    queuedId: string;
    itemId: string;
    draft: string;
    original: string;
    saving?: string;
    /** The turn was cancelled; its returned copy is already stripped. */
    cancelled?: boolean;
  } | null>(null);

  /** End the edit. `restore` puts the displaced draft back; `keep` leaves
   *  the composer's words and adds the displaced draft above them, so
   *  neither is lost. */
  const finish = useCallback(
    (mode: "restore" | "keep") => {
      const edit = editRef.current;
      editRef.current = null;
      setEditingId(null);
      setEditingHasImages(false);
      if (!edit) return;
      setInputDraft(
        edit.threadId,
        mode === "restore" ? edit.draft : joinDrafts(edit.draft, draftOf(edit.threadId)),
      );
    },
    [setInputDraft],
  );

  const begin = useCallback(
    (item: UserMessageItem) => {
      if (!threadId || !item.queued) return;
      const previous = editRef.current;
      const draft = previous?.threadId === threadId ? previous.draft : draftOf(threadId);
      editRef.current = {
        threadId,
        queuedId: item.queued.queuedId,
        itemId: item.id,
        draft,
        original: item.text,
      };
      setEditingId(item.queued.queuedId);
      setEditingHasImages((item.images?.length ?? 0) > 0);
      setInputDraft(threadId, item.text);
    },
    [threadId, setInputDraft],
  );

  const cancel = useCallback(() => finish("restore"), [finish]);

  const save = useCallback(
    async (text: string) => {
      const edit = editRef.current;
      if (!edit || edit.saving !== undefined) return;
      if (text.trim() === edit.original.trim()) return finish("restore");
      edit.saving = text;
      try {
        const still = await agentChatEditQueuedTurn(provider, edit.threadId, edit.queuedId, text);
        if (editRef.current !== edit) return;
        // Cancelled while saving, whichever won: its words come back once.
        if (edit.cancelled) return finish("keep");
        if (still) {
          // Anything typed while the save was in flight is not the saved
          // text, so it stays in the composer.
          return finish(draftOf(edit.threadId) === text ? "restore" : "keep");
        }
        // It dispatched first; the words stay in the composer as a draft.
        toast.error("That message was sent before the edit was saved.", {
          description: "Your edit is still in the composer.",
        });
        finish("keep");
      } catch (error) {
        edit.saving = undefined;
        toast.failure("Couldn't edit the queued message", error);
      }
    },
    [provider, finish],
  );

  // Queued text as of the last render, to recognise what a removal hands
  // back to the composer.
  const queuedTextRef = useRef(new Map<string, string>());
  useEffect(() => {
    const before = queuedTextRef.current;
    const now = new Map<string, string>();
    for (const m of messages ?? []) {
      if (m.kind === "user_message" && m.queued) now.set(m.queued.queuedId, m.text);
    }
    queuedTextRef.current = now;
    const edit = editRef.current;
    if (!edit || !messages) return;
    // Removing another queued turn returns its text to the composer, which
    // holds this edit. Move it onto the displaced draft, so it is there
    // whichever way the edit ends.
    for (const [queuedId, text] of before) {
      if (queuedId === edit.queuedId || now.has(queuedId)) continue;
      const rest = withoutReturned(draftOf(edit.threadId), text);
      if (rest === null) continue;
      setInputDraft(edit.threadId, rest);
      edit.draft = joinDrafts(edit.draft, text);
    }
    // The edited turn left the queue.
    if (now.has(edit.queuedId)) return;
    const cancelled = !messages.some((m) => m.id === edit.itemId);
    if (cancelled && !edit.cancelled) {
      // Its text came back to the composer too. Drop that copy: the words
      // in the composer (the original or the user's edit) are what return.
      edit.cancelled = true;
      // The copy is its text when cancelled: as last shown, or as a save in
      // flight just changed it. Longest first: the shown text can be the
      // tail of the edited one, and stripping only that tail would leave
      // the rest of the copy behind.
      const current = draftOf(edit.threadId);
      const returned = [before.get(edit.queuedId) ?? edit.original, edit.saving]
        .filter((text): text is string => text !== undefined)
        .sort((a, b) => b.length - a.length);
      for (const text of returned) {
        const rest = withoutReturned(current, text);
        if (rest === null) continue;
        setInputDraft(edit.threadId, rest);
        break;
      }
    }
    // A save in flight settles the edit itself.
    if (edit.saving !== undefined) return;
    if (cancelled) return finish("keep");
    // It dispatched: keep a changed draft, otherwise put the earlier one back.
    finish(draftOf(edit.threadId).trim() === edit.original.trim() ? "restore" : "keep");
  }, [messages, finish, setInputDraft]);

  // A different thread is a different composer.
  useEffect(() => {
    const edit = editRef.current;
    if (edit && edit.threadId !== threadId) finish("restore");
  }, [threadId, finish]);

  // Leaving (a closed pane, another tab) ends the edit: the displaced draft
  // comes back, so the queued text is never left to be sent again.
  useEffect(
    () => () => {
      const edit = editRef.current;
      editRef.current = null;
      if (edit) useAgentChatStore.getState().setInputDraft(edit.threadId, edit.draft);
    },
    [],
  );

  return useMemo(
    () => ({ editingId, editingHasImages, begin, cancel, save }),
    [editingId, editingHasImages, begin, cancel, save],
  );
}

// ── The list ───────────────────────────────────────────────────────────

const ACTION =
  "inline-flex h-[22px] shrink-0 items-center justify-center gap-1 rounded-sm px-1.5 text-label font-semibold text-muted-foreground outline-none transition-colors duration-150 hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-40";
const ICON_ACTION = cn(ACTION, "w-[22px] px-0");

/** Where a drop lands: before `index` in the dragged turn's group. */
interface DragState {
  queuedId: string;
  insertIndex: number | null;
}

export interface QueuedTurnsListProps {
  threadId: string;
  provider: AgentChatProviderKind;
  /** Queued turns in dispatch order (`queuedMessages`). */
  queued: UserMessageItem[];
  editor: QueuedTurnEditor;
  /** A turn is running, so steer and interrupt have something to act on. */
  streaming: boolean;
  canSteer: boolean;
  onSteer: (queuedId: string) => void;
  onInterrupt: (queuedId: string) => void;
  onRemove: (queuedId: string) => void;
  requestSendAnchor: (clientNonce: string) => void;
  clearSendAnchor: (clientNonce: string) => void;
}

/** Saved and live turns are separate queues; each reorders on its own. */
function groupOf(queued: UserMessageItem[], saved: boolean): string[] {
  return queued.filter((m) => !!m.queued?.saved === saved).map((m) => m.queued!.queuedId);
}

export function QueuedTurnsList({
  threadId,
  provider,
  queued,
  editor,
  streaming,
  canSteer,
  onSteer,
  onInterrupt,
  onRemove,
  requestSendAnchor,
  clearSendAnchor,
}: QueuedTurnsListProps) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(true);
  // A turn being edited (by its pencil or the shortcut) is always in view.
  useEffect(() => {
    if (editor.editingId) setExpanded(true);
  }, [editor.editingId]);
  // Escape from this composer puts the previous draft back. Claiming it
  // keeps the composer's Escape-to-stop from stopping the run. Another
  // pane's composer, or this one under an inert page, is left alone.
  const { editingId, cancel } = editor;
  useEffect(() => {
    if (!editingId) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      const root = rootRef.current;
      if (!root || root.closest("[inert]")) return;
      const region = composerRegionOf(root);
      if (!region || !(event.target instanceof Node) || !region.contains(event.target)) return;
      event.preventDefault();
      cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [editingId, cancel]);
  const [drag, setDrag] = useState<DragState | null>(null);
  // Drags start from the grip only: it arms its row on pointerdown and
  // dragstart checks it, so selecting preview text never drags the row.
  const armedRef = useRef<string | null>(null);
  // A grip moved by keyboard keeps focus once its row lands in place.
  const refocusRef = useRef<string | null>(null);
  useEffect(() => {
    const queuedId = refocusRef.current;
    if (!queuedId) return;
    const grip = rootRef.current?.querySelector<HTMLButtonElement>(
      `[data-queued-id="${CSS.escape(queuedId)}"] [data-queued-grip]`,
    );
    if (grip && document.activeElement !== grip) grip.focus();
  }, [queued]);

  const move = useCallback(
    (queuedId: string, before: string | null) => {
      agentChatMoveQueuedTurn(provider, threadId, queuedId, before).catch((error) => {
        toast.failure("Couldn't move the queued message", error);
      });
    },
    [provider, threadId],
  );

  const sendSaved = useCallback(
    (item: UserMessageItem) => {
      const nonce = item.clientNonce;
      if (nonce) requestSendAnchor(nonce);
      agentChatSendSavedQueuedTurn(provider, threadId, item.queued!.queuedId, "queue").catch((error) => {
        if (nonce) clearSendAnchor(nonce);
        toast.failure("Couldn't send the saved message", error);
      });
    },
    [provider, threadId, requestSendAnchor, clearSendAnchor],
  );

  const remove = useCallback(
    (item: UserMessageItem) => {
      const queuedId = item.queued!.queuedId;
      if (!item.queued?.saved) return onRemove(queuedId);
      agentChatCancelSavedQueuedTurn(threadId, queuedId).catch((error) => {
        toast.failure("Couldn't remove the saved message", error);
      });
    },
    [threadId, onRemove],
  );

  const completeDrag = (state: DragState | null) => {
    setDrag(null);
    armedRef.current = null;
    if (!state || state.insertIndex === null) return;
    const item = queued.find((m) => m.queued?.queuedId === state.queuedId);
    if (!item) return;
    const ids = groupOf(queued, !!item.queued?.saved);
    const from = ids.indexOf(state.queuedId);
    // Dropping right before or after itself changes nothing.
    if (from < 0 || state.insertIndex === from || state.insertIndex === from + 1) return;
    move(state.queuedId, ids[state.insertIndex] ?? null);
  };

  if (queued.length === 0) return null;
  const count = queued.length;

  // Its own provider, so the list works wherever the strip is mounted.
  return (
    <TooltipProvider delayDuration={300}>
    <div
      ref={rootRef}
      role="region"
      aria-label={`${count} queued message${count === 1 ? "" : "s"}`}
      data-testid="queued-turns"
    >
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-label={expanded ? "Collapse queued messages" : "Expand queued messages"}
        onClick={() => setExpanded((open) => !open)}
        className={cn(
          "flex w-full items-center gap-2.5 rounded-t-lg px-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60",
          STRIP_ROW_HEIGHT,
        )}
      >
        <span className="flex size-5 shrink-0 items-center justify-center">
          <Clock className="size-3.5 text-muted-foreground" aria-hidden />
        </span>
        <span className="text-body-sm font-semibold text-foreground/80">Queued</span>
        <span className="ml-auto font-mono text-label tabular-nums text-muted-foreground">{count}</span>
        <span className="flex size-[26px] items-center justify-center text-muted-foreground" aria-hidden>
          <ChevronDown className={cn("size-3.5 transition-transform duration-150", !expanded && "rotate-180")} />
        </span>
      </button>
      <ol
        id={listId}
        aria-label="Queued messages"
        className={cn("flex max-h-32 flex-col gap-px overflow-y-auto px-1 pb-1 thin-scrollbar", !expanded && "hidden")}
      >
        {queued.map((item) => {
          const queuedId = item.queued!.queuedId;
          const saved = !!item.queued?.saved;
          const ids = groupOf(queued, saved);
          const index = ids.indexOf(queuedId);
          const editing = editor.editingId === queuedId;
          const preview = item.text.replace(/\s+/g, " ").trim();
          const images = item.images?.length ?? 0;
          return (
            <li
              key={queuedId}
              data-testid="queued-turn-row"
              data-queued-id={queuedId}
              aria-current={editing ? "true" : undefined}
              draggable
              onDragStart={(event) => {
                if (armedRef.current !== queuedId) {
                  event.preventDefault();
                  return;
                }
                event.dataTransfer.setData("text/plain", preview);
                event.dataTransfer.effectAllowed = "move";
                setDrag({ queuedId, insertIndex: null });
              }}
              onDragEnd={() => {
                armedRef.current = null;
                setDrag(null);
              }}
              onDragOver={(event) => {
                if (!drag) return;
                const dragged = queued.find((m) => m.queued?.queuedId === drag.queuedId);
                // A turn moves only within its own queue.
                if (!dragged || !!dragged.queued?.saved !== saved) return;
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
                const rect = event.currentTarget.getBoundingClientRect();
                const insertIndex = event.clientY < rect.top + rect.height / 2 ? index : index + 1;
                if (drag.insertIndex !== insertIndex) setDrag({ ...drag, insertIndex });
              }}
              onDrop={(event) => {
                if (!drag) return;
                event.preventDefault();
                completeDrag(drag);
              }}
              className={cn(
                "relative flex h-7 items-center gap-1.5 rounded-md pr-1 pl-0.5",
                editing && "bg-surface-2",
                drag?.queuedId === queuedId && "opacity-50",
              )}
            >
              {drag && drag.insertIndex === index && drag.queuedId !== queuedId ? (
                <span aria-hidden className="pointer-events-none absolute inset-x-1 top-0 h-0.5 rounded-sm bg-accent-ember/70" />
              ) : null}
              {drag && index === ids.length - 1 && drag.insertIndex === ids.length ? (
                <span aria-hidden className="pointer-events-none absolute inset-x-1 bottom-0 h-0.5 rounded-sm bg-accent-ember/70" />
              ) : null}
              <button
                type="button"
                aria-label="Reorder queued message (drag, or press the arrow keys)"
                title="Drag to reorder"
                onPointerDown={() => {
                  armedRef.current = queuedId;
                }}
                data-queued-grip
                onBlur={() => {
                  if (refocusRef.current === queuedId) refocusRef.current = null;
                }}
                onKeyDown={(event) => {
                  if (event.key === "ArrowUp" && index > 0) {
                    event.preventDefault();
                    refocusRef.current = queuedId;
                    move(queuedId, ids[index - 1]);
                  } else if (event.key === "ArrowDown" && index < ids.length - 1) {
                    event.preventDefault();
                    refocusRef.current = queuedId;
                    move(queuedId, ids[index + 2] ?? null);
                  }
                }}
                className={cn(ICON_ACTION, "cursor-grab active:cursor-grabbing")}
              >
                <GripVertical className="size-3.5" aria-hidden />
              </button>
              {editing ? <span className="sr-only">Editing queued message: </span> : null}
              {images > 0 ? (
                <span className="inline-flex shrink-0 items-center gap-0.5 text-label text-muted-foreground" title={`${images} image${images === 1 ? "" : "s"}`}>
                  <ImageIcon className="size-3" aria-hidden />
                  {images > 1 ? images : null}
                </span>
              ) : null}
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="min-w-0 flex-1 truncate text-body-sm text-foreground/80">{preview}</span>
                </TooltipTrigger>
                <TooltipContent side="top" className="max-w-96 whitespace-pre-wrap break-words">
                  {item.text}
                </TooltipContent>
              </Tooltip>
              {saved && !editing ? (
                <span
                  className="shrink-0 rounded-sm bg-surface-2 px-1.5 text-label font-medium text-muted-foreground"
                  title="Kept from before Codemux restarted. It waits until you send it."
                >
                  Saved
                </span>
              ) : null}
              {editing ? (
                <button
                  type="button"
                  aria-label="Cancel editing queued message"
                  onClick={() => {
                    editor.cancel();
                    focusComposerNear(rootRef.current);
                  }}
                  className={ACTION}
                >
                  Cancel
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    aria-label="Edit queued message"
                    title="Edit in the composer"
                    onClick={() => {
                      setExpanded(true);
                      editor.begin(item);
                      focusComposerNear(rootRef.current);
                    }}
                    className={ICON_ACTION}
                  >
                    <Pencil className="size-3" aria-hidden />
                  </button>
                  {saved ? (
                    <button
                      type="button"
                      aria-label="Send saved message"
                      title={streaming ? "Queue behind the current task" : "Send now"}
                      onClick={() => sendSaved(item)}
                      className={ACTION}
                    >
                      <ArrowUp className="size-3" aria-hidden />
                      Send
                    </button>
                  ) : (
                    <>
                      {canSteer ? (
                        <button
                          type="button"
                          aria-label="Steer with queued message"
                          title={streaming ? "Guide the current task without stopping tools" : "There is no running task to steer"}
                          disabled={!streaming}
                          onClick={() => onSteer(queuedId)}
                          className={ACTION}
                        >
                          <CornerUpRight className="size-3" aria-hidden />
                          Steer
                        </button>
                      ) : null}
                      <button
                        type="button"
                        aria-label="Interrupt and send queued message"
                        title="Stop current work and send this message"
                        onClick={() => onInterrupt(queuedId)}
                        className={ACTION}
                      >
                        <CornerDownLeft className="size-3" aria-hidden />
                        Interrupt
                      </button>
                    </>
                  )}
                  <button
                    type="button"
                    aria-label="Remove queued message"
                    title="Remove from the queue"
                    onClick={() => remove(item)}
                    className={cn(ICON_ACTION, "hover:bg-destructive/10 hover:text-destructive/80")}
                  >
                    <X className="size-3.5" aria-hidden />
                  </button>
                </>
              )}
            </li>
          );
        })}
      </ol>
    </div>
    </TooltipProvider>
  );
}

// ── Queue reconcile ────────────────────────────────────────────────


/** What the queued bubbles currently say, to tell whether a queue event
 *  arrived meanwhile. */
function queuedSignature(messages: ChatThreadState["messages"]): string {
  return messages
    .filter((m): m is UserMessageItem => m.kind === "user_message" && !!m.queued)
    .map((m) => `${m.queued!.queuedId}:${m.seq}:${m.text}`)
    .join("\n");
}


function imagesOf(row: QueuedTurnView) {
  return row.images.map((image) => ({ src: image.path, mediaType: image.media_type }));
}

/** Match the thread's queued bubbles to the backend's queue: add the
 *  missing ones (a client attaching mid-queue), update text and the saved
 *  flag, drop ones no longer queued, and take its order. Events missed
 *  while detached (an edit or move by another client) are repaired here. A
 *  turn already sent under its nonce is left alone. */
export function reconcileQueuedTurns(
  state: ChatThreadState,
  threadId: string,
  rows: QueuedTurnView[],
): ChatThreadState {
  const keep = new Set(rows.map((row) => row.queued_id));
  let next = state;
  const gone = (m: ChatThreadState["messages"][number]) =>
    m.kind === "user_message" && !!m.queued && !keep.has(m.queued.queuedId);
  if (next.messages.some(gone)) {
    next = { ...next, messages: next.messages.filter((m) => !gone(m)) };
  }
  for (const row of rows) {
    const index = next.messages.findIndex(
      (m) => m.kind === "user_message" && m.queued?.queuedId === row.queued_id,
    );
    if (index >= 0) {
      next = editQueuedTurn(next, {
        type: "queued_turn_edited",
        thread_id: threadId,
        queued_id: row.queued_id,
        text: row.text,
      });
      // A bubble kept from before the backend restarted is saved now, and a
      // bubble built from another client's event has no images yet. An
      // optimistic preview of the same images stays.
      const item = next.messages[index];
      if (item.kind !== "user_message") continue;
      const savedChanged = !!item.queued?.saved !== row.saved;
      const imagesMissing = row.images.length > 0 && (item.images?.length ?? 0) !== row.images.length;
      if (savedChanged || imagesMissing) {
        next = {
          ...next,
          messages: replaceTranscriptItem(next.messages, index, {
            ...item,
            queued: { queuedId: row.queued_id, ...(row.saved ? { saved: true } : {}) },
            ...(imagesMissing ? { images: imagesOf(row) } : {}),
          }),
        };
      }
      continue;
    }
    if (next.messages.some((m) => m.kind === "user_message" && m.clientNonce === row.client_nonce)) {
      continue;
    }
    next = applyEvent(next, {
      type: "turn_queued",
      thread_id: threadId,
      queued_id: row.queued_id,
      client_nonce: row.client_nonce,
      text: row.text,
    });
    const added = next.messages.findIndex(
      (m) => m.kind === "user_message" && m.queued?.queuedId === row.queued_id,
    );
    const item = next.messages[added];
    if (item?.kind !== "user_message") continue;
    next = {
      ...next,
      messages: replaceTranscriptItem(next.messages, added, {
        ...item,
        queued: { queuedId: row.queued_id, ...(row.saved ? { saved: true } : {}) },
        ...(row.images.length > 0 ? { images: imagesOf(row) } : {}),
      }),
    };
  }
  next = reorderQueuedTurns(next, {
    type: "queued_turns_reordered",
    thread_id: threadId,
    queued_ids: rows.map((row) => row.queued_id),
  });
  // Adding goes through the reducer, which would clear a stall notice.
  return next === state || next.stalled === state.stalled ? next : { ...next, stalled: state.stalled };
}

/** After every transcript replay of the thread (hydration, including a
 *  remote reconnect's, and a revert) and whenever saved rows are rehomed
 *  onto it, match its queued bubbles to the backend's queue
 *  (`reconcileQueuedTurns`). Between those, queue events keep them in step. */
export function useSavedQueueRestore(
  threadId: string | null,
  hydrated: boolean,
  hydrationPass: number,
) {
  // The backend rehomed saved rows onto this thread after it was listed.
  const [rehomed, setRehomed] = useState(0);
  useTauriEvent<string>(
    onAgentChatQueuedTurnsChanged,
    (changed) => {
      if (changed === threadId) setRehomed((n) => n + 1);
    },
    [threadId],
  );
  useEffect(() => {
    if (!threadId || !hydrated) return;
    let cancelled = false;
    const queuedNow = () =>
      queuedSignature(useAgentChatStore.getState().threads[threadId]?.messages ?? []);
    void (async () => {
      // A queue event that lands while the list is in flight makes the
      // answer stale (it could bring back a cancelled turn): ask again.
      for (let attempt = 0; attempt < 3 && !cancelled; attempt++) {
        const before = queuedNow();
        const rows = await agentChatListQueuedTurns(threadId);
        if (cancelled) return;
        if (queuedNow() !== before) continue;
        useAgentChatStore.setState((state) => {
          const slice = state.threads[threadId];
          if (!slice) return {};
          const next = reconcileQueuedTurns(slice, threadId, rows);
          if (next === slice) return {};
          return { threads: { ...state.threads, [threadId]: { ...slice, ...next } } };
        });
        return;
      }
    })().catch((error) => console.warn("[agent-chat] saved queue unavailable:", error));
    return () => {
      cancelled = true;
    };
  }, [threadId, hydrated, hydrationPass, rehomed]);
}
