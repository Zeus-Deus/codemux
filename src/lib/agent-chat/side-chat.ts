/**
 * `/btw` side chats — ask a quick question that uses the current chat's
 * context without adding anything to it.
 *
 * The backend forks the chat's provider session (Claude: `forkSession` with
 * persistence off; Codex: an ephemeral `thread/fork`) into its own read-only
 * process. The answer streams into the right panel's Side chat pane; the
 * main transcript, draft and running turn are never touched, and nothing is
 * saved. One side chat per main thread: asking a new `/btw` while one is
 * open replaces it.
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import { randomUUID } from "@/lib/uuid";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { sessionProviderLabel } from "./session-mentions";

export const SIDE_CHAT_EVENT = "agent-chat-side-chat";

/** `/btw <question>` → the question, `""` for a bare `/btw`, or null. */
export function parseBtwCommand(text: string): { question: string } | null {
  const match = /^\/btw(?:\s+([\s\S]*))?$/i.exec(text.trim());
  if (!match) return null;
  return { question: (match[1] ?? "").trim() };
}

/** Providers whose sessions can be forked into a side chat. */
export function sideChatSupported(provider: AgentChatProviderKind | null | undefined): boolean {
  return provider === "claude" || provider === "codex";
}

export function sideChatUnavailableMessage(provider: AgentChatProviderKind | null | undefined): string {
  const label = !provider || provider === "acp" ? "this agent" : sessionProviderLabel(provider);
  return `Side chat isn't available for ${label} yet.`;
}

export interface SideChatTurn {
  question: string;
  answer: string;
  status: "streaming" | "done" | "error";
  error?: string;
}

/** The newest turn (ES2020 target: no `Array.prototype.at`). */
export function lastSideChatTurn(chat: Pick<SideChatState, "turns"> | null | undefined): SideChatTurn | undefined {
  return chat ? chat.turns[chat.turns.length - 1] : undefined;
}

export interface SideChatState {
  sideId: string;
  /** Main thread the side chat forked. */
  threadId: string;
  workspaceId: string;
  provider: AgentChatProviderKind;
  /** Model id the fork answers with: the main chat's, seeded from its
   *  thread slice and replaced by what the backend reports it started.
   *  `null` is the provider's default model. */
  model: string | null;
  /** Reasoning effort carried over from the main chat; `null` is the
   *  model's default. */
  effort: string | null;
  turns: SideChatTurn[];
  /** The fork's process exited (closed, crashed or ended). */
  ended: boolean;
}

/** What `agent_chat_side_chat_start` reports the fork runs with. */
export interface SideChatStarted {
  model: string | null;
  effort: string | null;
}

type SideChatUpdate =
  | { kind: "delta"; text: string }
  | { kind: "turn_completed"; text: string | null }
  | { kind: "failed"; message: string }
  | { kind: "closed" };

export type SideChatEvent = { side_id: string } & SideChatUpdate;

interface SideChatStore {
  /** Keyed by side id. */
  chats: Record<string, SideChatState>;
  /** The open side chat for each workspace's deck (one at a time). */
  activeByWorkspace: Record<string, string>;
  open: (chat: SideChatState) => void;
  /** Record the model/effort the backend says the fork started with. */
  started: (sideId: string, info: SideChatStarted) => void;
  apply: (event: SideChatEvent) => void;
  ask: (sideId: string, question: string) => void;
  fail: (sideId: string, message: string) => void;
  remove: (sideId: string) => void;
}

function updateLastTurn(
  chat: SideChatState,
  fn: (turn: SideChatTurn) => SideChatTurn,
): SideChatState {
  const last = lastSideChatTurn(chat);
  if (!last) return chat;
  return { ...chat, turns: [...chat.turns.slice(0, -1), fn(last)] };
}

export const useSideChatStore = create<SideChatStore>((set) => ({
  chats: {},
  activeByWorkspace: {},
  open: (chat) =>
    set((s) => ({
      chats: { ...s.chats, [chat.sideId]: chat },
      activeByWorkspace: { ...s.activeByWorkspace, [chat.workspaceId]: chat.sideId },
    })),
  started: (sideId, info) =>
    set((s) => {
      const chat = s.chats[sideId];
      if (!chat) return s;
      // `null` from the backend means "provider default", which the seed
      // (the thread's own picker value) names more precisely.
      const model = info.model ?? chat.model;
      const effort = info.effort ?? chat.effort;
      if (model === chat.model && effort === chat.effort) return s;
      return { chats: { ...s.chats, [sideId]: { ...chat, model, effort } } };
    }),
  apply: (event) =>
    set((s) => {
      const chat = s.chats[event.side_id];
      if (!chat) return s;
      let next = chat;
      switch (event.kind) {
        case "delta":
          next = updateLastTurn(chat, (turn) =>
            turn.status === "streaming" ? { ...turn, answer: turn.answer + event.text } : turn,
          );
          break;
        case "turn_completed":
          next = updateLastTurn(chat, (turn) =>
            turn.status === "streaming"
              ? { ...turn, status: "done", answer: turn.answer || (event.text ?? "") }
              : turn,
          );
          break;
        case "failed":
          next = updateLastTurn(chat, (turn) =>
            turn.status === "streaming" ? { ...turn, status: "error", error: event.message } : turn,
          );
          break;
        case "closed":
          next = updateLastTurn({ ...chat, ended: true }, (turn) =>
            turn.status === "streaming"
              ? { ...turn, status: "error", error: "The side chat ended before it answered." }
              : turn,
          );
          break;
      }
      return next === chat ? s : { chats: { ...s.chats, [chat.sideId]: next } };
    }),
  ask: (sideId, question) =>
    set((s) => {
      const chat = s.chats[sideId];
      if (!chat) return s;
      return {
        chats: {
          ...s.chats,
          [sideId]: { ...chat, turns: [...chat.turns, { question, answer: "", status: "streaming" }] },
        },
      };
    }),
  fail: (sideId, message) =>
    set((s) => {
      const chat = s.chats[sideId];
      if (!chat) return s;
      const next = updateLastTurn(chat, (turn) => ({ ...turn, status: "error", error: message }));
      return { chats: { ...s.chats, [sideId]: next } };
    }),
  remove: (sideId) =>
    set((s) => {
      const chat = s.chats[sideId];
      if (!chat) return s;
      const chats = { ...s.chats };
      delete chats[sideId];
      const activeByWorkspace = { ...s.activeByWorkspace };
      if (activeByWorkspace[chat.workspaceId] === sideId) delete activeByWorkspace[chat.workspaceId];
      return { chats, activeByWorkspace };
    }),
}));

/** The side chat shown in `workspaceId`'s deck, or null. */
export function selectWorkspaceSideChat(workspaceId: string) {
  return (s: SideChatStore): SideChatState | null => {
    const id = s.activeByWorkspace[workspaceId];
    return id ? (s.chats[id] ?? null) : null;
  };
}

// ── IPC ─────────────────────────────────────────────────────────────────

let listening: Promise<UnlistenFn> | null = null;

/** Subscribe once, app-wide. Events for unknown side ids are ignored. */
export function ensureSideChatListener(): Promise<UnlistenFn> {
  listening ??= listen<SideChatEvent>(SIDE_CHAT_EVENT, (event) =>
    useSideChatStore.getState().apply(event.payload),
  );
  return listening;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Close a side chat: reap its fork and drop it from the deck. */
export async function closeSideChat(sideId: string): Promise<void> {
  useSideChatStore.getState().remove(sideId);
  await invoke("agent_chat_side_chat_close", { sideId }).catch(() => {});
}

/**
 * Start a side chat for `threadId`, replacing any open one in this
 * workspace. Resolves once the fork is running; failures land in the pane.
 */
export async function startSideChat(args: {
  threadId: string;
  workspaceId: string;
  provider: AgentChatProviderKind;
  question: string;
}): Promise<string> {
  const store = useSideChatStore.getState();
  const previous = store.activeByWorkspace[args.workspaceId];
  if (previous) void closeSideChat(previous);
  const sideId = randomUUID();
  // The thread's picker values, so the pane can name the model before the
  // fork is up; the backend's report (read from the same persisted row)
  // replaces them once it is.
  const thread = useAgentChatStore.getState().threads[args.threadId];
  store.open({
    sideId,
    threadId: args.threadId,
    workspaceId: args.workspaceId,
    provider: args.provider,
    model: thread?.model ?? null,
    effort: thread?.effort ?? null,
    turns: [{ question: args.question, answer: "", status: "streaming" }],
    ended: false,
  });
  // Closing or replacing the pane drops the chat from the store; a start
  // still in flight then must not spawn, or must reap what it spawned.
  const isOpen = () => sideId in useSideChatStore.getState().chats;
  try {
    // Subscribe before the fork starts so no early delta is missed.
    await ensureSideChatListener();
    if (!isOpen()) return sideId;
    const started = await invoke<SideChatStarted | null | undefined>("agent_chat_side_chat_start", {
      sideId,
      threadId: args.threadId,
      text: args.question,
    });
    if (!isOpen()) {
      // The close raced ahead of the backend's claim and found nothing.
      await invoke("agent_chat_side_chat_close", { sideId }).catch(() => {});
      return sideId;
    }
    if (started && typeof started === "object") {
      useSideChatStore.getState().started(sideId, {
        model: started.model ?? null,
        effort: started.effort ?? null,
      });
    }
  } catch (error) {
    if (!isOpen()) return sideId;
    useSideChatStore.getState().fail(sideId, errorText(error));
    useSideChatStore.getState().apply({ side_id: sideId, kind: "closed" });
  }
  return sideId;
}

/** Ask a follow-up on the same fork. */
export async function sendSideChat(sideId: string, question: string): Promise<void> {
  const store = useSideChatStore.getState();
  const chat = store.chats[sideId];
  if (!chat || chat.ended) return;
  if (lastSideChatTurn(chat)?.status === "streaming") return;
  store.ask(sideId, question);
  try {
    await invoke("agent_chat_side_chat_send", { sideId, text: question });
  } catch (error) {
    useSideChatStore.getState().fail(sideId, errorText(error));
  }
}
