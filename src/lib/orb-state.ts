import type { OrbState } from "thinking-orbs";

/**
 * Single source of truth for "what is the agent doing right now" → which
 * `thinking-orbs` state to paint. Every orb surface (sidebar cards, the
 * transcript's in-flight turn marker, subagent rows, the docked composer
 * strip) resolves through here so the same activity never animates two
 * different ways in two places.
 *
 * The mapping is deliberately coarse. The orb is a *mood*, not a readout:
 * the precise tool and target are already spelled out in the text next to
 * it, so the orb only has to answer "what kind of busy is this".
 *
 * Neutral fallback whenever nothing better is known. Also the pinned state
 * for every orb when Settings → Appearance → Agents → "Match the orb to
 * the activity" is off.
 */
export const ORB_FALLBACK_STATE: OrbState = "working";

/**
 * The live signal a surface can offer. Every field is optional — a surface
 * that only knows "something is running" passes `{}` and gets the neutral
 * working orb, which is exactly right for the sidebar (see
 * `docs/features/sidebar.md`: `pane_statuses` carries no tool name).
 */
export interface OrbActivity {
  /** Name of the tool currently running, if the surface can see one. */
  toolName?: string | null;
  /** That tool's input, used to read a shell command's intent. */
  toolInput?: Record<string, unknown> | null;
  /** Blocked on a human — a permission request or a question. */
  awaitingUser?: boolean;
  /** Accepted but not started yet. */
  queued?: boolean;
  /** Having another go after something failed. */
  retrying?: boolean;
}

/**
 * Exact tool-name matches, checked before any keyword scan.
 *
 * Keys are normalized by {@link normalizeToolName}. The list spans all
 * three providers' native vocabularies (Claude `Read`/`Bash`/`MultiEdit`,
 * Codex `shell`/`apply_patch`/`read_file`, OpenCode `list`/`webfetch`)
 * because a thread can be running on any of them.
 */
const STATE_BY_TOOL: Record<string, OrbState> = {
  // ── searching: reading files and looking things up in the repo ──
  read: "searching",
  readfile: "searching",
  read_file: "searching",
  view: "searching",
  cat: "searching",
  grep: "searching",
  grep_search: "searching",
  glob: "searching",
  ls: "searching",
  list: "searching",
  list_dir: "searching",
  listdirectory: "searching",
  find: "searching",
  search: "searching",
  file_search: "searching",
  codebase_search: "searching",
  notebookread: "searching",
  explore: "searching",
  // A web search is still a search — the answer comes back as something to
  // read, not as a connection the user is waiting on.
  websearch: "searching",
  web_search: "searching",

  // ── composing: producing new text ──
  write: "composing",
  writefile: "composing",
  write_file: "composing",
  edit: "composing",
  editfile: "composing",
  edit_file: "composing",
  multiedit: "composing",
  multi_edit: "composing",
  str_replace: "composing",
  str_replace_editor: "composing",
  apply_patch: "composing",
  applypatch: "composing",
  patch: "composing",
  create: "composing",
  notebookedit: "composing",
  notebook_edit: "composing",

  // ── working: running things ──
  bash: "working",
  bashoutput: "working",
  shell: "working",
  run: "working",
  exec: "working",
  execute: "working",
  terminal: "working",
  run_command: "working",
  run_terminal_cmd: "working",
  killshell: "working",
  killbash: "working",
  test: "working",

  // ── connecting: reaching something that isn't this machine ──
  webfetch: "connecting",
  web_fetch: "connecting",
  fetch: "connecting",
  curl: "connecting",
  git: "connecting",
  gh: "connecting",
  github: "connecting",

  // ── listening: the turn is parked on a human ──
  askuserquestion: "listening",
  ask_user_question: "listening",
  exitplanmode: "listening",
  exit_plan_mode: "listening",
};

/**
 * Ordered keyword fallback for names the exact table misses (MCP tools,
 * provider-specific spellings, anything new a provider ships).
 *
 * Order is load-bearing: merges are checked before git so `git_merge`
 * weaves rather than connects, and network verbs are checked before the
 * read/write verbs so `fetch_url` doesn't read as a file search.
 */
const STATE_BY_KEYWORD: [readonly string[], OrbState][] = [
  [["merge", "rebase", "cherry_pick", "cherrypick", "conflict"], "weaving"],
  [
    [
      "git",
      "github",
      "gitlab",
      "clone",
      "push",
      "pull",
      "fetch",
      "curl",
      "wget",
      "http",
      "url",
      "network",
      "socket",
      "deploy",
      "publish",
      "workflow_run",
      "pipeline",
      " ci",
      "browser",
    ],
    "connecting",
  ],
  [
    ["search", "grep", "glob", "find", "read", "list", "view", "explore"],
    "searching",
  ],
  [
    ["write", "edit", "patch", "replace", "insert", "append", "compose"],
    "composing",
  ],
  [
    ["bash", "shell", "exec", "run", "terminal", "command", "test", "build"],
    "working",
  ],
];

/**
 * Shell-command intent, checked when the running tool is a shell. The tool
 * name alone says "working" for every command, which throws away the most
 * legible signal in the app — `git push` and `cargo test` are visibly
 * different kinds of waiting.
 *
 * Ordered, first match wins.
 */
const STATE_BY_COMMAND: [RegExp, OrbState][] = [
  [/\bgit\s+(merge|rebase|cherry-pick|revert)\b/, "weaving"],
  [
    /\b(gh|git\s+(push|pull|fetch|clone|remote|ls-remote))\b|\b(curl|wget|ssh|scp|rsync|docker\s+(push|pull))\b|\bnpm\s+(publish|login)\b/,
    "connecting",
  ],
  [
    /\b(npm|pnpm|yarn|bun)\s+(run|test|build)\b|\b(vitest|jest|pytest|tox|rspec|phpunit|go\s+test|cargo\s+(test|check|build|clippy)|make|gradle|mvn|tsc|eslint|ruff|mypy)\b/,
    "working",
  ],
  [/\b(rg|ag|grep|find|fd|ls|cat|head|tail|wc|tree)\b/, "searching"],
  [/\b(sed|awk|tee)\b/, "composing"],
];

/**
 * Strip a tool name down to something matchable.
 *
 * MCP tools arrive fully qualified (`mcp__codemux__git_status`); only the
 * last segment carries meaning, so the server prefix is dropped. Everything
 * else is lowercased and trimmed of separator noise.
 */
function normalizeToolName(name: string): string {
  const tail = name.includes("__") ? name.slice(name.lastIndexOf("__") + 2) : name;
  return tail.toLowerCase().trim().replace(/^[._-]+|[._-]+$/g, "");
}

/** True when this tool runs a shell command rather than doing its own work. */
function isShellTool(normalized: string): boolean {
  return (
    normalized === "bash" ||
    normalized === "shell" ||
    normalized === "exec" ||
    normalized === "execute" ||
    normalized === "terminal" ||
    normalized === "run_command" ||
    normalized === "run_terminal_cmd"
  );
}

/**
 * Map a running tool (and, for shells, what it is running) to an orb state.
 * Returns `null` when the name carries no usable signal, so callers can
 * fall through to their own default rather than being handed a wrong guess.
 */
export function orbStateForTool(
  toolName: string | null | undefined,
  toolInput?: Record<string, unknown> | null,
): OrbState | null {
  if (!toolName) return null;
  const normalized = normalizeToolName(toolName);
  if (!normalized) return null;

  if (isShellTool(normalized)) {
    const command = toolInput?.command;
    if (typeof command === "string" && command.length > 0) {
      const haystack = command.toLowerCase();
      for (const [pattern, state] of STATE_BY_COMMAND) {
        if (pattern.test(haystack)) return state;
      }
    }
    return "working";
  }

  const exact = STATE_BY_TOOL[normalized];
  if (exact) return exact;

  for (const [keywords, state] of STATE_BY_KEYWORD) {
    if (keywords.some((keyword) => normalized.includes(keyword.trim()))) {
      return state;
    }
  }
  return null;
}

/**
 * Resolve a surface's live signal to the orb state it should paint.
 *
 * Precedence runs from "not moving" to "moving", because a stopped reason
 * outranks whatever tool happens to be the most recent one in the
 * transcript: a turn parked on a permission prompt is listening even
 * though its last tool call was a grep.
 */
export function resolveOrbState(activity: OrbActivity): OrbState {
  if (activity.awaitingUser) return "listening";
  if (activity.queued) return "breathing";
  if (activity.retrying) return "solving";
  return orbStateForTool(activity.toolName, activity.toolInput) ?? ORB_FALLBACK_STATE;
}
