import type { ChatThreadState, ChatViewItem, UserMessageItem } from "./types";

/**
 * A goal the user set in this thread with `/goal <text>`.
 *
 * `/goal` is a provider command: Codemux forwards the text and the runtime
 * owns whatever loop it drives. The frontend only records that a goal was
 * set, so the composer strip can keep it in view. Codemux cannot see turn
 * counts, caps or completion, so nothing here pretends to.
 *
 * Durability comes from the transcript. The recording user turn is a
 * persisted row, and hydrate replays it through the reducer, so the goal
 * survives an app restart the same way the task snapshot does.
 */
export interface GoalSnapshot {
  /** Everything after `/goal`, trimmed. */
  text: string;
  /** Epoch ms of the recording user turn. */
  setAt: number;
  /** Transcript id of the user turn that set the goal. */
  sourceMessageId: string;
}

/**
 * `standing`: a goal exists. It says nothing about progress.
 * `interrupted`: the last run was cut off mid-turn (app quit, crash, or the
 * provider process died). A goal whose turn simply finished stays
 * `standing`, because a finished goal and an idle one look the same from
 * here, and offering Resume on finished work is worse than not offering it.
 */
export type GoalStatus = "standing" | "interrupted";

export interface ThreadGoal extends GoalSnapshot {
  status: GoalStatus;
}

export type GoalCommand =
  | { kind: "set"; text: string }
  | { kind: "clear" }
  /** `/goal`, `/goal status`, `/goal pause`, `/goal resume`: the provider
   *  acts on these, but they don't change which goal is standing. */
  | { kind: "control" };

type GoalFields = Pick<ChatThreadState, "goal" | "goalHistory">;

/** `/goal` subcommands that act on the goal without replacing it. */
export const GOAL_CONTROL_WORDS = ["status", "pause", "resume"] as const;
/** The `/goal` subcommand that drops the standing goal. */
export const GOAL_CLEAR_WORD = "clear";
/** Every word `/goal` reads as a subcommand rather than as goal text. */
export type GoalSubcommandWord =
  | (typeof GOAL_CONTROL_WORDS)[number]
  | typeof GOAL_CLEAR_WORD;

const GOAL_COMMAND =/^\/goal(?:\s+([\s\S]*))?$/i;
const CONTROL_ARGS = new Set<string>(["", ...GOAL_CONTROL_WORDS]);

/** Parse a user turn as a `/goal` command, or `null` when it isn't one. */
export function parseGoalCommand(text: string): GoalCommand | null {
  const match = GOAL_COMMAND.exec(text.trim());
  if (!match) return null;
  const arg = (match[1] ?? "").trim();
  const word = arg.toLowerCase();
  if (word === GOAL_CLEAR_WORD) return { kind: "clear" };
  if (CONTROL_ARGS.has(word)) return { kind: "control" };
  return { kind: "set", text: arg };
}

/**
 * Fold one new user turn into the thread's goal. Idempotent per item: a
 * queued turn that later promotes carries the same id, so it never records
 * twice or pushes a duplicate into history.
 */
export function applyGoalFromUserMessage<S extends GoalFields>(
  state: S,
  item: UserMessageItem,
  now: number,
): S {
  const command = parseGoalCommand(item.text);
  if (!command || command.kind === "control") return state;
  if (state.goal?.sourceMessageId === item.id) return state;
  const goalHistory = state.goal
    ? [...state.goalHistory, state.goal]
    : state.goalHistory;
  if (command.kind === "clear") {
    if (!state.goal) return state;
    return { ...state, goal: null, goalHistory };
  }
  return {
    ...state,
    goal: {
      text: command.text,
      setAt: item.created_at ?? now,
      sourceMessageId: item.id,
    },
    goalHistory,
  };
}

/**
 * Rebuild goal state from the transcript alone. Only used to undo a
 * rolled-back send, where the removed turn may have been the one that set
 * or cleared the goal.
 */
export function goalStateFromMessages(
  messages: readonly ChatViewItem[],
): GoalFields {
  let state: GoalFields = { goal: null, goalHistory: [] };
  for (const m of messages) {
    if (m.kind !== "user_message" || m.queued) continue;
    state = applyGoalFromUserMessage(state, m, m.created_at ?? 0);
  }
  return state;
}

/**
 * Keep `sourceMessageId` pointing at a live row after hydrate re-keys
 * replayed items onto the ids already on screen (`adoptItemIds` keeps array
 * positions, so the index carries over).
 */
export function remapGoalSource(
  goal: GoalSnapshot | null,
  replayed: readonly ChatViewItem[],
  adopted: readonly ChatViewItem[],
): GoalSnapshot | null {
  if (!goal || replayed === adopted) return goal;
  const index = replayed.findIndex((m) => m.id === goal.sourceMessageId);
  const id = index >= 0 ? adopted[index]?.id : undefined;
  return id && id !== goal.sourceMessageId
    ? { ...goal, sourceMessageId: id }
    : goal;
}

/** The goal as the strip shows it. `interrupted` is the thread's own
 *  mid-flight-cutoff flag, which hydrate derives from persisted history. */
export function resolveThreadGoal(
  goal: GoalSnapshot | null,
  interrupted: boolean,
): ThreadGoal | null {
  if (!goal) return null;
  return { ...goal, status: interrupted ? "interrupted" : "standing" };
}

/** Compact age for the strip: `<1m`, `4m`, `3h`, `2d`. */
export function formatGoalAge(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/** Wall-clock time for the opened row's meta line, e.g. `12:21`. */
export function formatClockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
}

/** Where an interrupted run stopped, as far as the transcript can say. */
export interface GoalLastActivity {
  /** Timestamp of the newest row that carries one. */
  at: number | null;
  /** The last assistant message: the Jump target and the source of the
   *  "stopped after" snippet. */
  itemId: string | null;
  turnId: string | null;
  snippet: string | null;
}

const SNIPPET_MAX = 48;

/**
 * Read the interrupted run's last activity off the transcript. Only called
 * while a goal is interrupted, so the backwards scan never runs per token.
 */
export function goalLastActivity(
  messages: readonly ChatViewItem[],
): GoalLastActivity | null {
  let at: number | null = null;
  let assistant: Extract<ChatViewItem, { kind: "assistant_message" }> | null =
    null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!;
    if (at === null) at = itemTime(m);
    if (!assistant && m.kind === "assistant_message" && m.text.trim()) {
      assistant = m;
    }
    if (at !== null && assistant) break;
  }
  if (at === null && !assistant) return null;
  return {
    at,
    itemId: assistant?.id ?? null,
    turnId: assistant?.turn_id ?? null,
    snippet: assistant ? activitySnippet(assistant.text) : null,
  };
}

function itemTime(m: ChatViewItem): number | null {
  const stamped = m as {
    completed_at?: number;
    created_at?: number;
    started_at?: number;
  };
  return stamped.completed_at ?? stamped.created_at ?? stamped.started_at ?? null;
}

/** First sentence of the reply, markdown punctuation dropped, kept short. */
function activitySnippet(text: string): string | null {
  const plain = text.replace(/[`*_#>]/g, "").replace(/\s+/g, " ").trim();
  const sentence = (plain.split(/(?<=[.!?])\s/)[0] ?? "").replace(/[.!?]$/, "");
  if (!sentence) return null;
  return sentence.length > SNIPPET_MAX
    ? `${sentence.slice(0, SNIPPET_MAX - 1).trimEnd()}…`
    : sentence;
}
