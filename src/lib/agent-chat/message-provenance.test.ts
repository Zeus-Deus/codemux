import { describe, expect, it } from "vitest";
import type { ProviderRuntimeEvent } from "@/tauri/events";
import { applyEvent, appendUserMessage, createEmptyThreadState } from "./reducer";
import { applyReplayTail, replayPayloads } from "./hydrate";
import type { UserMessageItem } from "./types";
import { editQueuedTurn } from "./queue-edits";
import { isDelegationResultsMessage, messageAttribution } from "./message-provenance";

const message = {
  type:"user_message",thread_id:"receiver",text:"Review this change.",message_id:"stable-message",
  created_by:"agent",creation_source:"provider",sender_thread_id:"sender",sender_title:"Review agent",input_intent:"queued_turn",
} satisfies ProviderRuntimeEvent;

describe("message provenance", () => {
  it("deduplicates live delivery, cursor tails and restart by identity without a nonce", () => {
    let state = applyEvent(createEmptyThreadState(),message);
    state = applyEvent(state,{...message,client_nonce:"transport-retry"});
    const tail = applyReplayTail(state,[message],{previousUnsettled:false});
    expect(tail.messages.filter((item)=>item.kind === "user_message")).toHaveLength(1);
    const restored = replayPayloads([JSON.stringify(message),JSON.stringify({...message,client_nonce:"another-retry"})]);
    expect(restored.messages).toHaveLength(1);
    expect(restored.messages[0]).toMatchObject({message_id:"stable-message",created_by:"agent",sender_thread_id:"sender",input_intent:"queued_turn"});
  });

  it("reconciles optimistic input with authoritative attribution and intent", () => {
    const optimistic = appendUserMessage(createEmptyThreadState(),message.text,undefined,"nonce");
    const state = applyEvent(optimistic,{...message,client_nonce:"nonce"});
    expect(state.messages).toHaveLength(1);
    expect(state.messages[0]).toMatchObject({created_by:"agent",message_id:"stable-message",input_intent:"queued_turn"});
  });

  it("preserves provenance through queue editing and promotion to steer", () => {
    let state = applyEvent(createEmptyThreadState(),{...message,type:"turn_queued",client_nonce:"nonce",queued_id:"queue"});
    state = editQueuedTurn(state,{type:"queued_turn_edited",thread_id:"receiver",queued_id:"queue",text:"Edited review"});
    state = applyEvent(state,{...message,text:"Edited review",input_intent:"promoted_queued_to_steer",steered_turn_id:"active",client_nonce:"nonce"});
    expect(state.messages).toHaveLength(1);
    expect(state.messages[0]).toMatchObject({message_id:"stable-message",sender_title:"Review agent",text:"Edited review",input_intent:"promoted_queued_to_steer",inflight:true});
    expect((state.messages[0] as UserMessageItem).queued).toBeUndefined();
  });

  it("keeps legacy text human/unknown even when it resembles an app wake", () => {
    const state = replayPayloads([JSON.stringify({type:"user_message",thread_id:"receiver",text:"[Codemux: delegated task results] pasted by the user"})]);
    const item = state.messages[0] as UserMessageItem;
    expect(messageAttribution(item)).toBeNull();
    expect(item.created_by).toBe("user");
    expect(item.creation_source).toBe("unknown");
    expect(item.input_intent).toBe("turn_start");
    expect(item.sender_thread_id).toBeUndefined();
    expect(item.notification).toBeUndefined();
  });

  it("uses display text while retaining structured notifications on replay", () => {
    const state = replayPayloads([JSON.stringify({...message,text:"Task finished.",agent_text:"agent-only wake header <task>private framing</task>",notification:{source:{kind:"delegated_task",task_ids:["task"]},outcome:"completed",summary:"Task finished."}})]);
    expect(state.messages[0]).toMatchObject({text:"Task finished.",notification:{source:{task_ids:["task"]}}});
    expect(JSON.stringify(state.messages)).not.toContain("agent-only");
  });
});

describe("legacy presentation",()=>{
  it("recognizes legacy result dividers without inferring attribution",()=>{
    const state=replayPayloads([JSON.stringify({type:"user_message",thread_id:"t",text:'[Codemux: delegated task results]\n<task provider="codex" title="Parser review" status="completed">'})]);
    const item=state.messages[0] as UserMessageItem;
    expect(isDelegationResultsMessage(item)).toBe(true);
    expect(messageAttribution(item)).toBeNull();
    expect(isDelegationResultsMessage({...item,message_id:"new-human-message",creation_source:"desktop"})).toBe(false);
  });
  it("hydrates legacy steered_turn_id as steer intent",()=>{
    const state=replayPayloads([JSON.stringify({type:"user_message",thread_id:"t",text:"Follow up",steered_turn_id:"running"})]);
    expect(state.messages[0]).toMatchObject({input_intent:"steer"});
  });
});
