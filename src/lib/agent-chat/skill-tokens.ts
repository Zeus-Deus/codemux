// Skill-token parser for the Cursor-style inline composer.
//
// `/skill-name` tokens that appear in the textarea are matched against
// the skills registry. Matched tokens drive (a) the in-textarea
// highlight overlay and (b) the per-turn body injection at send time.
// Unmatched `/foo` text passes through as plain prose so a typo never
// silently injects nothing.

import type { Skill } from "@/tauri/commands";

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
const SKILL_TOKEN_RE = /(?<=^|\s)\/([A-Za-z0-9_-]+)(?=[^A-Za-z0-9_-]|$)/g;

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
export function parseSkillTokens(text: string, skills: Skill[]): SkillTokenMatch[] {
  if (!text || skills.length === 0) return [];

  // Build a fast lookup once per call. Skill lists are small (≤50 in
  // practice) so a Map is sufficient — no need to memoize across calls.
  //
  // FIRST-wins, matching the popup's `dedupeSkillsByName`. A plain
  // `set` loop would be last-wins, which silently disagreed with the
  // menu: picking the top `/omarchy` row inserted text that resolved
  // to the *lowest*-priority copy's body at send time.
  const byName = new Map<string, Skill>();
  for (const s of skills) {
    if (!byName.has(s.name)) byName.set(s.name, s);
  }

  const matches: SkillTokenMatch[] = [];
  // Reset stateful regex's lastIndex per call — `g` flag mutates the
  // RegExp object across executions, which would otherwise drop matches
  // on the second invocation.
  SKILL_TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = SKILL_TOKEN_RE.exec(text)) !== null) {
    const name = m[1];
    if (!name) continue;
    const skill = byName.get(name);
    if (!skill) continue;
    const start = m.index;
    const end = start + m[0].length;
    matches.push({ start, end, token: m[0], name, skill });
  }
  return matches;
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
export function resolveSkillBodies(text: string, skills: Skill[]): string | null {
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

export function segmentForHighlight(text: string, skills: Skill[]): HighlightSegment[] {
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
