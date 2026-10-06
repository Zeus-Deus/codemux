/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ElementInfo } from "./inspector";
import { InspectorPanel } from "./InspectorPanel";

const element: ElementInfo = {
  tag: "button",
  id: "save",
  classes: ["primary"],
  text: "Save",
  selector: "#save",
  rect: { x: 0, y: 0, width: 10, height: 10 },
};

afterEach(() => cleanup());

describe("InspectorPanel", () => {
  it("sends the element to the named agent", async () => {
    const onTellAgent = vi.fn();
    render(
      <InspectorPanel
        element={element}
        agentTargetTitle="Claude"
        onDismiss={() => {}}
        onTellAgent={onTellAgent}
      />,
    );
    const send = screen.getByRole("button", { name: "Send to agent" });
    expect(send.parentElement).toHaveAttribute("title", "Send to agent (Claude)");
    await userEvent.click(send);
    expect(onTellAgent).toHaveBeenCalledWith(element);
  });

  it("disables sending when the workspace has no agent", () => {
    render(
      <InspectorPanel
        element={element}
        agentTargetTitle={null}
        onDismiss={() => {}}
        onTellAgent={() => {}}
      />,
    );
    const send = screen.getByRole("button", { name: "Send to agent" });
    expect(send).toBeDisabled();
    expect(send.parentElement).toHaveAttribute("title", "No agent in this workspace");
  });
});
