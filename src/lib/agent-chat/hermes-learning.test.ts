import { expect, it } from "vitest";
import { hermesLearningLabel } from "./hermes-learning";
import type { ToolCallItem } from "./types";
const tool = (name:string, report:string, status="done") => ({tool_name:name,status,input:{hermesAcp:true},result_content:[{type:"content",content:{type:"text",text:report}}]} as ToolCallItem);
it("only labels confirmed native foreground completions", () => {
  expect(hermesLearningLabel(tool("memory","✅ Memory add saved (memory)\nEntry added."))).toBe("Memory saved");
  expect(hermesLearningLabel(tool("skill_manage","**✅ Skill updated**\n- **Action:** `create`"))).toBe("Skill created");
  expect(hermesLearningLabel(tool("skill_manage","**✅ Skill updated**\n- **Action:** `patch`"))).toBe("Skill updated");
  expect(hermesLearningLabel(tool("skill_manage","**✅ Skill updated**\n- **Action:** `manage`"))).toBeNull();
  expect(hermesLearningLabel(tool("memory","I saved a memory."))).toBeNull();
  expect(hermesLearningLabel(tool("memory","✅ Memory add saved (memory)","error"))).toBeNull();
  expect(hermesLearningLabel({...tool("memory","✅ Memory add saved (memory)"),input:{}})).toBeNull();
});
