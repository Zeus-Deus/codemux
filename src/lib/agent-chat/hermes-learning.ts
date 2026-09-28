import type { ToolCallItem } from "./types";

/** Recognize the official ACP completion formatter, never assistant prose or a tool start.
 * Batch `manage` output omits per-operation success; it cannot certify creation/update.
 */
export function hermesLearningLabel(item: ToolCallItem): string | null {
  if (item.status !== "done" || !item.input || typeof item.input !== "object" || !("hermesAcp" in item.input)) return null;
  if (!Array.isArray(item.result_content)) return null;
  const report = item.result_content.map((v: unknown) => {
    if (!v || typeof v !== "object") return "";
    const value = v as { type?: string; content?: { type?: string; text?: string } };
    return value.type === "content" && value.content?.type === "text" ? value.content.text ?? "" : "";
  }).join("\n");
  if (item.tool_name === "memory" && /^✅ Memory (add|replace|remove) saved \(/.test(report)) return "Memory saved";
  if (item.tool_name === "skill_manage" && report.startsWith("**✅ Skill updated**")) {
    if (report.includes("- **Action:** `create`")) return "Skill created";
    if (/\*\*Action:\*\* `(patch|edit|write_file)`/.test(report)) return "Skill updated";
  }
  return null;
}
