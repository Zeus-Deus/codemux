import { expect, it } from "vitest";
import { applyEvent, createEmptyThreadState } from "./reducer";
import { toStepView } from "@/components/chat/activity-steps";
import { hermesLearningLabel } from "./hermes-learning";
import type { ToolCallItem } from "./types";
it("marks missing Hermes completions unconfirmed, and accepts a later native result", () => {
  let state = applyEvent(createEmptyThreadState(), {type:"item_completed",thread_id:"t",turn_id:"turn",item:{kind:"tool_use",tool_name:"memory",tool_use_id:"m",input:{hermesAcp:true}}});
  state = applyEvent(state, {type:"turn_completed",thread_id:"t",turn_id:"turn",status:{kind:"success"},usage:null});
  let tool = state.messages.find(m => m.kind === "tool_call") as ToolCallItem;
  expect(tool.status).toBe("unconfirmed");
  expect(toStepView(tool).meta).toBe("outcome unconfirmed");
  expect(toStepView(tool).status).toBe("unconfirmed");
  expect(hermesLearningLabel(tool)).toBeNull();
  state = applyEvent(state, {type:"item_completed",thread_id:"t",turn_id:"turn",item:{kind:"tool_result",tool_use_id:"m",is_error:false,content:[{type:"content",content:{type:"text",text:"✅ Memory add saved (memory)"}}]}});
  tool = state.messages.find(m => m.kind === "tool_call") as ToolCallItem;
  expect(tool.status).toBe("done");
  expect(hermesLearningLabel(tool)).toBe("Memory saved");
});
it("does not infer non-Hermes tool outcomes at turn completion", () => {
  let state = applyEvent(createEmptyThreadState(), {type:"item_completed",thread_id:"t",turn_id:"turn",item:{kind:"tool_use",tool_name:"Bash",tool_use_id:"m",input:{}}});
  state = applyEvent(state, {type:"turn_completed",thread_id:"t",turn_id:"turn",status:{kind:"success"},usage:null});
  expect((state.messages.find(m => m.kind === "tool_call") as ToolCallItem).status).toBe("running");
});
