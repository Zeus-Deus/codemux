/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import type { ComponentProps } from "react";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { taskFixture } from "@/lib/delegation.test-fixtures";

vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return { LegendList: React.forwardRef(function List(props: Record<string, any>, ref) {
    React.useImperativeHandle(ref, () => ({
      getScrollableNode: () => document.createElement("div"),
      getState: () => ({ isAtEnd: true, listen: () => () => undefined }),
      scrollToEnd: () => Promise.resolve(), scrollToIndex: () => Promise.resolve(),
    }));
    return <div>{props.data.map((item: any) => <div key={item.key}>{props.renderItem({ item })}</div>)}</div>;
  }) };
});
const { ChatTranscript } = await import("./ChatTranscript");
afterEach(cleanup);

describe("remote task action ownership", () => {
  it("keeps the task detail navigation bound to the current transcript scope", () => {
    const onOpen = vi.fn();
    const props = {
      messages: [{ kind: "remote_task", id: "remote-task-1", seq: 1, task: taskFixture() }] as ChatViewItem[],
      streaming: false, runtimeIntentAllowed: false,
      onRespondToRequest: vi.fn(), onAcceptPlan: vi.fn(), onRejectPlan: vi.fn(),
      remoteTaskScope: { paneId: "pane-1", writable: false, onOpen },
    } as ComponentProps<typeof ChatTranscript>;
    const view = render(<ChatTranscript {...props} />);
    fireEvent.click(view.getByRole("button", { name: /View task/ }));
    expect(onOpen).toHaveBeenCalledExactlyOnceWith("task-1");
  });
});
