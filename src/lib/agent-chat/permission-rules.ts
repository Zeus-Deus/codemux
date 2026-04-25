/**
 * Build SDK-shaped `PermissionUpdate[]` payloads for "Allow always"
 * decisions. The SDK consumes these via the `updatedPermissions`
 * field on a permission decision; persistence is its responsibility,
 * not Codemux's.
 *
 * Three scopes:
 *   `once`    → no payload, single-shot allow with no rule saved
 *   `project` → write to the project's `.claude/settings.local.json`
 *   `user`    → write to the user-wide `~/.claude/settings.json`
 *
 * `ruleContent` is omitted for Stage 5 — every "Allow always" matches
 * any input for that tool. Stage 7 will add command-specific rule
 * granularity (e.g. `Bash(git status)` vs `Bash(*)`) by parsing the
 * tool input and generating a more specific rule string.
 */

export type PermissionScope = "once" | "project" | "user";

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
  let destination: "localSettings" | "userSettings";
  switch (scope) {
    case "once":
      return undefined;
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
