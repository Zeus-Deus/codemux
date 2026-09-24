import type * as React from "react";

import {
  Bug,
  Cpu,
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

import { type GoalSubcommandWord } from "./goal";

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
  /** Prose searched by {@link filterSlashItems} instead of
   *  `description`. Set when the displayed description carries
   *  decoration (scope suffixes, fallback text) that shouldn't match. */
  searchDescription?: string;
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
   *  See `rightAdornmentInteractive` for whether it owns its own
   *  clicks. */
  rightAdornment?: React.ReactNode;
  /** Whether `rightAdornment` is something the user can click (a
   *  Switch, a button). An interactive adornment swallows its own
   *  pointer events so the row underneath is not selected by
   *  accident; a purely decorative one (a provider label, a relative
   *  timestamp) must stay transparent to clicks, or it turns the
   *  right-hand slice of the row into a dead zone. Defaults to
   *  false. */
  rightAdornmentInteractive?: boolean;
  /** Opt into a roomier two-line row with the description below the
   *  label. Useful for entity results (such as conversations) whose
   *  names and previews need independent truncation. */
  stacked?: boolean;
  /** Redesigned `+` command menu — tone of the row's 24px icon chip.
   *  Optional; the menu falls back to `muted` when unset. Ignored by
   *  the legacy `SlashCommandPopup` (slash / mention surfaces). */
  tone?: CommandTone;
  /** Provider-supplied argument hint (e.g. `<pr-url>`), kept verbatim so
   *  the popup can show it after the label. Only set on commands without
   *  registered subcommands — those list their arguments instead. */
  argumentHint?: string;
}

export interface SlashAnchor {
  /** Index of the trigger character (`/` or `@`) in the value. */
  start: number;
  /** Substring between the trigger and the cursor. Empty when the
   *  user has just typed the trigger. */
  query: string;
  /** Set when the cursor sits in the argument slot of `/<command> <query>`
   *  rather than on the command name itself. `query` is then the partial
   *  subcommand. See {@link findSlashArgAtCursor}. */
  command?: string;
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

/** Thread state that decides which argument affordances apply. */
export interface SlashCommandContext {
  /** The thread has a standing goal (`threads[id].goal !== null`). */
  hasActiveGoal: boolean;
}

/** One entry in a command's subcommand list. */
export interface SlashSubcommand {
  name: string;
  description: string;
  /** Only offered while the thread has a standing goal — managing a goal
   *  that doesn't exist is meaningless. */
  requiresActiveGoal?: boolean;
}

/** Argument affordances for one provider command. */
interface SlashCommandSpec {
  subcommands: readonly SlashSubcommand[];
  /** Muted ghost text shown after `/<command> ` before any argument is
   *  typed. Overrides the provider's own `argumentHint`. */
  argumentPlaceholder?: (context: SlashCommandContext) => string;
}

const GOAL_SUBCOMMANDS: ReadonlyArray<
  SlashSubcommand & { name: GoalSubcommandWord }
> = [
  {
    name: "resume",
    description: "Resume the goal after an interruption",
    requiresActiveGoal: true,
  },
  {
    name: "clear",
    description: "Clear the current goal",
    requiresActiveGoal: true,
  },
  {
    name: "status",
    description: "Show the goal's progress",
    requiresActiveGoal: true,
  },
  {
    name: "pause",
    description: "Pause working on the goal",
    requiresActiveGoal: true,
  },
];

/**
 * Argument affordances for provider commands, keyed by lowercase command
 * name. Providers advertise a command's name but not its subcommands, so
 * this is the one place to list them. The composer only uses an entry when
 * the provider actually advertised that command.
 */
const SLASH_COMMAND_SPECS: ReadonlyMap<string, SlashCommandSpec> = new Map([
  [
    "goal",
    {
      subcommands: GOAL_SUBCOMMANDS,
      argumentPlaceholder: ({ hasActiveGoal }) =>
        hasActiveGoal
          ? "describe a new goal, or pick an option above"
          : "describe the goal to work toward",
    },
  ],
]);

/** Every registered subcommand for `name`, regardless of availability, or
 *  an empty list. */
export function subcommandsFor(name: string): readonly SlashSubcommand[] {
  return SLASH_COMMAND_SPECS.get(name.toLowerCase())?.subcommands ?? [];
}

/** The subcommands of `name` that apply in `context`. */
export function availableSubcommands(
  name: string,
  context: SlashCommandContext,
): readonly SlashSubcommand[] {
  return subcommandsFor(name).filter(
    (sub) => !sub.requiresActiveGoal || context.hasActiveGoal,
  );
}

/** A provider `argumentHint` made readable as ghost text: one surrounding
 *  `<...>` or `[...]` pair is dropped (`<optional summary>` → `optional
 *  summary`); anything more structured is kept verbatim. */
export function cleanArgumentHint(hint: string | undefined): string | null {
  const trimmed = hint?.trim() ?? "";
  if (!trimmed) return null;
  const wrapped = /^(?:<([^<>[\]]*)>|\[([^<>[\]]*)\])$/.exec(trimmed);
  const inner = wrapped ? (wrapped[1] ?? wrapped[2] ?? "").trim() : trimmed;
  return inner || null;
}

/**
 * Ghost text for an empty argument slot: the draft is exactly
 * `/<command> ` (leading spaces allowed, one trailing space, nothing else,
 * caret at the end) and `<command>` is an advertised provider command. The
 * slash must lead the draft, same as the provider-command popup rule.
 * Returns null when no hint applies.
 */
export function slashArgumentPlaceholder({
  value,
  caretAtEnd,
  commands,
  context,
}: {
  value: string;
  caretAtEnd: boolean;
  /** Advertised provider commands (label = command name). */
  commands: ReadonlyArray<Pick<SlashCommandItem, "label" | "argumentHint">>;
  context: SlashCommandContext;
}): string | null {
  if (!caretAtEnd) return null;
  const match = /^[ \t]*\/([^\s/]+) $/.exec(value);
  if (!match) return null;
  const name = match[1]!.toLowerCase();
  const command = commands.find((c) => c.label.toLowerCase() === name);
  if (!command) return null;
  const spec = SLASH_COMMAND_SPECS.get(name);
  if (spec?.argumentPlaceholder) return spec.argumentPlaceholder(context);
  return cleanArgumentHint(command.argumentHint);
}

/**
 * Detect the cursor in the argument slot of a leading `/<command> <partial>`
 * whose command has registered subcommands. Like provider commands, the
 * slash must lead the draft (leading whitespace allowed). `<partial>` is a
 * single word, so a second space, or text that can't be a subcommand (a
 * URL, an `@` mention), ends the match.
 *
 * Examples:
 *   `"/goal "`            → { start: 0, command: "goal", query: "" }
 *   `"/goal re"`          → { start: 0, command: "goal", query: "re" }
 *   `"/goal resume "`     → null (past the argument)
 *   `"/compact "`         → null (no registered subcommands)
 *   `"hi /goal re"`       → null (slash doesn't lead the draft)
 */
export function findSlashArgAtCursor(
  value: string,
  cursor: number,
): SlashAnchor | null {
  if (cursor < 0 || cursor > value.length) return null;
  const match = /^(\s*)\/([^\s/]+)[ \t]([\w-]*)$/.exec(value.slice(0, cursor));
  if (!match) return null;
  const command = match[2]!;
  if (subcommandsFor(command).length === 0) return null;
  return { start: match[1]!.length, query: match[3]!, command };
}

/** The slash context at the cursor: a command name being typed, or a
 *  subcommand argument after one. */
export function findSlashContextAtCursor(
  value: string,
  cursor: number,
): SlashAnchor | null {
  return findSlashAtCursor(value, cursor) ?? findSlashArgAtCursor(value, cursor);
}

/**
 * Popup rows for `command`'s subcommands that apply in `context` and whose
 * name starts with `query` (case-insensitive). Picking one inserts
 * `/<command> <name> `.
 */
export function buildSubcommandItems(
  command: string,
  query: string,
  context: SlashCommandContext,
): SlashCommandItem[] {
  const q = query.toLowerCase();
  return availableSubcommands(command, context)
    .filter((sub) => sub.name.startsWith(q))
    .map((sub) => ({
      id: `subcommand:${command}:${sub.name}`,
      label: sub.name,
      description: sub.description,
      command: `/${command} ${sub.name}`,
      icon: SquareSlash,
      group: `/${command}`,
      onSelect: () => {},
    }));
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

/** Shortest query that also searches descriptions. */
const DESCRIPTION_MATCH_MIN_LENGTH = 3;

/**
 * Filter slash items by query. Matches when the typed string is a
 * prefix of the literal command (`/pl` → `/plan`) OR when it appears
 * anywhere in the label (`pla` → `Plan`). Case-insensitive.
 *
 * Queries of three or more characters also match descriptions, ranked
 * after name matches within the same group. A skill's name is often not
 * the word the user remembers: `update-personal-desktop` is "the Hermes
 * one", and the Codex CLI finds it that way too.
 */
export function filterSlashItems(
  items: SlashCommandItem[],
  query: string,
): SlashCommandItem[] {
  if (!query) return items;
  const q = query.toLowerCase();
  // Rank within each group so the flat list stays group-contiguous:
  // the popup draws rows via `groupSlashItems`, and arrow keys walk
  // this array, so the two orders must agree.
  const groups = new Map<
    string,
    { names: SlashCommandItem[]; descriptions: SlashCommandItem[] }
  >();
  const bucketFor = (group: string) => {
    let bucket = groups.get(group);
    if (!bucket) {
      bucket = { names: [], descriptions: [] };
      groups.set(group, bucket);
    }
    return bucket;
  };
  const searchDescriptions = q.length >= DESCRIPTION_MATCH_MIN_LENGTH;
  for (const item of items) {
    if (
      item.command.toLowerCase().startsWith(`/${q}`) ||
      item.label.toLowerCase().includes(q)
    ) {
      bucketFor(item.group).names.push(item);
    } else if (
      searchDescriptions &&
      (item.searchDescription ?? item.description)
        ?.toLowerCase()
        .includes(q)
    ) {
      bucketFor(item.group).descriptions.push(item);
    }
  }
  return [...groups.values()].flatMap((b) => [...b.names, ...b.descriptions]);
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
      searchDescription: c.description,
      command: `/${c.name}`,
      argumentHint:
        c.argumentHint && subcommandsFor(c.name).length === 0
          ? c.argumentHint
          : undefined,
      icon: SquareSlash,
      group: "COMMANDS",
      onSelect: () => {},
    }));
}

/** What the goal row's actions send. Both are plain text through the normal
 *  send path; Codemux never drives the provider's goal loop itself. */
export interface GoalPhrases {
  /** Sent by Resume, and shown verbatim in the row's `sends` box. */
  resume: string;
  /** Sent by Clear goal. */
  clear: string;
}

/**
 * The resume / clear phrases for the provider's goal command, sitting next
 * to the provider commands they belong to.
 *
 * A provider that advertises its own `goal` command gets that command's
 * subcommands (`/goal resume`, `/goal clear`). Anything else gets the goal
 * restated as a plain continue instruction. That is a guess, which is why
 * the row shows the literal text before it goes and lets the user edit it.
 * Clear keeps `/goal clear` either way: it is the turn Codemux records as
 * "no goal", so it has to reach the transcript for the clear to persist.
 */
export function buildGoalPhrases({
  commands,
  goalText,
}: {
  /** The provider's live-discovered commands (see `buildProviderCommands`). */
  commands: readonly ProviderSlashCommand[];
  goalText: string;
}): GoalPhrases {
  const native = commands.some((c) => c.name.toLowerCase() === "goal");
  return {
    resume: native
      ? "/goal resume"
      : `Continue working toward this goal: ${goalText}`,
    clear: "/goal clear",
  };
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
