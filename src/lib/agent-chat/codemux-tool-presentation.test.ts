import { describe, expect, it } from "vitest";
import { codemuxToolPresentation, launchedChatId, orchestrationSummary } from "./codemux-tool-presentation";
import type { ToolCallItem } from "./types";
describe("orchestration tool rows",()=>{
 it("recognizes each provider prefix and readable verbs",()=>{for(const name of ["thread_launch","create_threads","thread_send","thread_read","thread_wait","thread_list","thread_search","thread_interrupt","orchestrator_capabilities","delegate_task","task_status","task_cancel"]){expect(codemuxToolPresentation(name)).not.toBeNull();expect(codemuxToolPresentation(`mcp__codemux__${name}`)).toEqual(codemuxToolPresentation(name));}expect(codemuxToolPresentation("unrelated")).toBeNull();expect(codemuxToolPresentation("mcp__other__thread_send")).toBeNull();});
 it("presents launch progress and title",()=>{expect(orchestrationSummary({tool_name:"mcp__codemux__thread_launch",status:"running",input:{title:"Review"}} as ToolCallItem)).toEqual({verb:"Starting chat",target:"Review"});});
 it("extracts a started chat from both provider result envelopes",()=>{expect(launchedChatId([{type:"text",text:JSON.stringify({state:"accepted",thread_id:"new-chat"})}])).toBe("new-chat");expect(launchedChatId({success:true,contentItems:[{text:'{"thread_id":"codex-chat"}'}]})).toBe("codex-chat");expect(launchedChatId({state:"failed",thread_id:"failed-chat"})).toBeNull();});
});

it("presents Codex orchestration envelopes and rejects foreign server aliases", () => {
  const item = {tool_name:"dynamicToolCall",status:"running",input:{type:"dynamicToolCall",tool:"codemux_mcp__codemux__thread_launch",arguments:{title:"Codex launch"}}} as ToolCallItem;
  expect(orchestrationSummary(item)).toEqual({verb:"Starting chat",target:"Codex launch"});
  expect(orchestrationSummary({...item,input:{...item.input as object,tool:"codemux_mcp__other__thread_launch"}})).toBeNull();
});
