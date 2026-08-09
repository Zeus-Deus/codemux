/**
 * Stress fixtures for GUI-responsiveness testing in the dev mock runtime.
 *
 * The default seed is a *design* fixture: 18 hand-curated workspaces chosen to
 * exercise every sidebar and chat state. It is deliberately far below the
 * audited real profile (79 workspaces, 74k persisted events, 165 MB of chat
 * payload), so measuring against it flatters every number. These presets scale
 * the same seed up to that profile without touching the curated data: with no
 * fixture selected, `createSeedAppState()` and the seeded transcript are
 * byte-identical to before.
 *
 * Select one with either:
 *   - a URL param — `http://localhost:1420/?fixture=large`
 *   - localStorage — `localStorage["codemux:fixture"] = "large"`
 *
 * Both also accept an inline JSON object for one-off shapes, merged over the
 * `medium` preset:
 *   `?fixture={"workspaces":80,"chatEvents":5000}`
 */

export interface StressFixture {
  /** Total workspaces in the seeded snapshot (1–80). */
  workspaces: number;
  /** Persisted chat events per synthetic thread (50–5,000). */
  chatEvents: number;
  /** Approximate total tool_result payload per synthetic thread, in MB. */
  payloadMb: number;
  /** Streamed content deltas per second, for driven-delta scenarios. */
  deltasPerSec: number;
}

export const STRESS_PRESETS: Record<string, StressFixture> = {
  small: { workspaces: 8, chatEvents: 50, payloadMb: 1, deltasPerSec: 20 },
  medium: { workspaces: 30, chatEvents: 1_000, payloadMb: 5, deltasPerSec: 50 },
  large: { workspaces: 60, chatEvents: 3_000, payloadMb: 10, deltasPerSec: 80 },
  xl: { workspaces: 80, chatEvents: 5_000, payloadMb: 15, deltasPerSec: 100 },
};

/** Thread ids handed to generated workspaces. `agent_chat_list_messages`
 *  routes anything with this prefix to the synthetic transcript. */
export const STRESS_THREAD_PREFIX = "thread-stress-";

const STORAGE_KEY = "codemux:fixture";

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, Math.round(value)));
}

function normalize(spec: Partial<StressFixture>): StressFixture {
  const base = STRESS_PRESETS.medium;
  return {
    workspaces: clamp(spec.workspaces ?? base.workspaces, 1, 80),
    chatEvents: clamp(spec.chatEvents ?? base.chatEvents, 50, 5_000),
    payloadMb: clamp(spec.payloadMb ?? base.payloadMb, 0, 64),
    deltasPerSec: clamp(spec.deltasPerSec ?? base.deltasPerSec, 0, 500),
  };
}

function readRawSpec(): string | null {
  try {
    const param = new URLSearchParams(location.search).get("fixture");
    if (param) return param;
  } catch {
    /* no location (non-browser host) */
  }
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

/** Parse a preset name or an inline JSON object. Returns null for an absent
 *  or unparseable spec, which keeps the default seed. */
export function parseStressFixture(raw: string | null): StressFixture | null {
  if (!raw) return null;
  const trimmed = raw.trim();
  const preset = STRESS_PRESETS[trimmed.toLowerCase()];
  if (preset) return { ...preset };
  if (!trimmed.startsWith("{")) return null;
  try {
    const parsed = JSON.parse(trimmed) as Partial<StressFixture>;
    if (!parsed || typeof parsed !== "object") return null;
    return normalize(parsed);
  } catch {
    return null;
  }
}

let resolved: StressFixture | null | undefined;

/** The active fixture, or null when none is selected. Resolved once — the
 *  seed is built at module evaluation and must not change under it. */
export function getStressFixture(): StressFixture | null {
  if (resolved === undefined) resolved = parseStressFixture(readRawSpec());
  return resolved;
}

// ── Synthetic transcript ────────────────────────────────────────────────

/** One turn emits 5 envelopes: user_message, tool_use, tool_result,
 *  assistant_text, turn_completed. */
const EVENTS_PER_TURN = 5;

const TOOL_RESULT_LINES = [
  "  const distance = el.scrollHeight - el.scrollTop - el.clientHeight;",
  "  if (distance <= PIN_THRESHOLD_PX) anchorToTail();",
  "export function reuseTranscriptSlots(prev: Slot[], next: Slot[]): Slot[] {",
  "    // slot identity is preserved so memoized rows keep their fibers",
  "  return next.map((slot, i) => (equalSlot(prev[i], slot) ? prev[i] : slot));",
  "}",
];

/** Build a tool_result body of roughly `bytes` characters that still looks
 *  like real captured output — a uniform filler string would compress and
 *  parse unrealistically. */
function syntheticToolOutput(bytes: number, seed: number): string {
  if (bytes <= 0) return "";
  const parts: string[] = [];
  let size = 0;
  let line = seed;
  while (size < bytes) {
    const text = `${String(line).padStart(5, " ")}\t${TOOL_RESULT_LINES[line % TOOL_RESULT_LINES.length]}`;
    parts.push(text);
    size += text.length + 1;
    line += 1;
  }
  return parts.join("\n");
}

const transcriptCache = new Map<string, string[]>();

/**
 * The persisted-payload list `agent_chat_list_messages` returns for a
 * generated thread: the same JSON envelope shapes the real backend stores,
 * sized so the total tool_result payload lands near `fixture.payloadMb`.
 */
export function stressChatTranscript(threadId: string, fixture: StressFixture): string[] {
  const cached = transcriptCache.get(threadId);
  if (cached) return cached;

  const turns = Math.max(1, Math.ceil(fixture.chatEvents / EVENTS_PER_TURN));
  const totalPayloadBytes = fixture.payloadMb * 1024 * 1024;
  const bytesPerResult = Math.floor(totalPayloadBytes / turns);

  const out: string[] = [];
  const push = (envelope: unknown) => out.push(JSON.stringify(envelope));

  for (let i = 0; i < turns && out.length < fixture.chatEvents; i += 1) {
    const turnId = `${threadId}-turn-${i + 1}`;
    const toolUseId = `${threadId}-tu-${i + 1}`;
    push({
      type: "user_message",
      thread_id: threadId,
      text: `Turn ${i + 1}: trace the render path for the transcript window.`,
    });
    push({
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      item: {
        kind: "tool_use",
        tool_name: "Read",
        input: { file_path: `/src/components/chat/generated-${i}.ts` },
        tool_use_id: toolUseId,
      },
    });
    push({
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      item: {
        kind: "tool_result",
        tool_use_id: toolUseId,
        content: syntheticToolOutput(bytesPerResult, i),
        is_error: false,
      },
    });
    push({
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      item: {
        kind: "assistant_text",
        text: `(${i + 1}/${turns}) The window mounts only the rows intersecting the viewport; the rest stay measured but unmounted.`,
      },
    });
    push({
      type: "turn_completed",
      thread_id: threadId,
      turn_id: turnId,
      status: { kind: "success" },
      usage: null,
    });
  }

  const trimmed = out.slice(0, fixture.chatEvents);
  transcriptCache.set(threadId, trimmed);
  return trimmed;
}
