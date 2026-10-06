/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { announceInterfaceSize, InterfaceSizeHud, interfaceSizeLabel } from "./interface-size-hud";
import { TYPOGRAPHY_DEFAULTS, TYPOGRAPHY_RANGES } from "@/lib/typography";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("interfaceSizeLabel", () => {
  it("names the size, the default, and either end of the range", () => {
    expect(interfaceSizeLabel({ size: 17, atLimit: null })).toBe("Interface 17px");
    expect(interfaceSizeLabel({ size: TYPOGRAPHY_DEFAULTS.interfaceSize, atLimit: null })).toBe(
      `Interface ${TYPOGRAPHY_DEFAULTS.interfaceSize}px · default`,
    );
    expect(interfaceSizeLabel({ size: 22, atLimit: "max" })).toBe("Maximum size · 22px");
    expect(interfaceSizeLabel({ size: 12, atLimit: "min" })).toBe("Minimum size · 12px");
  });
});

describe("InterfaceSizeHud", () => {
  it("answers a press at the limit, then fades after the hold", () => {
    vi.useFakeTimers();
    render(<InterfaceSizeHud />);
    const { max } = TYPOGRAPHY_RANGES.interface;
    act(() => announceInterfaceSize(max, "in", false));
    const hud = screen.getByTestId("interface-size-hud");
    expect(hud).toHaveTextContent(`Maximum size · ${max}px`);
    expect(hud).toHaveAttribute("data-visible", "true");
    act(() => vi.advanceTimersByTime(1_000));
    expect(hud).toHaveAttribute("data-visible", "false");
  });

  it("restarts the hold on every press instead of stacking", () => {
    vi.useFakeTimers();
    render(<InterfaceSizeHud />);
    act(() => announceInterfaceSize(17, "in", true));
    act(() => vi.advanceTimersByTime(600));
    act(() => announceInterfaceSize(18, "in", true));
    act(() => vi.advanceTimersByTime(600));
    expect(screen.getAllByTestId("interface-size-hud")).toHaveLength(1);
    expect(screen.getByTestId("interface-size-hud")).toHaveAttribute("data-visible", "true");
    expect(screen.getByTestId("interface-size-hud")).toHaveTextContent("Interface 18px");
  });
});
