/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { useUIStore } from "@/stores/ui-store";

import { TurnChangesChip } from "./TurnChangesChip";

const changes = {
  files: [
    { path: "/repo/src/a.ts", added: 40, removed: 7 },
    { path: "/repo/src/b.ts", added: 2, removed: 0 },
  ],
  added: 42,
  removed: 7,
};

describe("TurnChangesChip", () => {
  const initialState = useUIStore.getState();
  beforeEach(() => {
    useUIStore.setState(initialState, true);
  });
  afterEach(cleanup);

  it("summarises the turn and opens the Changes pane", () => {
    render(<TurnChangesChip changes={changes} workspaceId="ws-1" cwd="/repo" />);
    const chip = screen.getByTestId("turn-changes-chip");
    expect(chip).toHaveTextContent("2 files changed");
    expect(chip).toHaveTextContent("+42 −7");

    fireEvent.click(chip);
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
  });

  it("uses the singular for one file", () => {
    render(
      <TurnChangesChip
        changes={{ files: [changes.files[1]], added: 2, removed: 0 }}
        workspaceId="ws-1"
      />,
    );
    expect(screen.getByTestId("turn-changes-chip")).toHaveTextContent(
      "1 file changed",
    );
  });
});
