import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeliveryAttention } from "./DeliveryAttention";
const api=vi.hoisted(()=>({list:vi.fn(),ack:vi.fn(),retry:vi.fn(),subscribe:vi.fn()}));
vi.mock("@/tauri/commands",()=>({agentChatDeliveryAttention:api.list,agentChatAcknowledgeDelivery:api.ack,agentChatRetryDelivery:api.retry}));
vi.mock("@/tauri/events",()=>({onAgentChatDeliveryChanged:api.subscribe}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
api.subscribe.mockResolvedValue(() => {});
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
  it("refreshes only for this thread's delivery transitions and unsubscribes",async()=>{
    let changed: ((thread: string) => void) | undefined;
    const stop = vi.fn();
    api.subscribe.mockImplementation(async (cb) => { changed = cb; return stop; });
    api.list.mockResolvedValue([]);
    const mounted = render(<DeliveryAttention threadId="parent"/>);
    await act(async()=>{});
    const mountedReads = api.list.mock.calls.length;
    api.list.mockResolvedValue([unknown]);
    await act(async()=>{ changed?.("other"); });
    expect(api.list).toHaveBeenCalledTimes(mountedReads);
    await act(async()=>{ changed?.("parent"); });
    await screen.findByText("Review finished");
    expect(api.list).toHaveBeenCalledTimes(mountedReads + 1);
    mounted.unmount(); expect(stop).toHaveBeenCalledOnce();
  });

});
