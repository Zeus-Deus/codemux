import type { CompletedItem } from "@/tauri/events";

type ToolResult = Extract<CompletedItem, { kind: "tool_result" }>;
type ToolResultStatus = "done" | "error" | "unconfirmed";

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Old Codex results retained the native item but hardcoded is_error=false.
 *  Read only identifiable native envelopes; stdout-only command history and
 *  other providers' results still use the canonical flag. Keep in sync with
 *  codex/tool_outcome.rs and the shared outcome fixtures. */
export function toolResultStatus(item: ToolResult): ToolResultStatus {
  if (item.is_error) return "error";
  const native = item.content;
  if (!isRecord(native)) return "done";
  const evidence = native.__codemux_codex_tool_outcome;
  if (isRecord(native.__codemux_lazy_tool_result) && isRecord(evidence)
    && evidence.tool_use_id === item.tool_use_id
    && (evidence.status === "done" || evidence.status === "error" || evidence.status === "unconfirmed")) {
    return evidence.status;
  }
  if (native.id !== item.tool_use_id) return "done";
  const type = native.type;
  if (type !== "commandExecution" && type !== "fileChange"
    && type !== "dynamicToolCall" && type !== "mcpToolCall") return "done";

  const status = native.status;
  if (status === "failed") return "error";
  if ((type === "commandExecution" || type === "fileChange") && status === "declined") {
    // Codex maps both approval rejection and cancellation to declined.
    return "error";
  }
  if (type === "commandExecution" && typeof native.exitCode === "number"
    && Number.isInteger(native.exitCode) && native.exitCode !== 0) return "error";
  if (type === "dynamicToolCall" && native.success === false) return "error";
  if (type === "mcpToolCall" && (native.error != null
    || (isRecord(native.result) && native.result.isError === true))) return "error";

  if (status === "completed") return "done";
  // A future or nonterminal status is not proof of success, even if another
  // field happens to contain a successful value.
  if (status != null) return "unconfirmed";
  if (type === "commandExecution" && native.exitCode === 0) return "done";
  if (type === "dynamicToolCall" && native.success === true) return "done";
  if (type === "mcpToolCall" && isRecord(native.result) && native.result.isError === false) {
    return "done";
  }
  return "unconfirmed";
}
