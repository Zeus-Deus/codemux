import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeliveryAttention } from "./DeliveryAttention";
const api=vi.hoisted(()=>({list:vi.fn(),ack:vi.fn(),retry:vi.fn()}));
vi.mock("@/tauri/commands",()=>({agentChatDeliveryAttention:api.list,agentChatAcknowledgeDelivery:api.ack,agentChatRetryDelivery:api.retry}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
const unknown={id:"delivery",message_id:"message",state:"held_unknown",reason:"Provider acceptance could not be confirmed after restart",display_text:"Review finished",can_retry:false};
describe("DeliveryAttention",()=>{
  it("hydrates durable attention and persists acknowledgement across remount",async()=>{
    let rows=[unknown]; api.list.mockImplementation(async()=>rows);
    api.ack.mockImplementation(async()=>{rows=[];});
    const mounted=render(<DeliveryAttention threadId="parent"/>);
    await screen.findByText("Review finished");
    expect(screen.queryByRole("button",{name:"Retry"})).toBeNull();
    fireEvent.click(screen.getByRole("button",{name:"Acknowledge"}));
    await waitFor(()=>expect(screen.queryByText("Review finished")).toBeNull());
    expect(api.ack).toHaveBeenCalledWith("parent","delivery");
    mounted.unmount(); render(<DeliveryAttention threadId="parent"/>);
    await act(async()=>{});
    expect(screen.queryByRole("region",{name:"Message delivery attention"})).toBeNull();
  });
  it("allows explicit retry only with definite no-send evidence",async()=>{
    api.list.mockResolvedValue([{...unknown,state:"rejected",can_retry:true}]);api.retry.mockResolvedValue(undefined);
    render(<DeliveryAttention threadId="parent"/>);
    fireEvent.click(await screen.findByRole("button",{name:"Retry"}));
    await waitFor(()=>expect(api.retry).toHaveBeenCalledWith("parent","delivery"));
    await waitFor(()=>expect(screen.queryByText("Review finished")).toBeNull());
  });
});
