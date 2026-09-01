// Skill-token parser for the Cursor-style inline composer. Recognized tokens
// resolve to stable ids; the backend revalidates those ids at turn time and
// chooses the provider-specific invocation mechanism.

import type { Skill, SkillProvider } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

export interface SkillTokenMatch {
  /** Inclusive start offset in the source text, points at the `/`. */
  start: number;
  /** Exclusive end offset (one past the last char of the name). */
  end: number;
  /** Literal token, e.g. `/codemux-release`. Useful for highlight render. */
  token: string;
  /** Bare skill name, e.g. `codemux-release`. */
  name: string;
  /** Resolved skill from the registry. */
  skill: Skill;
}

// Slash must sit at start-of-text or be preceded by whitespace, mirroring
// the rule in `findSlashAtCursor`. The name is `[A-Za-z0-9_-]+` and must
// end at a non-name character (or end-of-text). Lookbehind + lookahead
// keep matches non-greedy and boundary-respecting.
const SKILL_TOKEN_RE =
  /(?<=^|\s)\/([A-Za-z0-9_-]+(?::[A-Za-z0-9_-]+){0,3})(?=[^A-Za-z0-9_:-]|$)/g;

/** Match chat adapters to the projection the backend invokes for them. */
function projectionProviderForChat(
  provider: AgentChatProviderKind,
): SkillProvider {
  switch (provider) {
    case "claude":
    case "codex":
    case "opencode":
      return provider;
    // Cursor and Grok have no native SKILL.md inventory. The backend feeds
    // both the portable `.agents/skills` / Codex projection.
    case "cursor":
    case "grok":
      return "codex";
  }
}

/** Keep popup tokens, highlighting, and send-time resolution on the same
 * target-provider inventory. An unavailable name collision must not change
 * the token address after the user selects it. */
export function skillsForProvider(
  skills: Skill[],
  provider: AgentChatProviderKind,
): Skill[] {
  const targetProvider = projectionProviderForChat(provider);
  return skills.filter((skill) => {
    // Old cached/test records can predate projections entirely. Keep their
    // legacy availability, but once a record supplies a projection table an
    // absent target is fail-closed so popup and backend validation agree.
    if (!skill.projections) return true;
    const projection = skill.projections.find(
      (candidate) => candidate.targetProvider === targetProvider,
    );
    return (
      projection !== undefined &&
      projection.availability !== "unavailable" &&
      projection.availability !== "native-only"
    );
  });
}

/** Stable textual address for one exact definition. Unique names keep the
 * friendly `/name`; collisions are qualified by provider and scope. */
export function skillTokenFor(skill: Skill, skills: Skill[]): string {
  const conflicts = skills.filter((candidate) => candidate.name === skill.name);
  if (conflicts.length <= 1) return `/${skill.name}`;
  const sameSource = conflicts.filter(
    (candidate) =>
      candidate.provider === skill.provider && candidate.scope === skill.scope,
  );
  let suffix = "";
  if (sameSource.length > 1) {
    let prefixLength = Math.min(6, skill.id.length);
    while (
      prefixLength < skill.id.length &&
      sameSource.some(
        (candidate) =>
          candidate.id !== skill.id &&
          candidate.id.slice(0, prefixLength) ===
            skill.id.slice(0, prefixLength),
      )
    ) {
      prefixLength += 1;
    }
    suffix = `:${skill.id.slice(0, prefixLength)}`;
  }
  return `/${skill.provider}:${skill.scope}:${skill.name}${suffix}`;
}

/**
 * Find every `/skill-name` token in `text` whose name matches a skill
 * in `skills`. Match order follows source position.
 *
 * Names are matched case-sensitively to avoid surprising substitutions
 * (`/Plan` should not silently expand to a skill named `plan`). The
 * regex itself doesn't reject mismatched names — that filter happens
 * after the regex pass so the highlight UI can decide to render
 * unmatched `/foo` differently if it ever wants to.
 */
export function parseSkillTokens(
  text: string,
  skills: Skill[],
): SkillTokenMatch[] {
  if (!text || skills.length === 0) return [];

  // Build an exact-address lookup once per call. Conflicting definitions
  // have distinct qualified tokens, so no discovery-order winner exists.
  const byToken = new Map(
    skills.map((skill) => [skillTokenFor(skill, skills).slice(1), skill]),
  );

  const matches: SkillTokenMatch[] = [];
  // Reset stateful regex's lastIndex per call — `g` flag mutates the
  // RegExp object across executions, which would otherwise drop matches
  // on the second invocation.
  SKILL_TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = SKILL_TOKEN_RE.exec(text)) !== null) {
    const name = m[1];
    if (!name) continue;
    const skill = byToken.get(name);
    if (!skill) continue;
    const start = m.index;
    const end = start + m[0].length;
    matches.push({ start, end, token: m[0], name, skill });
  }
  return matches;
}

export interface ResolvedSkillSelection {
  skillIds: string[];
  /** Prompt text with Codemux-owned skill tokens removed. */
  text: string;
}

/** Rebind path-derived ids after a deferred worktree is created. Skills
 * outside the source cwd (for example user skills) retain their exact id;
 * project skills are matched by their path relative to the old/new cwd. */
export function rebaseSkillSelection(
  selection: ResolvedSkillSelection,
  sourceSkills: Skill[],
  sourceCwd: string,
  targetCwd: string,
  targetSkills: Skill[],
): ResolvedSkillSelection {
  if (sourceCwd === targetCwd || selection.skillIds.length === 0) {
    return selection;
  }
  const sourcePrefix = sourceCwd.endsWith("/") ? sourceCwd : `${sourceCwd}/`;
  const targetPrefix = targetCwd.endsWith("/") ? targetCwd : `${targetCwd}/`;
  const skillIds = selection.skillIds.map((id) => {
    const exact = targetSkills.find((skill) => skill.id === id);
    if (exact) return exact.id;
    const source = sourceSkills.find((skill) => skill.id === id);
    if (!source) throw new Error(`Selected skill is no longer available: ${id}`);
    if (!source.filePath.startsWith(sourcePrefix)) {
      throw new Error(`Selected skill is no longer available: ${source.name}`);
    }
    const rebasedPath = `${targetPrefix}${source.filePath.slice(sourcePrefix.length)}`;
    const rebased = targetSkills.find(
      (skill) =>
        skill.filePath === rebasedPath &&
        skill.provider === source.provider &&
        skill.scope === source.scope &&
        skill.name === source.name,
    );
    if (!rebased) {
      throw new Error(`Selected skill is not available in the new worktree: ${source.name}`);
    }
    return rebased.id;
  });
  return { ...selection, skillIds };
}

/** Resolve tokens to stable ids and remove only the recognized token ranges.
 * Unrecognized provider slash commands and prose remain byte-for-byte. */
export function resolveSkillSelection(
  text: string,
  skills: Skill[],
): ResolvedSkillSelection {
  const matches = parseSkillTokens(text, skills);
  if (matches.length === 0) return { skillIds: [], text };
  const ids: string[] = [];
  const seen = new Set<string>();
  let cursor = 0;
  let stripped = "";
  for (const match of matches) {
    let removeStart = match.start;
    let removeEnd = match.end;
    if (removeEnd < text.length && /[ \t]/.test(text[removeEnd])) {
      // Consume one token delimiter, leaving any additional whitespace
      // exactly as the user typed it.
      removeEnd += 1;
    } else if (
      removeStart > cursor &&
      /[ \t]/.test(text[removeStart - 1])
    ) {
      removeStart -= 1;
    }
    stripped += text.slice(cursor, removeStart);
    cursor = removeEnd;
    if (!seen.has(match.skill.id)) {
      seen.add(match.skill.id);
      ids.push(match.skill.id);
    }
  }
  stripped += text.slice(cursor);
  return {
    skillIds: ids,
    text: stripped,
  };
}

/**
 * Concatenate the bodies of every matched skill in `text`, framed by
 * markdown rules so the model can distinguish multiple stacked skills.
 * Returns `null` when no skills match (so callers can fall through to
 * the existing "no skill prefix" branch in `applyAllPrefixes`).
 *
 * Stacked order matches source order — the first skill mentioned is
 * the first body emitted. Duplicate mentions are deduped by skill id;
 * mentioning `/foo /foo` injects the body once.
 */
export function resolveSkillBodies(
  text: string,
  skills: Skill[],
): string | null {
  const matches = parseSkillTokens(text, skills);
  if (matches.length === 0) return null;

  const seen = new Set<string>();
  const bodies: string[] = [];
  for (const m of matches) {
    if (seen.has(m.skill.id)) continue;
    seen.add(m.skill.id);
    const body = m.skill.body.trim();
    if (body) bodies.push(body);
  }
  if (bodies.length === 0) return null;
  return bodies.join("\n\n---\n\n");
}

/**
 * Split `text` into a sequence of plain runs and skill-token runs. The
 * highlight overlay maps each run to either a plain `string` or a
 * styled `<span>`. Plain runs preserve original characters byte-for-byte
 * (no normalization) so cursor positioning stays in lockstep with the
 * underlying textarea.
 */
export type HighlightSegment =
  | { kind: "plain"; text: string }
  | { kind: "skill"; text: string; name: string };

export function segmentForHighlight(
  text: string,
  skills: Skill[],
): HighlightSegment[] {
  const matches = parseSkillTokens(text, skills);
  if (matches.length === 0) {
    return text ? [{ kind: "plain", text }] : [];
  }
  const out: HighlightSegment[] = [];
  let cursor = 0;
  for (const m of matches) {
    if (m.start > cursor) {
      out.push({ kind: "plain", text: text.slice(cursor, m.start) });
    }
    out.push({ kind: "skill", text: text.slice(m.start, m.end), name: m.name });
    cursor = m.end;
  }
  if (cursor < text.length) {
    out.push({ kind: "plain", text: text.slice(cursor) });
  }
  return out;
}
