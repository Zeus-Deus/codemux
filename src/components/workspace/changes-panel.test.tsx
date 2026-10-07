/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";

import { SmartCommitButton } from "./changes-panel";

function renderButton(over: Partial<ComponentProps<typeof SmartCommitButton>> = {}) {
  const props: ComponentProps<typeof SmartCommitButton> = {
    hasChanges: false,
    staged: 0,
    isGenerating: false,
    isMerging: false,
    busy: null,
    justDone: null,
    onCommit: vi.fn(),
    onCommitAndPush: vi.fn(),
    onAmend: vi.fn(),
    onUndoLastCommit: vi.fn(),
    onStashPush: vi.fn(),
    onStashPop: vi.fn(),
    onPush: vi.fn(),
    onPull: vi.fn(),
    onSync: vi.fn(),
    onFetch: vi.fn(),
    ahead: 2,
    behind: 0,
    ...over,
  };
  return render(<SmartCommitButton {...props} />);
}

afterEach(cleanup);

describe("SmartCommitButton", () => {
  it("says Push when ahead and idle", () => {
    renderButton();
    expect(screen.getByRole("button", { name: "Push 2" })).toBeEnabled();
  });

  it("says what is running while a push is in flight", () => {
    renderButton({ busy: "push" });
    const button = screen.getByRole("button", { name: "Pushing…" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("aria-busy", "true");
  });

  it("names an action started from the menu, not the primary's own", () => {
    renderButton({ busy: "pull" });
    expect(screen.getByRole("button", { name: "Pulling…" })).toBeInTheDocument();
  });

  it("confirms a finished push on the button", () => {
    renderButton({ justDone: "push", ahead: 0 });
    expect(screen.getByRole("button", { name: "Pushed" })).toBeInTheDocument();
  });

  it("says Amend with all changes when nothing is staged", async () => {
    const user = userEvent.setup();
    renderButton({ hasChanges: true, staged: 0 });
    await user.click(screen.getByRole("button", { name: "More actions" }));
    expect(await screen.findByText("Amend with all changes")).toBeInTheDocument();
  });
});
