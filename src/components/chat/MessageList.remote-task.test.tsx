/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { taskFixture } from "@/lib/delegation.test-fixtures";
import { projectRemoteTasks } from './remote-task-projection';
import { buildTranscriptSlots } from './transcript-slots';

vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function List(props: Record<string, any>, ref) {
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => document.createElement("div"),
        getState: () => ({ isAtEnd: true, listen: () => () => undefined }),
        scrollToEnd: () => Promise.resolve(),
        scrollToIndex: () => Promise.resolve(),
      }));
      return <div>{props.data.map((item: any) => <div key={item.key}>{props.renderItem({ item })}</div>)}</div>;
    }),
  };
});
const { MessageList } = await import("./MessageList");
afterEach(cleanup);

describe("remote task transcript integration", () => {
  it('keeps the actual expanded live parent ActivityBlock and working slots when a tail receipt arrives', () => {
    const messages: ChatViewItem[] = [
      {kind:'user_message',id:'user',seq:0,text:'Parent work',created_at:Date.now()-5000},
      {kind:'tool_call',id:'read',seq:1,tool_use_id:'read',tool_name:'Read',input:{file_path:'/synthetic/a'},status:'done',result_content:null,approval_request_id:null},
      {kind:'tool_call',id:'live',seq:2,tool_use_id:'live',tool_name:'Bash',input:{command:'parent-only-command'},status:'running',result_content:null,approval_request_id:null},
    ];
    Object.assign(messages[2], {source_event_id:10}); // persisted hydration metadata is not a reducer field
    const props={streaming:true,onRespondToRequest:vi.fn(),onAcceptPlan:vi.fn(),onRejectPlan:vi.fn()};
    const view=render(<MessageList {...props} messages={messages} />);
    fireEvent.click(view.container.querySelector('button[aria-expanded="false"]')!);
    expect(view.getByRole('button',{name:/Work log/})).toHaveAttribute('aria-expanded','true');
    const projected=projectRemoteTasks(messages,[taskFixture({parent_event_id:10})]);
    view.rerender(<MessageList {...props} messages={projected} />);
    expect(view.getByRole('button',{name:/Work log/})).toHaveAttribute('aria-expanded','true');
    expect(view.getByText('parent-only-command')).toBeInTheDocument();
    expect(view.getByText('Build host')).toBeInTheDocument();
    expect(buildTranscriptSlots(projected,true).find(s=>s.body.kind==='activity')?.body).toMatchObject({working:true});
  });
  it('FEUI-2 keeps the same expanded root and step when multiple delayed receipts anchor inside live mechanical work', () => {
    const messages:ChatViewItem[]=[
      {kind:'user_message',id:'user',seq:0,text:'Parent work',created_at:Date.now()-5000},
      {kind:'tool_call',id:'read',seq:1,tool_use_id:'read',tool_name:'Read',input:{file_path:'/synthetic/retained-read'},status:'done',result_content:'parent read content',approval_request_id:null},
      {kind:'tool_call',id:'live',seq:2,tool_use_id:'live',tool_name:'Bash',input:{command:'parent-only-command'},status:'running',result_content:null,approval_request_id:null},
    ];
    Object.assign(messages[1],{source_event_id:9});Object.assign(messages[2],{source_event_id:10});
    const props={streaming:true,onRespondToRequest:vi.fn(),onAcceptPlan:vi.fn(),onRejectPlan:vi.fn()};
    const view=render(<MessageList {...props} messages={messages}/>);
    fireEvent.click(view.container.querySelector('button[aria-expanded="false"]')!);
    const workLog=view.getByRole('button',{name:/Work log/});const root=workLog.parentElement;
    const step=view.getByRole('button',{name:/parent-only-command/});fireEvent.click(step);
    expect(step).toHaveAttribute('aria-expanded','true');
    for(const tasks of [[taskFixture({parent_event_id:9})],[taskFixture({parent_event_id:9}),taskFixture({id:'task-2',parent_event_id:9,title:'Second delayed receipt'})]]) {
      const projected=projectRemoteTasks(messages,tasks);
      view.rerender(<MessageList {...props} messages={projected}/>);
      expect(view.getByRole('button',{name:/Work log/})).toBe(workLog);
      expect(workLog.parentElement).toBe(root);expect(workLog).toHaveAttribute('aria-expanded','true');
      expect(view.getByRole('button',{name:/parent-only-command/})).toBe(step);expect(step).toHaveAttribute('aria-expanded','true');
      expect(view.getByText('/synthetic/retained-read')).toBeInTheDocument();
      const slots=buildTranscriptSlots(projected,true);const activities=slots.filter(slot=>slot.body.kind==='activity');
      expect(activities).toHaveLength(1);expect(activities[0]).toMatchObject({key:'run:read',body:{working:true,items:[{id:'read'},{id:'live'}]}});
      expect(slots.filter(slot=>slot.body.kind==='item' && slot.body.item.kind==='remote_task').map(slot=>slot.key)).toEqual(tasks.map(task=>`remote:${task.id}`));
      expect(view.getAllByRole('button',{name:/View task/})).toHaveLength(tasks.length);
    }
  });
  it("renders the actual host-labelled card as a first-class transcript row", () => {
    const messages = [{ kind: "remote_task", id: "remote-task-1", seq: 1, task: taskFixture() }] as unknown as ChatViewItem[];
    const view = render(<MessageList messages={messages} onRespondToRequest={vi.fn()} onAcceptPlan={vi.fn()} onRejectPlan={vi.fn()} />);
    expect(view.getByText("Audit routes")).toBeInTheDocument();
    expect(view.getByText("Build host")).toBeInTheDocument();
    expect(view.getByRole("button", { name: /View task/ })).toBeInTheDocument();
  });
});
