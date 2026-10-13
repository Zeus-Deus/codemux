import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

import type { AssistantMessageItem } from "@/lib/agent-chat/types";

vi.mock("./ChatMarkdown", () => ({
  ChatMarkdown: ({
    children,
    streaming,
  }: {
    children: ReactNode;
    streaming?: boolean;
  }) => <span data-streaming={streaming}>{children}</span>,
}));

import { AssistantMessage } from "./AssistantMessage";

function item(text: string, streaming = true): AssistantMessageItem {
  return {
    kind: "assistant_message",
    id: "assistant-1",
    seq: 1,
    turn_id: "turn-1",
    text,
    streaming,
  };
}

describe("AssistantMessage", () => {
  it.each(["Still working", ""])(
    "does not append a typing cursor to streaming prose %#",
    (text) => {
      const { container } = render(<AssistantMessage item={item(text)} />);
      const root = container.firstElementChild;

      expect(root?.children).toHaveLength(1);
      expect(root?.querySelector("[data-streaming='true']")).not.toBeNull();
      expect(root?.querySelector("[aria-hidden]")).toBeNull();
    },
  );

  it("reserves the copy strip only for a final answer, not commentary", () => {
    const answer = render(<AssistantMessage item={item("Done", false)} />);
    expect(answer.queryByTestId("message-copy-button")).not.toBeNull();
    answer.unmount();

    const commentary = render(
      <AssistantMessage item={item("Checking files", false)} commentary />,
    );
    expect(commentary.queryByTestId("message-copy-button")).toBeNull();
  });
});
