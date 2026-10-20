/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach,expect,it,vi } from "vitest";
import {cleanup,fireEvent,render,screen} from "@testing-library/react";
import {ChildOriginMarker} from "./ChildOriginMarker";
import {DurableTasksContext} from "./durable-task-context";
const {open}=vi.hoisted(()=>({open:vi.fn()}));vi.mock("./delegation-actions",()=>({openDelegatedChat:open}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
it("links a completed child's durable parent origin",()=>{
  render(<DurableTasksContext.Provider value={{tasks:[],origin:{relationship:"subagent",parent_thread_id:"parent",parent_title:"Build parser",parent_exists:true,task_id:"task"}}}><ChildOriginMarker/></DurableTasksContext.Provider>);
  expect(screen.getByText("Subagent of · Build parser")).toBeInTheDocument();fireEvent.click(screen.getByRole("button",{name:"Open parent chat"}));expect(open).toHaveBeenCalledWith("parent");
});
it("keeps the parent title without a broken action after deletion",()=>{
  render(<DurableTasksContext.Provider value={{tasks:[],origin:{relationship:"subagent",parent_thread_id:"gone",parent_title:"Old parent",parent_exists:false,task_id:"task"}}}><ChildOriginMarker/></DurableTasksContext.Provider>);
  expect(screen.getByText("Subagent of · Old parent")).toBeInTheDocument();expect(screen.queryByRole("button")).toBeNull();
});
it("does not infer lineage from message text",()=>{render(<ChildOriginMarker/>);expect(screen.queryByTestId("child-origin-marker")).toBeNull();});
