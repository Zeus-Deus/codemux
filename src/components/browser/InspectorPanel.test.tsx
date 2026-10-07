/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ElementInfo } from "./inspector";
import { InspectorPanel } from "./InspectorPanel";
import { TooltipProvider } from "@/components/ui/tooltip";

const element: ElementInfo = {
  tag: "button",
  id: "save",
  classes: ["primary"],
  text: "Save",
  selector: "#save",
  rect: { x: 0, y: 0, width: 10, height: 10 },
};

afterEach(() => cleanup());

const renderPanel = (ui: React.ReactElement) => render(<TooltipProvider>{ui}</TooltipProvider>);

describe("InspectorPanel", () => {
  it("sends the element to the named agent", async () => {
    const onTellAgent = vi.fn();
    renderPanel(
      <InspectorPanel
        element={element}
        agentTargetTitle="Claude"
        onDismiss={() => {}}
        onTellAgent={onTellAgent}
      />,
    );
    const send = screen.getByRole("button", { name: "Send to agent" });
    expect(send).toHaveTextContent("Send to agent");
    await userEvent.click(send);
    expect(onTellAgent).toHaveBeenCalledWith(element);
  });

  it("disables sending when the workspace has no agent", () => {
    renderPanel(
      <InspectorPanel
        element={element}
        agentTargetTitle={null}
        onDismiss={() => {}}
        onTellAgent={() => {}}
      />,
    );
    const send = screen.getByRole("button", { name: "Send to agent" });
    expect(send).toBeDisabled();
    // The reason is readable without hovering: the disabled button points at it.
    expect(send).toHaveAccessibleDescription("No agent in this workspace");
  });
});
