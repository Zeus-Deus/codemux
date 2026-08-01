/**
 * Lazy tool-result bodies.
 *
 * A cold thread can carry megabytes of tool output the user never
 * expands — a collapsed card renders 12 lines but used to hold (and
 * transfer, and parse) the entire body. On the cursor read path the
 * backend replaces an oversized `tool_result` `content` with the stub
 * below: enough metadata to render the collapsed card exactly as before,
 * plus the row id needed to fetch the real body on expand or copy.
 *
 * Stored rows are never rewritten — this is read-path shaping only, and
 * LIVE events always carry full content (they arrive over the channel,
 * not through a list read).
 *
 * Image-bearing results are never stubbed, so the thumbnail grid and the
 * auto-expand-on-image behaviour keep reading real content.
 */

/** Marker key. Mirrors `LAZY_TOOL_RESULT_KEY` in `commands/agent_chat.rs`. */
export const LAZY_TOOL_RESULT_KEY = "__codemux_lazy_tool_result";

export interface LazyToolResultStub {
  /** `agent_chat_messages.id` of the row holding the full payload. */
  row_id: number;
  /** Size of the serialized original `content`. */
  bytes: number;
  /** Leading slice of the stringified body — what the collapsed card
   *  renders. */
  preview: string;
  /** Line count of the FULL body, so "Show N more lines" stays honest. */
  line_count: number;
  has_images: boolean;
}

/** The wire shape of a stubbed `result_content`. */
export interface LazyToolResultEnvelope {
  [LAZY_TOOL_RESULT_KEY]: LazyToolResultStub;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Narrowing guard for a stubbed `result_content`. */
export function isLazyToolResultStub(
  content: unknown,
): content is LazyToolResultEnvelope {
  if (!isRecord(content)) return false;
  const stub = content[LAZY_TOOL_RESULT_KEY];
  if (!isRecord(stub)) return false;
  return (
    typeof stub.row_id === "number" &&
    typeof stub.bytes === "number" &&
    typeof stub.preview === "string" &&
    typeof stub.line_count === "number"
  );
}

/** The stub inside a `result_content`, or `null` when it is a real body. */
export function lazyToolResultStub(
  content: unknown,
): LazyToolResultStub | null {
  return isLazyToolResultStub(content)
    ? content[LAZY_TOOL_RESULT_KEY]
    : null;
}

/**
 * Pull the tool-result `content` out of a full row payload fetched with
 * `agent_chat_get_tool_result`. Returns `null` when the row is not an
 * `item_completed` carrying a `tool_result` (a deleted / rewritten row),
 * so the caller can surface a retry instead of rendering nothing.
 */
export function toolResultContentFromPayload(payload: string): unknown {
  let value: unknown;
  try {
    value = JSON.parse(payload);
  } catch {
    return null;
  }
  if (!isRecord(value) || value.type !== "item_completed") return null;
  const item = value.item;
  if (!isRecord(item) || item.kind !== "tool_result") return null;
  return item.content ?? null;
}

/** Human-readable size for the "load full output" affordance. */
export function formatLazyBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
