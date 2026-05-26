/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";

import type { HostView } from "@/tauri/commands";
import {
  ConfirmPushDialog,
  dontAskAgainKey,
  shouldSkipPushConfirm,
} from "./confirm-push-dialog";

function makeHost(id: number, name = "homedesk"): HostView {
  return {
    id,
    server_id: `srv-${id}`,
    name,
    ssh_target: `${name}@example`,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
  };
}

afterEach(() => {
  cleanup();
  try {
    localStorage.clear();
  } catch {
    /* test env may lack localStorage */
  }
});

describe("ConfirmPushDialog", () => {
  it("renders the host name and workspace title in the headline + summary", () => {
    render(
      <ConfirmPushDialog
        open
        workspaceTitle="codemux/feature-x"
        host={makeHost(7, "homedesk")}
        onConfirm={() => {}}
        onOpenChange={() => {}}
      />,
    );
    expect(
      screen.getByText("Push to homedesk?"),
    ).toBeInTheDocument();
    expect(screen.getByText("codemux/feature-x")).toBeInTheDocument();
    // The "What this does" bullets describe the rsync + idle-local
    // semantics so the user knows what they're committing to.
    expect(screen.getByText(/Copies the workspace files/i)).toBeInTheDocument();
    expect(screen.getByText(/goes idle/i)).toBeInTheDocument();
    expect(screen.getByText(/Undo, for 10 seconds/i)).toBeInTheDocument();
  });

  it("invokes onConfirm and closes when 'Push' is clicked", () => {
    const onConfirm = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <ConfirmPushDialog
        open
        workspaceTitle="w"
        host={makeHost(7)}
        onConfirm={onConfirm}
        onOpenChange={onOpenChange}
      />,
    );
    fireEvent.click(screen.getByText("Push"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does NOT invoke onConfirm when Cancel is clicked", () => {
    const onConfirm = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <ConfirmPushDialog
        open
        workspaceTitle="w"
        host={makeHost(7)}
        onConfirm={onConfirm}
        onOpenChange={onOpenChange}
      />,
    );
    fireEvent.click(screen.getByText("Cancel"));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("persists the 'don't ask again' flag per-host on confirm when ticked", () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmPushDialog
        open
        workspaceTitle="w"
        host={makeHost(99)}
        onConfirm={onConfirm}
        onOpenChange={() => {}}
      />,
    );
    const checkbox = screen.getByLabelText(/Don't ask again/i);
    fireEvent.click(checkbox);
    fireEvent.click(screen.getByText("Push"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(dontAskAgainKey(99))).toBe("1");
    expect(shouldSkipPushConfirm(99)).toBe(true);
    // Other hosts unaffected.
    expect(shouldSkipPushConfirm(100)).toBe(false);
  });

  it("does NOT persist the flag when the user cancels", () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmPushDialog
        open
        workspaceTitle="w"
        host={makeHost(5)}
        onConfirm={onConfirm}
        onOpenChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByLabelText(/Don't ask again/i));
    fireEvent.click(screen.getByText("Cancel"));
    expect(shouldSkipPushConfirm(5)).toBe(false);
  });

  it("returns null when no host is supplied (the gate caller is mid-state)", () => {
    const { container } = render(
      <ConfirmPushDialog
        open
        workspaceTitle="w"
        host={null}
        onConfirm={() => {}}
        onOpenChange={() => {}}
      />,
    );
    expect(container.firstChild).toBeNull();
  });
});

describe("shouldSkipPushConfirm", () => {
  it("returns false when no flag is set", () => {
    expect(shouldSkipPushConfirm(1)).toBe(false);
  });

  it("returns true after the flag is set for that host id", () => {
    localStorage.setItem(dontAskAgainKey(1), "1");
    expect(shouldSkipPushConfirm(1)).toBe(true);
    // Other host ids unaffected.
    expect(shouldSkipPushConfirm(2)).toBe(false);
  });
});
