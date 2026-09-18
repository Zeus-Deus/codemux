// Leading `/command` recognition for the composer.
//
// Skills already highlight wherever they appear (`skill-tokens.ts`),
// because Codemux expands them itself. Provider-native commands are
// different: the runtime only interprets a slash command when it
// *leads* the message, so only a leading token is a real invocation
// and only a leading token should be advertised as one.
//
// Recognition is registry-backed on purpose. An unknown `/foo` is just
// prose the provider will read literally, and painting it like a
// command would promise an execution that never happens.

import type { Skill } from "@/tauri/commands";

import { skillTokenFor } from "./skill-tokens";

/** What kind of thing the leading token resolves to. */
export type DraftCommandKind = "skill" | "provider";

export interface DraftCommandMatch {
  /** Inclusive start offset, points at the `/`. */
  start: number;
  /** Exclusive end offset (one past the last char of the name). */
  end: number;
  /** Literal token, e.g. `/goal`. */
  token: string;
  /** Bare name, e.g. `goal`. */
  name: string;
  kind: DraftCommandKind;
  /** One-line help shown next to the name. Empty when the registry
   *  entry carries none. */
  description: string;
  /** Everything typed after the token, trimmed. Drives the "needs an
   *  argument" hint. */
  args: string;
  /** Placeholder for the expected argument (`<version>`), when the
   *  provider advertises one. */
  argumentHint: string;
}

/** Minimal shape both registries reduce to, so the parser doesn't have
 *  to know about `Skill` vs `ProviderSlashCommand`. */
export interface CommandRegistryEntry {
  name: string;
  kind: DraftCommandKind;
  description?: string;
  argumentHint?: string;
}

// The token has to lead the message (whitespace before it is fine —
// the provider trims). The name charset matches `skill-tokens.ts`, and
// the trailing assertion rejects `/` so an absolute path pasted as the
// first thing in a draft (`/home/zeus/notes.md`) can never read as a
// command.
const LEADING_COMMAND_RE =
  /^\s*\/([A-Za-z0-9_-]+(?::[A-Za-z0-9_-]+){0,3})(?=[^A-Za-z0-9_:/-]|$)/;

/** Fold a skill list + the provider's discovered commands into one
 *  lookup keyed by lowercased name. Skills win a collision, matching
 *  the popup's `reservedNames` precedence. */
export function buildCommandRegistry(
  skills: readonly Skill[],
  providerCommands: readonly {
    name: string;
    description?: string;
    argumentHint?: string;
  }[],
): Map<string, CommandRegistryEntry> {
  const registry = new Map<string, CommandRegistryEntry>();
  for (const command of providerCommands) {
    registry.set(command.name.toLowerCase(), {
      name: command.name,
      kind: "provider",
      description: command.description,
      argumentHint: command.argumentHint,
    });
  }
  for (const skill of skills) {
    // Address the skill the same way the popup inserts it, so a
    // collision-qualified token (`/provider:scope:name`) still resolves.
    const token = skillTokenFor(skill, skills as Skill[]).slice(1);
    registry.set(token.toLowerCase(), {
      name: token,
      kind: "skill",
      description: skill.description ?? undefined,
    });
  }
  return registry;
}

/**
 * Resolve the draft's leading token against the registry, or `null`
 * when the draft doesn't open with a command the runtime will act on.
 */
export function parseLeadingCommand(
  text: string,
  registry: ReadonlyMap<string, CommandRegistryEntry>,
): DraftCommandMatch | null {
  const match = LEADING_COMMAND_RE.exec(text);
  if (!match) return null;
  const name = match[1];
  if (!name) return null;
  const entry = registry.get(name.toLowerCase());
  if (!entry) return null;
  const end = match[0].length;
  const start = end - name.length - 1;
  return {
    start,
    end,
    token: `/${name}`,
    name,
    kind: entry.kind,
    description: entry.description?.trim() ?? "",
    args: text.slice(end).trim(),
    argumentHint: entry.argumentHint?.trim() ?? "",
  };
}
