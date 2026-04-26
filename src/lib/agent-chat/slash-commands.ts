import {
  Bug,
  ListTodo,
  MessageCircleQuestion,
  type LucideIcon,
} from "lucide-react";

import type { ChatMode } from "@/stores/agent-chat-store";
import type { ActivePillMode } from "@/components/chat/pickers/ModePill";

/**
 * One row in the slash-command popup. The popup component renders these
 * generically — no hard-coded knowledge of modes — so Step 7 (skills)
 * can append its own items without touching the popup.
 */
export interface SlashCommandItem {
  /** Stable id, e.g. `"mode:plan"` or `"skill:codemux-ui"`. */
  id: string;
  /** Display label, e.g. `"Plan"`. */
  label: string;
  /** Optional one-line description shown muted next to the label. */
  description?: string;
  /** The literal command string (`"/plan"`) — used for filtering and
   *  shown right-aligned as a hint. */
  command: string;
  /** Lucide icon. Optional; some skills may not have an icon. */
  icon?: LucideIcon;
  /** What happens when the user picks this item. The composer wires
   *  this to the appropriate handler (mode activation, skill invoke,
   *  …). */
  onSelect: () => void;
  /** Section heading. Stage 8 ships only `"MODES"`; Step 7 will add
   *  `"SKILLS"`. Free-form so future categories can slot in. */
  group: string;
  /** Step 8 Stage 3 — when true, the row renders muted and is
   *  non-selectable. Used for "coming soon" entries (Issue / PR /
   *  Image kinds before their respective stages light them up).
   *  Defaults to false; back-compatible with all existing call sites. */
  disabled?: boolean;
}

export interface SlashAnchor {
  /** Index of the trigger character (`/` or `@`) in the value. */
  start: number;
  /** Substring between the trigger and the cursor. Empty when the
   *  user has just typed the trigger. */
  query: string;
}

/** Mention anchors share the SlashAnchor shape — both record the
 *  trigger position and the query suffix. Aliased so call-sites read
 *  cleanly when handling `@` mentions specifically. */
export type MentionAnchor = SlashAnchor;

/**
 * Walk back from the cursor to find the trigger character that opens
 * a command. Generalised over `/` (slash commands) and `@` (mentions)
 * so Step 8 can reuse the same primitive without forking detection.
 *
 * Returns `null` when the cursor isn't currently inside a trigger
 * context. A trigger is "in command" only when:
 *   - The trigger itself is at start-of-text or preceded by whitespace
 *     (newlines included), AND
 *   - There is no whitespace between the trigger and the cursor.
 */
export function findTriggerAtCursor(
  value: string,
  cursor: number,
  trigger: "/" | "@",
): SlashAnchor | null {
  if (cursor < 0 || cursor > value.length) return null;
  for (let i = cursor - 1; i >= 0; i--) {
    const ch = value[i];
    if (ch === trigger) {
      const before = i === 0 ? "" : value[i - 1];
      if (before === "" || /\s/.test(before)) {
        return { start: i, query: value.slice(i + 1, cursor) };
      }
      return null;
    }
    if (ch && /\s/.test(ch)) return null;
  }
  return null;
}

/**
 * Walk back from the cursor to find the slash that opens a command.
 *
 * Examples:
 *   `""`              → null
 *   `"/"`             → { start: 0, query: "" }
 *   `"/pl"` cursor=3  → { start: 0, query: "pl" }
 *   `"hello /pl"` c=9 → { start: 6, query: "pl" }
 *   `"hello/pl"` c=8  → null (slash inside a word)
 *   `"/pl world"` c=9 → null (cursor past the space)
 */
export function findSlashAtCursor(
  value: string,
  cursor: number,
): SlashAnchor | null {
  return findTriggerAtCursor(value, cursor, "/");
}

/**
 * Walk back from the cursor to find the `@` that opens a mention
 * (Step 8). Same rules as `findSlashAtCursor` — the `@` must be at
 * start-of-text or after whitespace, and there can be no whitespace
 * between the `@` and the cursor.
 *
 * Examples:
 *   `"@"`               → { start: 0, query: "" }
 *   `"@composer"` c=9   → { start: 0, query: "composer" }
 *   `"hi @comp"` c=8    → { start: 3, query: "comp" }
 *   `"a@b"` c=3         → null (`@` inside a word)
 *   `"/foo @bar"` c=9   → { start: 5, query: "bar" } (the `@` wins
 *                          when the cursor is past the space + at
 *                          the bar)
 */
export function findMentionAtCursor(
  value: string,
  cursor: number,
): MentionAnchor | null {
  return findTriggerAtCursor(value, cursor, "@");
}

/**
 * Filter slash items by query. Matches when the typed string is a
 * prefix of the literal command (`/pl` → `/plan`) OR when it appears
 * anywhere in the label (`pla` → `Plan`). Case-insensitive.
 */
export function filterSlashItems(
  items: SlashCommandItem[],
  query: string,
): SlashCommandItem[] {
  if (!query) return items;
  const q = query.toLowerCase();
  return items.filter(
    (item) =>
      item.command.toLowerCase().startsWith(`/${q}`) ||
      item.label.toLowerCase().includes(q),
  );
}

interface BuildModeCommandsArgs {
  /** Currently-active mode. The matching command is omitted from the
   *  returned list so the user can't double-activate. */
  activeMode: ChatMode;
  /** Called when the user picks one of the mode items. */
  onActivate: (mode: ActivePillMode) => void;
}

/**
 * Build the `MODES` group of slash items.
 *
 * The active mode is filtered out so the popup never shows a no-op
 * choice. When `mode === "default"`, all three modes are shown.
 */
export function buildModeCommands({
  activeMode,
  onActivate,
}: BuildModeCommandsArgs): SlashCommandItem[] {
  const all: SlashCommandItem[] = [
    {
      id: "mode:plan",
      label: "Plan",
      description: "Plan and design before coding",
      command: "/plan",
      icon: ListTodo,
      group: "MODES",
      onSelect: () => onActivate("plan"),
    },
    {
      id: "mode:ask",
      label: "Ask",
      description: "Read-only conversational mode",
      command: "/ask",
      icon: MessageCircleQuestion,
      group: "MODES",
      onSelect: () => onActivate("ask"),
    },
    {
      id: "mode:debug",
      label: "Debug",
      description: "Add diagnostic logs to find bugs",
      command: "/debug",
      icon: Bug,
      group: "MODES",
      onSelect: () => onActivate("debug"),
    },
  ];
  return all.filter((item) => item.id !== `mode:${activeMode}`);
}

/** Cycle order for Shift+Tab: default → plan → ask → debug → default. */
export const MODE_CYCLE_ORDER: ChatMode[] = [
  "default",
  "plan",
  "ask",
  "debug",
];

/**
 * Return the next mode in the Shift+Tab cycle. Pure function — caller
 * dispatches to `onModeActivate` / `onModeRemove` based on whether the
 * next mode is `"default"`.
 */
export function nextModeInCycle(current: ChatMode): ChatMode {
  const idx = MODE_CYCLE_ORDER.indexOf(current);
  // Defensive: an unknown mode falls back to `default → plan` so the
  // user always advances rather than getting stuck.
  if (idx === -1) return "plan";
  return MODE_CYCLE_ORDER[(idx + 1) % MODE_CYCLE_ORDER.length] ?? "default";
}

/**
 * Group items by their `group` field while preserving insertion order
 * within each group AND across groups (groups appear in the order
 * their first item appeared).
 */
export function groupSlashItems(
  items: SlashCommandItem[],
): Array<{ group: string; items: SlashCommandItem[] }> {
  const order: string[] = [];
  const map = new Map<string, SlashCommandItem[]>();
  for (const item of items) {
    if (!map.has(item.group)) {
      order.push(item.group);
      map.set(item.group, []);
    }
    map.get(item.group)!.push(item);
  }
  return order.map((group) => ({ group, items: map.get(group)! }));
}
