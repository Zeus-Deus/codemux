import { describe, expect, it } from "vitest";
import { workflowGoalWithContext } from "./launch-chat-workflow";



describe("workflowGoalWithContext", () => {
  it("appends resolved context after the typed goal", () => {
    expect(workflowGoalWithContext("review auth", null)).toBe("review auth");
    expect(workflowGoalWithContext("review auth", "  ")).toBe("review auth");
    expect(workflowGoalWithContext("review auth", "=== Attached context ===\nfile")).toBe("review auth\n\n=== Attached context ===\nfile");
  });

  it("refuses context beyond the engine's 64 KB goal limit instead of truncating it", () => {
    expect(() => workflowGoalWithContext("review", "x".repeat(70_000))).toThrow("too large for a workflow");
  });
});
