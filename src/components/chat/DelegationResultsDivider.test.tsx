/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import type { MessageNotification } from "@/tauri/events";
import { DelegationResultsDivider } from "./DelegationResultsDivider";
afterEach(cleanup);
const notification:MessageNotification={source:{kind:"delegated_task",task_ids:["task"]},outcome:"completed",summary:"Parser review finished"};
describe("DelegationResultsDivider",()=>{
  it("uses structured outcome metadata without parsing prompt text",()=>{
    const {getByRole,getByTestId}=render(<DelegationResultsDivider text="Parser review\nFocused checks passed." notification={notification} />);
    expect(getByRole("button")).toHaveTextContent("Delegated results · completed");
    expect(getByTestId("delegation-results-outcome").className).not.toContain("text-status-attention");
  });
  it("marks a structured failure with attention colour",()=>{
    const {getByTestId}=render(<DelegationResultsDivider text="Parser review failed." notification={{...notification,outcome:"failed"}} />);
    expect(getByTestId("delegation-results-outcome")).toHaveTextContent("failed");
    expect(getByTestId("delegation-results-outcome").className).toContain("text-status-attention");
  });
  it("expands the display text supplied by the backend",()=>{
    const text="Parser review\nFocused checks passed.";
    const {getByRole,getByTestId}=render(<DelegationResultsDivider text={text} notification={notification} />);
    fireEvent.click(getByRole("button"));
    expect(getByTestId("delegation-results-text").textContent).toBe(text);
    expect(getByTestId("delegation-results-text").className).toContain("max-h-[320px]");
  });
  it("keeps legacy provider logos and names while folding large reports",()=>{
    const text='[Codemux: delegated task results]\n<task provider="codex" title="Parser review" status="completed">\n<report>Done</report>\n</task>';
    const {getByRole,queryByTestId,container}=render(<DelegationResultsDivider text={text}/>);
    expect(getByRole("button")).toHaveTextContent("Codex");
    expect(container.querySelector("svg")).not.toBeNull();
    expect(queryByTestId("delegation-results-text")).toBeNull();
    fireEvent.click(getByRole("button"));
    expect(getByRole("button")).toHaveAttribute("aria-expanded","true");
  });
  it("preserves structured provider logos and task names",()=>{
    const {getByRole,container}=render(<DelegationResultsDivider text="Parser review\nDone" notification={{...notification,tasks:[{provider:"codex",title:"Parser review"}]}}/>);
    expect(getByRole("button")).toHaveTextContent("Codex");
    expect(getByRole("button")).toHaveTextContent("Parser review");
    expect(container.querySelector("svg")).not.toBeNull();
  });

});
