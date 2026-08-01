# Tool Permissions

- Purpose: Describe the Settings → Permissions panel that reads and manages agent tool-permission rules.
- Audience: Anyone working on the permissions UI, the settings-file scopes, or agent tool gating.
- Authority: Canonical feature-level reality doc.
- Update when: The rule model, scope resolution, or the read/remove behavior changes.
- Read next: `docs/features/settings.md`, `docs/features/agent-chat.md`

## What This Feature Is

A settings section that surfaces the **tool-permission rules** an agent session honors — the
`allow` / `deny` / `ask` rules the Claude CLI/SDK reads from its `settings.json` files — so a
user can see every rule, which file it lives in, and remove ones they no longer want. It is the
persistent-rule counterpart to the in-chat Allow/Deny approval cards (see `docs/features/agent-chat.md`):
approval cards decide a single tool call live; these rules are the standing policy the SDK applies
before it ever asks.

The section only appears when the **Agent Chat GUI** flag is on (the default; it sits under
Editor & Workflow alongside Skills and MCP Servers).

## Current Model

Rules are read from the three scopes the SDK itself reads, in this precedence order:

1. **user** — `~/.claude/settings.json`
2. **project** (shared) — `<project_root>/.claude/settings.json`
3. **local** — `<project_root>/.claude/settings.local.json`

The Rust command layer (`src-tauri/src/commands/permissions.rs`) flattens all three into one list of
`PermissionRule { tool_name, rule_content, behavior, scope, source_path }`:

- `behavior` is `"allow" | "deny" | "ask"` (parsed from the matching `permissions.<behavior>` array).
- Each rule remembers its `scope` and `source_path` so the UI can label where it came from and the
  remove path knows which file to rewrite.
- A missing settings file is not an error — absent settings mean "no rules", matching the SDK.
- SDK entries are accepted in **both** documented shapes: a string (`"Bash"` or `"Bash(git status:*)"`)
  or an object (`{ "toolName": "Bash", "ruleContent": "git status:*" }`).

The frontend (`src/components/settings/permissions-section.tsx`) loads the list via
`list_tool_permissions(project_root)`, groups rows by scope, renders a per-behavior icon, and lets the
user **remove** a rule via `remove_tool_permission(rule, project_root)`. Removal rewrites only the
owning file: it parses the JSON, `retain`s out every matching `toolName` + `ruleContent` entry from
the correct `permissions.<behavior>` array, and writes back via a sibling-tempfile + atomic rename
(`atomic_write`) so a crash can't truncate or corrupt the user's settings. Empty arrays are preserved
(the `permissions.allow` key survives as `[]`) so the JSON shape stays predictable.

## What Works Today

- Read + display every `allow` / `deny` / `ask` rule across the user / project / local scopes with the
  source file shown per group.
- Remove a rule; the change is written atomically to the owning file only, and the UI optimistically
  drops the row.
- Duplicate rules (same tool + content) are all removed in a single call.
- Robust parsing: null/absent `permissions`, wrong-typed arrays, non-object top-level JSON, and mixed
  string/object entries are all handled without panicking (malformed *arrays* are skipped; truncated
  JSON surfaces a parse error).
- Rules are **added** by the agent/CLI during normal use (e.g. clicking "Always allow" on an approval
  card, or editing settings by hand) — the panel is a read/remove surface, not an add form.

## Current Constraints

- **No add-rule form.** New rules come from the agent, the CLI, or manual edits; this panel only lists
  and removes.
- **JSONC not supported.** `settings.json` files containing `//` comments, block comments, or trailing
  commas fail `serde_json` parsing, so the panel falls back to "Failed to load rules" (see the
  `TODO(JSONC)` in `permissions.rs`). Interim fix is to edit those files by hand.
- **Last-write-wins against a concurrent CLI.** The atomic write blocks partial-write corruption but
  does not take an OS-level lock, so if the Claude CLI rewrites the same file at the same instant one
  write lands last. Accepted for now.
- **Claude-shaped only.** The rule model mirrors the Claude settings format; Codex/OpenCode do not
  surface rules here.
- Project/local scopes require a resolved `project_root`; without one only user-scope rules show.

## Important Touch Points

- `src-tauri/src/commands/permissions.rs` — `list_tool_permissions` / `remove_tool_permission`,
  scope→path resolution, atomic write, string/object rule parsing (+ extensive unit tests)
- `src/components/settings/permissions-section.tsx` — the settings UI (scope groups, behavior icons,
  remove)
- `src/components/settings/settings-view.tsx` — registers the `permissions` nav item (Beta-gated)
- `~/.claude/settings.json`, `<project>/.claude/settings.json`, `<project>/.claude/settings.local.json`
  — the on-disk sources of truth

## Notes

- Keep this file about current truth, not future plans.
- The live per-call approval flow (Allow/Deny/AskUserQuestion cards, "Always allow this tool")
  lives in `docs/features/agent-chat.md`; this doc is only the standing-rule management surface.
