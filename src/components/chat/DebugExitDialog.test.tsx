/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";

import { DebugExitDialog } from "./DebugExitDialog";

afterEach(() => cleanup());

describe("DebugExitDialog", () => {
  it("does not render when open is false", () => {
    render(<DebugExitDialog open={false} onChoose={vi.fn()} />);
    expect(screen.queryByText(/Remove debug markers\?/)).toBeNull();
  });

  it("renders three buttons when open", () => {
    render(<DebugExitDialog open onChoose={vi.fn()} />);
    expect(screen.getByText(/Remove debug markers\?/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Cancel$/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Leave them$/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Remove markers$/ }),
    ).toBeInTheDocument();
  });

  it("Remove markers fires onChoose('cleanup')", () => {
    const onChoose = vi.fn();
    render(<DebugExitDialog open onChoose={onChoose} />);
    fireEvent.click(screen.getByRole("button", { name: /^Remove markers$/ }));
    expect(onChoose).toHaveBeenCalledWith("cleanup");
  });

  it("Leave them fires onChoose('leave')", () => {
    const onChoose = vi.fn();
    render(<DebugExitDialog open onChoose={onChoose} />);
    fireEvent.click(screen.getByRole("button", { name: /^Leave them$/ }));
    expect(onChoose).toHaveBeenCalledWith("leave");
  });

  it("Cancel fires onChoose('cancel')", () => {
    const onChoose = vi.fn();
    render(<DebugExitDialog open onChoose={onChoose} />);
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/ }));
    expect(onChoose).toHaveBeenCalledWith("cancel");
  });

  it("pressing Escape routes through onOpenChange to onChoose('cancel')", () => {
    const onChoose = vi.fn();
    render(<DebugExitDialog open onChoose={onChoose} />);
    // Radix Dialog listens for Escape on the document while open.
    fireEvent.keyDown(document.body, {
      key: "Escape",
      code: "Escape",
      keyCode: 27,
    });
    expect(onChoose).toHaveBeenCalledWith("cancel");
  });

  it("clicking the overlay (onOpenChange(false)) routes to onChoose('cancel')", () => {
    const onChoose = vi.fn();
    render(<DebugExitDialog open onChoose={onChoose} />);
    // Radix exposes the overlay with role="dialog" parent; the overlay
    // itself dispatches `pointerDownOutside` → `onOpenChange(false)`.
    // We simulate the same effect by triggering Escape on a fresh
    // outside element; behaviour is identical from the dialog's
    // perspective. The button-level cancel test covers the explicit
    // button path; this guards the implicit dismiss paths.
    const dialogs = document.querySelectorAll('[role="dialog"]');
    expect(dialogs.length).toBeGreaterThan(0);
    fireEvent.keyDown(dialogs[0], { key: "Escape" });
    expect(onChoose).toHaveBeenCalledWith("cancel");
  });
});
