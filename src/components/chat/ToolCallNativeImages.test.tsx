/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import nativeImageResultRaw from "@/lib/agent-chat/__fixtures__/opencode-native-image-tool-result.json?raw";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import { ToolCallBody } from "./ToolCallBodies";

afterEach(cleanup);

it("renders a native image result as a thumbnail and lightbox without dumping its data URI", async () => {
  const fixture: { data: { content: unknown[] } } =
    JSON.parse(nativeImageResultRaw);
  const item: ToolCallItem = {
    kind: "tool_call",
    id: "native-read",
    seq: 1,
    tool_use_id: "tool_read_png",
    tool_name: "read",
    input: { path: "image.png" },
    status: "done",
    result_content: fixture.data.content,
    approval_request_id: null,
  };
  const { container } = render(<ToolCallBody item={item} />);
  expect(screen.getByText("Read image")).toBeInTheDocument();
  expect(screen.getByRole("img")).toHaveAttribute(
    "src",
    expect.stringMatching(/^data:image\/png;base64,/),
  );
  expect(container.textContent).not.toContain("base64");
  await userEvent.click(screen.getByRole("button", { name: "Open image" }));
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(within(screen.getByRole("dialog")).getByRole("img")).toHaveAttribute(
    "src",
    expect.stringMatching(/^data:image\/png;base64,/),
  );
});
