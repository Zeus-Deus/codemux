/**
 * Build SDK-shaped `PermissionUpdate[]` payloads for "Allow always"
 * decisions. The SDK consumes these via the `updatedPermissions`
 * field on a permission decision; persistence is its responsibility,
 * not Codemux's.
 *
 * Scopes:
 *   `once`    → no payload, single-shot allow with no rule saved
 *   `session` → rule lives only as long as the provider session
 *   `project` → write to the project's `.claude/settings.local.json`
 *   `user`    → write to the user-wide `~/.claude/settings.json`
 *
 * A rule without `ruleContent` matches every input for the tool, so the
 * UI offers the narrower rule from `suggestPermissionRule` first and
 * always shows the exact rule text (`formatPermissionRule`) before the
 * user commits to it.
 */

export type PermissionScope = "once" | "session" | "project" | "user";

export interface PermissionRuleSpec {
  /** Tool name as the SDK reports it: `"Bash"`, `"Read"`, etc. */
  toolName: string;
  /** Optional rule-content string. Omit to match any input. */
  ruleContent?: string;
}

export function buildPermissionUpdate(
  scope: PermissionScope,
  rule: PermissionRuleSpec,
): unknown[] | undefined {
  // Defensive: explicit map per scope so an unknown value (e.g. a
  // typo'd literal squeaking past the type system at a JSON
  // boundary) returns undefined rather than silently falling
  // through to userSettings.
  let destination: "session" | "localSettings" | "userSettings";
  switch (scope) {
    case "once":
      return undefined;
    case "session":
      destination = "session";
      break;
    case "project":
      destination = "localSettings";
      break;
    case "user":
      destination = "userSettings";
      break;
    default:
      return undefined;
  }

  return [
    {
      type: "addRules",
      rules: [
        {
          toolName: rule.toolName,
          ...(rule.ruleContent ? { ruleContent: rule.ruleContent } : {}),
        },
      ],
      behavior: "allow",
      destination,
    },
  ];
}

/** The rule as Claude Code writes it in settings: `Bash(git status:*)`,
 *  or the bare tool name when it matches any input. */
export function formatPermissionRule(rule: PermissionRuleSpec): string {
  return rule.ruleContent
    ? `${rule.toolName}(${rule.ruleContent})`
    : rule.toolName;
}

/** Shell syntax that chains or redirects commands. A prefix rule derived
 *  from such a command would describe only its first part, so none is
 *  offered. */
const SHELL_CONTROL = /[;&|<>`\n]|\$\(/;
/** A subcommand word (`status`, `run`, `check`), as opposed to a flag,
 *  path, file name or quoted argument. */
const SUBCOMMAND = /^[a-z][a-z0-9-]*$/i;
const FILE_EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);
/** Shells, interpreters and command wrappers. A prefix rule for them
 *  (`Bash(python:*)`, `Bash(sudo:*)`) runs arbitrary code, so it is as
 *  broad as the bare tool and must not be offered as the narrow choice. */
const ARBITRARY_CODE_PROGRAMS = new Set([
  "bash", "sh", "zsh", "fish", "dash", "ksh", "csh", "tcsh",
  "env", "sudo", "doas", "su", "xargs", "exec", "eval", "command",
  "builtin", "nohup", "nice", "ionice", "time", "timeout", "watch",
  "stdbuf", "setsid", "chroot", "unshare", "flock",
  "python", "python2", "python3", "node", "deno", "bun", "ruby", "perl",
  "php", "lua", "osascript", "pwsh", "powershell", "awk", "gawk",
  // Package runners fetch and execute any package.
  "npx", "bunx", "pipx", "uvx",
]);
/** Package-manager subcommands that fetch or run an arbitrary package
 *  (`npm exec`, `pnpm dlx`, `yarn dlx`, `bun x`), at any depth. `uv tool
 *  run`, the long form of `uvx`, is checked separately because `run`
 *  alone (`npm run build`) is safe. */
const ARBITRARY_CODE_SUBCOMMANDS = new Set(["exec", "dlx", "x"]);
/** Characters that would end or nest Claude's `Tool(content)` rule
 *  syntax. */
const RULE_DELIMITERS = /[()]/;

/**
 * The narrowest useful rule for this call, or `null` when none can be
 * derived safely:
 *
 *   Bash      `npm run build --watch` → `Bash(npm run build:*)`
 *   Edit etc. `/repo/src/a.ts`        → `Edit(//repo/src/**)`
 *   Read      `/repo/src/a.ts`        → `Read(//repo/src/**)`
 *   WebFetch  `https://docs.rs/x`     → `WebFetch(domain:docs.rs)`
 *
 * Every file-editing tool is governed by `Edit` rules in Claude Code, so
 * Write and MultiEdit calls suggest an `Edit(...)` rule.
 */
export function suggestPermissionRule(
  toolName: string,
  input: unknown,
): PermissionRuleSpec | null {
  if (!isRecord(input)) return null;

  if (toolName === "Bash") {
    const command = input.command;
    if (typeof command !== "string") return null;
    const trimmed = command.trim();
    if (!trimmed || SHELL_CONTROL.test(trimmed)) return null;
    const [program, ...rest] = trimmed.split(/\s+/);
    if (!program || program.includes("=") || RULE_DELIMITERS.test(program)) {
      return null;
    }
    const base = program.slice(program.lastIndexOf("/") + 1);
    // `python3.12`, `node18` and similar versioned binaries count too.
    const family = base.replace(/[\d.]+$/, "");
    if (ARBITRARY_CODE_PROGRAMS.has(base) || ARBITRARY_CODE_PROGRAMS.has(family)) {
      return null;
    }
    const words = [program];
    for (const word of rest) {
      if (words.length === 3 || !SUBCOMMAND.test(word)) break;
      words.push(word);
    }
    const subcommands = words.slice(1);
    if (
      subcommands.some((word) => ARBITRARY_CODE_SUBCOMMANDS.has(word)) ||
      (family === "uv" && subcommands[0] === "tool" && subcommands[1] === "run")
    ) {
      return null;
    }
    return { toolName: "Bash", ruleContent: `${words.join(" ")}:*` };
  }

  if (FILE_EDIT_TOOLS.has(toolName) || toolName === "Read") {
    const path = input.file_path ?? input.notebook_path;
    const dir = typeof path === "string" ? parentDirectory(path) : null;
    if (!dir || RULE_DELIMITERS.test(dir)) return null;
    return {
      toolName: toolName === "Read" ? "Read" : "Edit",
      ruleContent: `/${dir}/**`,
    };
  }

  if (toolName === "WebFetch" && typeof input.url === "string") {
    try {
      const host = new URL(input.url).hostname;
      return host ? { toolName, ruleContent: `domain:${host}` } : null;
    } catch {
      return null;
    }
  }

  return null;
}

/** Absolute POSIX parent directory, or `null` for relative and Windows
 *  paths, whose rule syntax this does not attempt. */
function parentDirectory(path: string): string | null {
  if (!path.startsWith("/")) return null;
  const cut = path.lastIndexOf("/");
  return cut > 0 ? path.slice(0, cut) : null;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
