/**
 * Chat fork UI: the "Fork" action under an assistant reply, the fork rows in
 * the chat-history menu, and the "Forked from" marker in a fork.
 *
 * The transcript offers a fork on the last reply of every finished turn.
 * Those points are computed once per transcript snapshot by
 * {@link ChatForkProvider} and read per reply through a subscription, so a
 * streaming token re-renders only a reply whose own point changed — the
 * transcript rows don't have to thread fork props through.
 */
import { createContext, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { ChevronDown, FolderGit2, Split } from "lucide-react";

import {
  canForkIntoWorktree,
  computeForkPoints,
  forkChat,
  workspaceById,
  chatForkSupported,
  type ForkBefore,
  type ForkTarget,
} from "@/lib/agent-chat/chat-fork";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  agentChatForkOrigin,
  agentChatOpenSearchResult,
  type AgentChatTurnCheckpointRecord,
} from "@/tauri/commands";
import { onAgentChatSessionsMerged } from "@/tauri/events";
import type { AgentChatProviderKind, ChatForkOrigin } from "@/tauri/types";

import { MESSAGE_ACTION_CLASS } from "./message-action";

// ── Fork points ─────────────────────────────────────────────────────────

interface ForkPointStore {
  get: (itemId: string) => ForkBefore | undefined;
  subscribe: (listener: () => void) => () => void;
  set: (points: Map<string, ForkBefore>) => void;
}

function createForkPointStore(): ForkPointStore {
  let points = new Map<string, ForkBefore>();
  const listeners = new Set<() => void>();
  return {
    get: (itemId) => points.get(itemId),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    set: (next) => {
      points = next;
      for (const listener of listeners) listener();
    },
  };
}

interface ChatForkContextValue {
  store: ForkPointStore;
  canUseWorktree: boolean;
  fork: (before: ForkBefore, target: ForkTarget) => void;
  /** Where this chat was forked from; `null` when it isn't a fork. */
  origin: ChatForkOrigin | null;
}

/** Exported so `ChatTranscript` can relay it into the transcript cache's
 *  portal, whose React ancestry skips the pane. */
export const ChatForkContext = createContext<ChatForkContextValue | null>(null);

/** Supplies the transcript's fork points and this chat's fork origin. */
export function ChatForkProvider({
  threadId,
  provider,
  workspaceId,
  messages,
  checkpointByNonce,
  busy,
  enabled,
  children,
}: {
  threadId: string | null;
  provider: AgentChatProviderKind | null | undefined;
  workspaceId: string | null | undefined;
  messages: readonly ChatViewItem[];
  checkpointByNonce?: ReadonlyMap<string, AgentChatTurnCheckpointRecord>;
  busy: boolean;
  /** False for a read-only view of the chat. Paired browsers never fork. */
  enabled: boolean;
  children: React.ReactNode;
}) {
  const [store] = useState(createForkPointStore);
  useEffect(() => {
    store.set(
      enabled && threadId && workspaceId && !isRemoteClient()
        ? computeForkPoints(messages, provider, checkpointByNonce, busy)
        : new Map(),
    );
  }, [store, enabled, threadId, workspaceId, messages, provider, checkpointByNonce, busy]);
  const latest = useRef({ threadId, workspaceId });
  latest.current = { threadId, workspaceId };
  const canUseWorktree = useWorkspaceCanFork(workspaceId);
  const origin = useChatForkOrigin(threadId);
  const value = useMemo<ChatForkContextValue>(
    () => ({
      store,
      canUseWorktree,
      origin,
      fork: (before, target) => {
        const { threadId: source, workspaceId: workspace } = latest.current;
        if (!source || !workspace) return;
        void forkChat({ sourceThreadId: source, workspaceId: workspace, before, target });
      },
    }),
    [store, canUseWorktree, origin],
  );
  return <ChatForkContext.Provider value={value}>{children}</ChatForkContext.Provider>;
}

function useWorkspaceCanFork(workspaceId: string | null | undefined): boolean {
  // Re-read on mount only: a workspace doesn't stop being a git checkout.
  return useMemo(() => canForkIntoWorktree(workspaceById(workspaceId)), [workspaceId]);
}

const NO_POINT = () => () => {};

/** The fork point this reply offers, or `undefined` for none. */
function useForkPoint(itemId: string): ForkBefore | undefined {
  const context = useContext(ChatForkContext);
  return useSyncExternalStore(
    context?.store.subscribe ?? NO_POINT,
    () => context?.store.get(itemId),
  );
}

// ── Reply action ────────────────────────────────────────────────────────

/** "Fork" in a settled reply's footer strip, beside Copy. */
export function ChatForkButton({ itemId }: { itemId: string }) {
  const context = useContext(ChatForkContext);
  const before = useForkPoint(itemId);
  const [open, setOpen] = useState(false);
  if (!context || before === undefined) return null;
  const label = before === null ? "Fork chat from here" : "Fork chat from this turn";
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={label}
          title={label}
          data-testid="message-fork-button"
          className={cn(MESSAGE_ACTION_CLASS, open && "pointer-events-auto opacity-100")}
        >
          <Split className="size-3 rotate-90" aria-hidden />
          <span className="pointer-coarse:sr-only">Fork</span>
          <ChevronDown className="size-2.5 opacity-60" aria-hidden />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56" data-testid="message-fork-menu">
        <ForkMenuRows before={before} canUseWorktree={context.canUseWorktree} onFork={context.fork} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ForkMenuRows({
  before,
  canUseWorktree,
  onFork,
}: {
  before: ForkBefore;
  canUseWorktree: boolean;
  onFork: (before: ForkBefore, target: ForkTarget) => void;
}) {
  return (
    <>
      <DropdownMenuItem onSelect={() => onFork(before, "here")} data-testid="fork-here">
        <Split className="size-3.5 rotate-90" />
        {before === null ? "Fork chat" : "Fork from here"}
      </DropdownMenuItem>
      <DropdownMenuItem
        disabled={!canUseWorktree}
        onSelect={() => onFork(before, "worktree")}
        data-testid="fork-worktree"
        title={canUseWorktree ? undefined : "Needs a local git workspace"}
      >
        <FolderGit2 className="size-3.5" />
        Fork into new worktree
      </DropdownMenuItem>
    </>
  );
}

// ── Chat-history menu ───────────────────────────────────────────────────

/** "Fork chat" rows for the chat-history dropdown (its `footerItem`). */
export function ChatForkMenuItems({
  threadId,
  provider,
  workspaceId,
}: {
  threadId: string | null;
  provider: AgentChatProviderKind | null | undefined;
  workspaceId: string | null | undefined;
}) {
  const canUseWorktree = useWorkspaceCanFork(workspaceId);
  // Forking runs local provider processes; a paired browser can't reach it.
  if (!threadId || !workspaceId || !chatForkSupported(provider) || isRemoteClient()) return null;
  return (
    <ForkMenuRows
      before={null}
      canUseWorktree={canUseWorktree}
      onFork={(before, target) =>
        void forkChat({ sourceThreadId: threadId, workspaceId, before, target })
      }
    />
  );
}

// ── "Forked from" marker ────────────────────────────────────────────────

/** Focus the source chat's tab, or reopen it from history. */
async function openOriginal(threadId: string): Promise<void> {
  try {
    const opened = await agentChatOpenSearchResult(threadId);
    if (isRemoteClient()) await activateWorkspaceInteraction(opened.workspace_id);
  } catch (error) {
    toast.failure("Couldn't open the original chat", error);
  }
}

/** This chat's fork origin, or `null` when it isn't a fork. Read again when
 *  a reopen from history merges into this chat (the pane is bound to its
 *  new thread before the lineage moves there) or merges its source away. */
export function useChatForkOrigin(threadId: string | null): ChatForkOrigin | null {
  const [origin, setOrigin] = useState<{ threadId: string; origin: ChatForkOrigin | null } | null>(null);
  const [merges, setMerges] = useState(0);
  // `undefined` until this chat's first read lands: a merge meanwhile may
  // have moved its source, so it reads again.
  const source = useRef<string | null | undefined>(undefined);
  source.current = origin?.threadId === threadId ? (origin.origin?.source_thread_id ?? null) : undefined;
  useEffect(() => {
    if (!threadId) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    onAgentChatSessionsMerged((payload) => {
      const ours = payload.survivor === threadId;
      const known = source.current;
      const ourSource = known === undefined || (known !== null && payload.merged.includes(known));
      if (ours || ourSource) setMerges((count) => count + 1);
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [threadId]);
  useEffect(() => {
    if (!threadId) return;
    let cancelled = false;
    agentChatForkOrigin(threadId)
      .then((value) => {
        if (!cancelled) setOrigin({ threadId, origin: value });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [threadId, merges]);
  return origin?.threadId === threadId ? origin.origin : null;
}

/** Hairline divider at the end of a fork's inherited transcript, shown
 *  until the fork's own first turn. Links back to the source chat. */
export function ChatForkMarker({ messages }: { messages: readonly ChatViewItem[] }) {
  const origin = useContext(ChatForkContext)?.origin;
  if (!origin) return null;
  const boundary = origin.boundary_message_id ?? 0;
  const ownTurn = messages.some(
    (item) => item.kind === "user_message" && (item.source_event_id == null || item.source_event_id > boundary),
  );
  if (ownTurn) return null;
  const title = origin.source_title?.trim() || "a chat";
  return (
    <div
      data-testid="chat-fork-marker"
      className="mt-[13px] flex items-center gap-3 text-muted-foreground/70"
    >
      <span className="h-px flex-1 bg-border/60" />
      <span className="inline-flex min-w-0 items-center gap-1.5 font-mono text-label font-medium tracking-wide">
        <Split className="size-3 shrink-0 rotate-90" aria-hidden />
        <span className="truncate">Forked from {title}</span>
        {origin.source_exists ? (
          <button
            type="button"
            className="shrink-0 rounded-sm text-foreground/80 underline-offset-2 outline-none hover:underline focus-visible:ring-1 focus-visible:ring-ring"
            onClick={() => void openOriginal(origin.source_thread_id)}
          >
            Open original
          </button>
        ) : null}
      </span>
      <span className="h-px flex-1 bg-border/60" />
    </div>
  );
}
