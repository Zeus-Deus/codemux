/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";

import { DebugCleanupBanner } from "./DebugCleanupBanner";

afterEach(() => cleanup());

describe("DebugCleanupBanner", () => {
  it("renders the banner copy and a Clean up button", () => {
    render(<DebugCleanupBanner onCleanup={vi.fn()} />);
    expect(
      screen.getByText(/Debug markers detected in this project\./),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Clean up/ }),
    ).toBeInTheDocument();
  });

  it("clicking Clean up fires onCleanup", () => {
    const onCleanup = vi.fn();
    render(<DebugCleanupBanner onCleanup={onCleanup} />);
    fireEvent.click(screen.getByRole("button", { name: /Clean up/ }));
    expect(onCleanup).toHaveBeenCalledOnce();
  });

  it("disables the button while busy and swaps the label", () => {
    const onCleanup = vi.fn();
    render(<DebugCleanupBanner onCleanup={onCleanup} busy />);
    const btn = screen.getByRole("button", { name: /Cleaning…/ });
    expect(btn).toBeDisabled();
    fireEvent.click(btn);
    expect(onCleanup).not.toHaveBeenCalled();
  });
});
