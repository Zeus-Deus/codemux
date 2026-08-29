import type * as React from "react";

import {
  Bug,
  Cpu,
  History,
  ListTodo,
  MessageCircleQuestion,
  RotateCcw,
  SquareSlash,
  Workflow,
  type LucideIcon,
} from "lucide-react";

import type { ChatMode } from "@/stores/agent-chat-store";
import type { ProviderSlashCommand } from "@/tauri/commands";
import type { ActivePillMode } from "@/components/chat/pickers/ModePill";

/**
 * Tone applied to a command-menu row's icon chip (redesigned `+`
 * command menu). Maps to the app's token utilities in
 * `ComposerCommandMenu` — never a raw colour. `muted` is the neutral
 * default so rows that don't opt in stay quiet.
 */
export type CommandTone =
  | "sky"
  | "amber"
  | "violet"
  | "green"
  | "red"
  | "muted"
  | "ember";

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
  /** Tailwind classes applied to the icon, overriding the popup's
   *  default `text-muted-foreground`. Used to tint state-bearing
   *  icons (e.g. green for open issues, muted for closed). Optional;
   *  rows that don't supply this fall back to the popup default. */
  iconClassName?: string;
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
  /** Step 9 Stage 4 — opt-in trailing element rendered right-aligned
   *  in the row, e.g. an inline Switch for the MCP servers submenu.
   *  When present, the row's onSelect is suppressed because the
   *  trailing control owns the click target. */
  rightAdornment?: React.ReactNode;
  /** Opt into a roomier two-line row with the description below the
   *  label. Useful for entity results (such as conversations) whose
   *  names and previews need independent truncation. */
  stacked?: boolean;
  /** Redesigned `+` command menu — tone of the row's 24px icon chip.
   *  Optional; the menu falls back to `muted` when unset. Ignored by
   *  the legacy `SlashCommandPopup` (slash / mention surfaces). */
  tone?: CommandTone;
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

/** Step 8 Stage 4 — category prefix parsed off a `@` mention query.
 *  `file` is the default; `issue` and `pr` route to GitHub-backed
 *  popups (Stage 4 ships `issue`, Stage 5 wires `pr`). `folder` is
 *  reserved so the future `@folder:` autocomplete can use the same
 *  routing without a second parser. */
export type MentionCategory = "file" | "folder" | "issue" | "pr" | "session";

export interface ParsedMentionQuery {
  category: MentionCategory;
  /** Substring after the `<category>:` prefix. Equals the raw query
   *  when no prefix was supplied (default file behaviour). */
  filter: string;
}

/** Parse a mention query into a `(category, filter)` pair. Recognised
 *  prefixes: `file:`, `folder:`, `issue:`, `pr:`, `session:`. Anything else is
 *  treated as a bare file query so existing `@<name>` autocomplete
 *  keeps working unchanged. The category match is case-insensitive
 *  to forgive copy-pastes from a stylised hint. */
export function parseMentionQuery(query: string): ParsedMentionQuery {
  const match = query.match(/^(file|folder|issue|pr|session):(.*)$/i);
  if (match) {
    const category = match[1]!.toLowerCase() as MentionCategory;
    return { category, filter: match[2] ?? "" };
  }
  return { category: "file", filter: query };
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

/**
 * Filter rows for the redesigned `+` command menu's search box.
 *
 * Unlike {@link filterSlashItems} (which only matches the leading
 * command token), this searches across the label, description AND
 * command tag so a row is findable by any of its visible text. A
 * leading `/` scopes the match to the command tag only — so `/pl`
 * resolves to Plan (tag `/plan`) without also matching prose that
 * happens to contain "pl" in another row's description. Disabled rows
 * are kept (they stay visible with their reason); the caller renders
 * them non-selectable. Case-insensitive; empty query returns all.
 */
export function filterCommandMenuItems(
  items: SlashCommandItem[],
  query: string,
): SlashCommandItem[] {
  const raw = query.trim().toLowerCase();
  if (!raw) return items;
  if (raw.startsWith("/")) {
    return items.filter((item) =>
      item.command.toLowerCase().startsWith(raw),
    );
  }
  return items.filter(
    (item) =>
      item.label.toLowerCase().includes(raw) ||
      (item.description?.toLowerCase().includes(raw) ?? false) ||
      item.command.toLowerCase().includes(raw),
  );
}

interface BuildModeCommandsArgs {
  /** Currently-active mode. The matching command is omitted from the
   *  returned list so the user can't double-activate. */
  activeMode: ChatMode;
  /** Called when the user picks one of the mode items. */
  onActivate: (mode: ActivePillMode) => void;
  /** Called when the user picks `/default` to return to normal build
   *  mode. Optional so existing call sites keep compiling; the
   *  `/default` row only renders when this is provided AND a
   *  non-default mode is active (it's a no-op otherwise). */
  onDeactivate?: () => void;
}

/**
 * Build the `MODES` group of slash items.
 *
 * The active mode is filtered out so the popup never shows a no-op
 * choice. When `mode === "default"`, all three modes are shown and
 * the `/default` row is omitted; when a mode is active, `/default`
 * appears so the mode can be dropped without reaching for the pill's
 * × button (parity with multi-provider chat clients).
 */
export function buildModeCommands({
  activeMode,
  onActivate,
  onDeactivate,
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
  if (activeMode !== "default" && onDeactivate) {
    all.push({
      id: "mode:default",
      label: "Default",
      description: "Back to normal build mode",
      command: "/default",
      icon: RotateCcw,
      group: "MODES",
      onSelect: onDeactivate,
    });
  }
  return all.filter((item) => item.id !== `mode:${activeMode}`);
}

interface BuildWorkflowCommandArgs {
  /** True when the active model is a Claude model. `/workflow` only
   *  runs through the Claude Code runtime's server-side orchestration
   *  (it fans a task out to many subagents), so other providers keep
   *  the row visible but disabled with a reason rather than hiding it
   *  outright — mirrors the `attach:image` capability-gate pattern. */
  isClaude: boolean;
}

/**
 * Build the single `/workflow` row shared by both command surfaces
 * (the typed `/` popup and the `+` menu). Selecting it only ever
 * inserts the literal text `/workflow ` into the draft — the runtime
 * parses that prefix server-side and drives the orchestration; there
 * is no frontend workflow-start logic here.
 */
export function buildWorkflowCommand({
  isClaude,
}: BuildWorkflowCommandArgs): SlashCommandItem {
  return {
    id: "workflow",
    label: "Workflow",
    description: isClaude
      ? "Orchestrate this task with many subagents"
      : "Only available with Claude models",
    command: "/workflow",
    icon: Workflow,
    tone: "ember",
    group: "WORKFLOWS",
    disabled: !isClaude,
    onSelect: () => {},
  };
}

interface BuildModelCommandArgs {
  /** Called when the user picks `/model`. The composer opens the
   *  model-picker popover in the footer — the typed text is stripped
   *  (state-only activation, same handling as mode picks). */
  onOpen: () => void;
}

/**
 * Build the `/model` row — a GUI-local built-in that opens the
 * footer's model picker instead of sending anything to the agent.
 * Parity with multi-provider chat clients whose `/model` clears the
 * draft and pops the picker.
 */
export function buildModelCommand({
  onOpen,
}: BuildModelCommandArgs): SlashCommandItem {
  return {
    id: "composer:model",
    label: "Model",
    description: "Switch the model for this thread",
    command: "/model",
    icon: Cpu,
    group: "SETTINGS",
    onSelect: onOpen,
  };
}

interface BuildResumeCommandArgs {
  /** Called when the user picks `/resume`. The composer opens the
   *  adoptable-session picker — the typed text is stripped, because
   *  nothing is inserted into the draft (state-only activation, same
   *  handling as `/model`). */
  onOpen: () => void;
}

/**
 * Build the `/resume` row — a GUI-local built-in that opens the picker
 * of conversations the agent CLI created outside Codemux (in a
 * terminal, or on another checkout of the same repo).
 *
 * Picking one adopts it into a thread bound to the folder the session
 * already lives in; it never inserts text and never reaches the
 * provider as a command.
 */
export function buildResumeCommand({
  onOpen,
}: BuildResumeCommandArgs): SlashCommandItem {
  return {
    id: "composer:resume",
    label: "Resume",
    description: "Pick up a conversation started outside Codemux",
    command: "/resume",
    icon: History,
    group: "SESSIONS",
    onSelect: onOpen,
  };
}

interface BuildProviderCommandsArgs {
  /** Provider-native commands discovered live from the provider (e.g.
   *  Claude Code's `/compact`, `/init`, `/review`, custom
   *  `.claude/commands` entries). Never hardcoded. */
  commands: ProviderSlashCommand[];
  /** Lowercased command names already claimed by Codemux-local rows
   *  (modes, `/workflow`, `/model`, skills). Collisions are dropped
   *  from the provider group — the local behaviour wins because it's
   *  what the user sees highlighted and expanded client-side. */
  reservedNames: ReadonlySet<string>;
}

/**
 * Build the `COMMANDS` group of slash items from the provider's
 * discovered command list.
 *
 * Selecting one only ever inserts the literal `/name ` text into the
 * draft (same mechanics as `/workflow` and skills); the text is
 * forwarded verbatim to the provider, which interprets the leading
 * slash itself. Codemux never executes provider commands locally.
 */
export function buildProviderCommands({
  commands,
  reservedNames,
}: BuildProviderCommandsArgs): SlashCommandItem[] {
  return commands
    .filter((c) => !reservedNames.has(c.name.toLowerCase()))
    .map((c) => ({
      id: `provider-command:${c.name}`,
      label: c.name,
      description:
        c.description ||
        (c.argumentHint ? `/${c.name} ${c.argumentHint}` : "Provider command"),
      command: `/${c.name}`,
      icon: SquareSlash,
      group: "COMMANDS",
      onSelect: () => {},
    }));
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
