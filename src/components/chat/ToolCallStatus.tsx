import type { ToolCallItem } from "@/lib/agent-chat/types";

/**
 * One-line tool summary for the single tool card (design D6): `verb +
 * target + argument` in mono, args dimmed. Truncation is applied by the
 * parent card wrapper.
 */
export function ToolCallStatus({ item }: { item: ToolCallItem }) {
  const { verb, target, argument } = describeToolCall(item);

  return (
    <div className="font-mono text-[11.5px] leading-5 text-muted-foreground break-words">
      <span>{verb}</span>
      {target && (
        <>
          {" "}
          <span>{target}</span>
        </>
      )}
      {argument && (
        <>
          {" "}
          <span className="text-muted-foreground/60">{argument}</span>
        </>
      )}
    </div>
  );
}

export interface ToolDescription {
  verb: string;
  target: string | null;
  targetMono: boolean;
  argument: string | null;
}

// Map known tool names to natural-English verbs. Unknown tools fall
// back to "Called <tool_name>" so nothing is silently dropped. Extend
// as providers expose new tools.
const VERB_MAP: Record<string, string> = {
  Read: "Read",
  Write: "Wrote",
  Edit: "Edit",
  MultiEdit: "Edit",
  Bash: "Ran",
  Glob: "Searched",
  Grep: "Searched",
  WebFetch: "Fetched",
  WebSearch: "Searched",
  TodoWrite: "Updated tasks",
  NotebookEdit: "Edit",
};

export function describeToolCall(item: ToolCallItem): ToolDescription {
  const verb = VERB_MAP[item.tool_name] ?? `Called ${item.tool_name}`;
  const inputRecord = isRecord(item.input) ? item.input : {};
  const toolName = item.tool_name;

  switch (toolName) {
    case "Read": {
      const path = stringField(inputRecord, "file_path") ?? stringField(inputRecord, "path");
      const offset = numberField(inputRecord, "offset");
      const limit = numberField(inputRecord, "limit");
      return {
        verb,
        target: path,
        targetMono: true,
        argument: rangeLabel(offset, limit),
      };
    }
    case "Edit":
    case "MultiEdit":
    case "Write": {
      const path = stringField(inputRecord, "file_path") ?? stringField(inputRecord, "path");
      return { verb, target: path, targetMono: true, argument: null };
    }
    case "Bash": {
      const cmd = stringField(inputRecord, "command");
      const display = cmd ? truncate(cmd, 80) : null;
      return { verb, target: display, targetMono: true, argument: null };
    }
    case "Glob": {
      const pattern = stringField(inputRecord, "pattern");
      return { verb, target: pattern, targetMono: true, argument: null };
    }
    case "Grep": {
      const pattern = stringField(inputRecord, "pattern");
      const path = stringField(inputRecord, "path");
      return { verb, target: pattern, targetMono: true, argument: path };
    }
    case "WebFetch":
    case "WebSearch": {
      const url = stringField(inputRecord, "url") ?? stringField(inputRecord, "query");
      return { verb, target: url, targetMono: true, argument: null };
    }
    default: {
      // Unknown tool — show the first string argument as the target, if any.
      const firstStr = Object.values(inputRecord).find(
        (v) => typeof v === "string",
      ) as string | undefined;
      return {
        verb,
        target: firstStr ? truncate(firstStr, 80) : null,
        targetMono: false,
        argument: null,
      };
    }
  }
}

function rangeLabel(offset: number | null, limit: number | null): string | null {
  if (offset == null && limit == null) return null;
  const start = offset ?? 1;
  if (limit == null) return `L${start}+`;
  return `L${start}-${start + limit - 1}`;
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s;
  return `${s.slice(0, max - 1)}…`;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function stringField(obj: Record<string, unknown>, key: string): string | null {
  const v = obj[key];
  return typeof v === "string" ? v : null;
}

function numberField(obj: Record<string, unknown>, key: string): number | null {
  const v = obj[key];
  return typeof v === "number" ? v : null;
}
