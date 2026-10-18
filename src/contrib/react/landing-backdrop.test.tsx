import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { useEffect } from "react";
import { registry } from "../registry";
import { LANDING_BACKDROP_AREA } from "@/sdk/areas";
import type { LandingBackdropProps } from "@/sdk/host";
import { LandingBackdropHost } from "./landing-backdrop";

afterEach(cleanup);

function Landing() {
  return (
    <LandingBackdropHost>
      <main data-testid="landing">
        <div data-backdrop-calm>
          <h1>What should we do today?</h1>
        </div>
        <textarea aria-label="composer" />
        <button type="button">Send</button>
        <a href="#x">Link</a>
        <div role="menuitem">Row</div>
        <div data-testid="empty" style={{ height: 200 }} />
      </main>
    </LandingBackdropHost>
  );
}

describe("LandingBackdropHost", () => {
  it("adds no wrapper and no node without a contribution", () => {
    const { container } = render(<Landing />);
    expect(container.firstElementChild?.tagName).toBe("MAIN");
    expect(container.querySelector("[data-landing-backdrop-host]")).toBeNull();
  });

  it("forwards only left clicks on empty space, and exposes the calm rect", () => {
    const pulses: [number, number][] = [];
    let calm: (() => DOMRect | null) | null = null;
    function Backdrop({ onPulse, calmRect }: LandingBackdropProps) {
      calm = calmRect;
      useEffect(() => onPulse((x, y) => pulses.push([x, y])), [onPulse]);
      return <canvas data-testid="canvas" />;
    }
    let off = () => {};
    act(() => {
      off = registry.register({ id: "test:backdrop", area: LANDING_BACKDROP_AREA, render: Backdrop });
    });
    const { getByTestId, getByLabelText, getByText, container } = render(<Landing />);
    const layer = getByTestId("canvas").closest("[aria-hidden]") as HTMLElement;
    expect(layer.getAttribute("aria-hidden")).toBe("true");
    expect(layer.className).toContain("pointer-events-none");

    for (const el of [getByLabelText("composer"), getByText("Send"), getByText("Link"), getByText("Row")]) {
      fireEvent.pointerDown(el, { button: 0, clientX: 1, clientY: 1 });
    }
    fireEvent.pointerDown(getByTestId("empty"), { button: 2, clientX: 5, clientY: 5 });
    expect(pulses).toEqual([]);

    fireEvent.pointerDown(getByTestId("empty"), { button: 0, clientX: 40, clientY: 60 });
    expect(pulses).toEqual([[40, 60]]);
    expect(calm!()).not.toBeNull();

    // Turning the plugin off removes the wrapper again.
    act(() => off());
    expect(container.firstElementChild?.tagName).toBe("MAIN");
  });

  it("tells subscribers when the headline may have moved", () => {
    let calls = 0;
    function Backdrop({ onCalmChange }: LandingBackdropProps) {
      useEffect(() => onCalmChange(() => (calls += 1)), [onCalmChange]);
      return <canvas data-testid="calm-canvas" />;
    }
    let off = () => {};
    act(() => {
      off = registry.register({ id: "test:calm", area: LANDING_BACKDROP_AREA, render: Backdrop });
    });
    const { getByTestId } = render(<Landing />);
    // Scroll events from inside the landing bubble to the host in capture.
    fireEvent.scroll(getByTestId("landing"));
    expect(calls).toBe(1);
    act(() => off());
    fireEvent.scroll(getByTestId("landing"));
    expect(calls).toBe(1);
  });

  it("keeps the landing alive when a backdrop or a pulse handler throws", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    function Broken({ onPulse }: LandingBackdropProps) {
      useEffect(
        () =>
          onPulse(() => {
            throw new Error("pulse boom");
          }),
        [onPulse],
      );
      return <canvas data-testid="ok-canvas" />;
    }
    function Crashes(): never {
      throw new Error("render boom");
    }
    let off = () => {};
    act(() => {
      off = registry.registerMany([
        { id: "test:ok", area: LANDING_BACKDROP_AREA, render: Broken },
        { id: "test:crash", area: LANDING_BACKDROP_AREA, render: Crashes },
      ]);
    });
    const { getByTestId } = render(<Landing />);
    expect(getByTestId("ok-canvas")).toBeTruthy();
    expect(() => fireEvent.pointerDown(getByTestId("empty"), { button: 0 })).not.toThrow();
    expect(getByTestId("landing")).toBeTruthy();
    act(() => off());
    error.mockRestore();
  });
});
