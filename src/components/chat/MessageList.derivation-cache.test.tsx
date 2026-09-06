import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import * as references from "@/lib/agent-chat/reference-cwd";
import * as transcript from "./transcript-slots";
import * as controls from "./always-render-keys";

const { listPropsLog } = vi.hoisted(() => ({
  listPropsLog: { entries: [] as Record<string, any>[] },
}));
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
      listPropsLog.entries.push(props);
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => document.createElement("div"),
        getState: () => ({ isAtEnd: true, listen: () => () => undefined }),
        scrollToEnd: () => Promise.resolve(),
        scrollToIndex: () => Promise.resolve(),
      }));
      return <div data-slot="transcript-list" />;
    }),
  };
});
const { MessageList } = await import("./MessageList");
const handlers = {
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  listPropsLog.entries = [];
});

it("remounts an unchanged snapshot without sorting or rebuilding history derivations", () => {
  const messages: ChatViewItem[] = [
    { kind: "assistant_message", id: "a", seq: 2, text: "done", streaming: false, turn_id: "t" },
    { kind: "user_message", id: "u", seq: 1, text: "go" },
  ];
  const slice = vi.spyOn(messages, "slice");
  const builders = [
    vi.spyOn(transcript, "buildTranscriptSlots"),
    vi.spyOn(transcript, "reuseTranscriptSlots"),
    vi.spyOn(references, "assistantReferenceCwds"),
    vi.spyOn(references, "assistantReferencePaths"),
    vi.spyOn(controls, "deriveAlwaysRenderKeys"),
  ];
  const first = render(<MessageList messages={messages} {...handlers} />);
  const data = listPropsLog.entries[listPropsLog.entries.length - 1].data;
  expect(data.map((slot: transcript.TranscriptSlot) => slot.messageId)).toEqual(["u", "a"]);
  const calls = builders.map((builder) => builder.mock.calls.length);
  const sorts = slice.mock.calls.length;
  first.unmount();
  render(<MessageList messages={messages} {...handlers} />);
  expect(builders.map((builder) => builder.mock.calls.length)).toEqual(calls);
  expect(slice).toHaveBeenCalledTimes(sorts);
  expect(listPropsLog.entries[listPropsLog.entries.length - 1].data).toBe(data);
});

it("does not cache provider, workspace, cwd or action closures with the shared data", () => {
  const messages: ChatViewItem[] = [
    { kind: "assistant_message", id: "a", seq: 1, text: "done", streaming: false, turn_id: "t" },
  ];
  const first = render(<MessageList messages={messages} provider="claude" workspaceId="one" cwd="/one" {...handlers} />);
  const data = listPropsLog.entries[listPropsLog.entries.length - 1].data;
  first.unmount();
  const onAcceptPlan = vi.fn();
  const next = render(<MessageList messages={messages} provider="codex" workspaceId="two" cwd="/two" {...handlers} onAcceptPlan={onAcceptPlan} />);
  const props = listPropsLog.entries[listPropsLog.entries.length - 1];
  expect(props.data).toBe(data);
  expect(next.container.querySelector("[data-provider]")?.getAttribute("data-provider")).toBe("codex");
  const rowProps = props.renderItem({ item: data[0] }).props.children.props;
  expect(rowProps.workspaceId).toBe("two");
  expect(rowProps.cwd).toBe("/two");
  expect(rowProps.onAcceptPlan).toBe(onAcceptPlan);
});
