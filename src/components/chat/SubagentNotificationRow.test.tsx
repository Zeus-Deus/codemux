/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SubagentNotificationRow } from "./SubagentNotificationRow";
import { DurableTasksContext } from "./durable-task-context";
import { RemoteTaskContext } from "./remote-task-context";
import type { MessageNotification } from "@/tauri/events";
const { open } = vi.hoisted(() => ({open: vi.fn()}));
vi.mock("./delegation-actions", () => ({openDelegatedChat:open}));
afterEach(() => {cleanup(); vi.clearAllMocks();});
const notification: MessageNotification = {source:{kind:"delegated_task",task_ids:["task-1"],child_thread_id:"child-1"},outcome:"completed",summary:"Review parser"};
it("renders a structured result with an exact child link and no agent text disclosure", () => {
  render(<SubagentNotificationRow notification={notification} timestamp={1000} />);
  expect(screen.getByText("Review parser")).toBeInTheDocument();expect(screen.getByText("Finished")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button"));expect(open).toHaveBeenCalledWith("child-1");
  expect(screen.queryByText(/<task|<report|Codemux posted/)).toBeNull();expect(screen.queryByRole("button",{expanded:false})).toBeNull();
});
it("renders a remote result as a server row and opens RemoteTaskDetail", () => {
  const onOpen=vi.fn();
  render(<RemoteTaskContext.Provider value={{paneId:"pane",writable:false,onOpen}}><SubagentNotificationRow notification={{...notification,source:{kind:"remote_task",task_ids:["ssh:remote-1"]},outcome:"failed"}} /></RemoteTaskContext.Provider>);
  expect(screen.getByText("Failed")).toBeInTheDocument();fireEvent.click(screen.getByRole("button"));expect(onOpen).toHaveBeenCalledWith("remote-1");expect(open).not.toHaveBeenCalled();
});
it("looks up the durable task by task ID and shows the published report", () => {
  render(<DurableTasksContext.Provider value={{origin:null,tasks:[{task_id:"task-1",backend:"local",parent_thread_id:"parent",child_thread_id:"child-1",provider:"codex",title:"Review parser",status:"completed",result:{version:1,text:"Fixed `offset`.\nChecked 8 tests."},created_at:1}]}}><SubagentNotificationRow notification={notification} /></DurableTasksContext.Provider>);
  expect(screen.getByText("Fixed offset. Checked 8 tests.")).toBeInTheDocument();
});
