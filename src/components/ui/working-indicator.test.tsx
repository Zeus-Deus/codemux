import { describe, it, expect, afterEach } from "vitest";
import { render, cleanup } from "@testing-library/react";

afterEach(cleanup);
import { WorkingIndicator } from "./working-indicator";
import type {
  WorkingIndicatorVariant,
  WorkingIndicatorColor,
} from "@/stores/settings-store";

const VARIANTS: WorkingIndicatorVariant[] = [
  "braille",
  "ring",
  "blink",
  "sweep",
  "typing",
];

describe("WorkingIndicator", () => {
  it("renders every variant without crashing and marks it as working", () => {
    for (const variant of VARIANTS) {
      const { container, getByLabelText, unmount } = render(
        <WorkingIndicator variant={variant} />,
      );
      // Each live variant announces itself as "Agent working".
      expect(getByLabelText("Agent working")).toBeInTheDocument();
      expect(container.firstChild).not.toBeNull();
      unmount();
    }
  });

  it("defaults to the braille spinner with the amber working token", () => {
    const { getByLabelText } = render(<WorkingIndicator />);
    const el = getByLabelText("Agent working");
    expect(el.className).toContain("text-status-working");
  });

  it("applies the text color token for text-based variants", () => {
    const { getByLabelText } = render(
      <WorkingIndicator variant="ring" color="accent-ember" />,
    );
    // Loader2 renders an <svg>, whose `className` is an SVGAnimatedString —
    // read the class attribute directly.
    expect(getByLabelText("Agent working").getAttribute("class")).toContain(
      "text-accent-ember",
    );
  });

  it("applies the bg color token for dot-based variants", () => {
    const { getByLabelText } = render(
      <WorkingIndicator variant="blink" color="status-open" />,
    );
    expect(getByLabelText("Agent working").className).toContain(
      "bg-status-open",
    );
  });

  it("uses a track tint + swept inner bar for the sweep variant", () => {
    const { getByLabelText } = render(
      <WorkingIndicator variant="sweep" color="status-remote" />,
    );
    const track = getByLabelText("Agent working");
    expect(track.className).toContain("bg-status-remote/20");
    const inner = track.querySelector(".cm-sweep");
    expect(inner).not.toBeNull();
    expect(inner!.className).toContain("bg-status-remote");
  });

  it("renders three staggered dots for the typing variant", () => {
    const { getByLabelText } = render(
      <WorkingIndicator variant="typing" color="accent-violet" />,
    );
    const dots = getByLabelText("Agent working").querySelectorAll(".cm-blink");
    expect(dots).toHaveLength(3);
    expect((dots[1] as HTMLElement).style.animationDelay).toBe("0.2s");
    expect(dots[0].className).toContain("bg-accent-violet");
  });

  it("is decorative (no working label) in preview mode", () => {
    const { queryByLabelText } = render(
      <WorkingIndicator variant="ring" preview />,
    );
    expect(queryByLabelText("Agent working")).toBeNull();
  });

  it.each<WorkingIndicatorColor>([
    "status-working",
    "foreground",
    "accent-ember",
    "status-open",
    "status-remote",
    "accent-violet",
  ])("supports the %s color token", (color) => {
    const { container, unmount } = render(
      <WorkingIndicator variant="blink" color={color} preview />,
    );
    expect(container.firstChild).not.toBeNull();
    unmount();
  });
});
