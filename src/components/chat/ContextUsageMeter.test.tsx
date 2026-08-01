/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";

import type { ContextUsageSnapshot } from "@/tauri/events";

import { ContextUsageMeter } from "./ContextUsageMeter";

type MeterProps = ComponentProps<typeof ContextUsageMeter>;

afterEach(() => cleanup());

function baseProps(overrides: Partial<MeterProps> = {}): MeterProps {
  return {
    usage: { used_tokens: 44_000, max_tokens: 200_000 },
    ...overrides,
  };
}

function snapshot(
  overrides: Partial<ContextUsageSnapshot> = {},
): ContextUsageSnapshot {
  return { used_tokens: 44_000, max_tokens: 200_000, ...overrides };
}

/** Render, then click the trigger so the popover contents mount. */
function open(props: MeterProps) {
  render(<ContextUsageMeter {...props} />);
  fireEvent.click(screen.getByTestId("context-usage-trigger"));
}

describe("ContextUsageMeter", () => {
  it("renders nothing without a usage snapshot", () => {
    const { container } = render(
      <ContextUsageMeter {...baseProps({ usage: null })} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByTestId("context-usage-trigger")).toBeNull();
  });

  it("labels the trigger with the percentage when the window is known", () => {
    render(<ContextUsageMeter {...baseProps()} />);
    expect(screen.getByTestId("context-usage-trigger")).toHaveAttribute(
      "aria-label",
      "Context window 22% used",
    );
  });

  it("shows percent, ratio, and a progressbar for a known window", () => {
    open(baseProps());
    expect(screen.getByText("Context Window")).toBeInTheDocument();
    expect(screen.getByTestId("context-usage-readout")).toHaveTextContent(
      "22% · 44k/200k",
    );
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "22");
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
  });

  it("falls back to a bare token count when the window is unknown", () => {
    const props = baseProps({
      usage: snapshot({ max_tokens: null }),
      seedMaxTokens: null,
    });
    open(props);
    expect(screen.getByTestId("context-usage-readout")).toHaveTextContent(
      "44k",
    );
    expect(screen.getByTestId("context-usage-readout")).not.toHaveTextContent(
      "%",
    );
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByTestId("context-usage-trigger")).toHaveAttribute(
      "aria-label",
      "Context window 44k tokens used",
    );
  });

  it("uses the capability seed until the provider reports a window", () => {
    open(
      baseProps({
        usage: snapshot({ max_tokens: null }),
        seedMaxTokens: 200_000,
      }),
    );
    expect(screen.getByTestId("context-usage-readout")).toHaveTextContent(
      "22% · 44k/200k",
    );
  });

  it("shows the total-processed row only once it exceeds live usage", () => {
    open(baseProps({ usage: snapshot({ total_processed_tokens: 91_000 }) }));
    const row = screen.getByTestId("context-usage-total-processed");
    expect(row).toHaveTextContent("Total processed");
    expect(row).toHaveTextContent("91k");
  });

  it("hides the total-processed row when it does not exceed live usage", () => {
    open(baseProps({ usage: snapshot({ total_processed_tokens: 44_000 }) }));
    expect(screen.queryByTestId("context-usage-total-processed")).toBeNull();
  });

  it("hides the total-processed row when the provider omits it", () => {
    open(baseProps());
    expect(screen.queryByTestId("context-usage-total-processed")).toBeNull();
  });

  it("notes auto-compaction with the provider's name when flagged", () => {
    open(
      baseProps({
        usage: snapshot({ compacts_automatically: true }),
        providerLabel: "Claude",
      }),
    );
    expect(
      screen.getByText(/Claude automatically compacts its context when needed/),
    ).toBeInTheDocument();
  });

  it("falls back to a generic subject when the provider is unknown", () => {
    open(baseProps({ usage: snapshot({ compacts_automatically: true }) }));
    expect(
      screen.getByText(
        /The agent automatically compacts its context when needed/,
      ),
    ).toBeInTheDocument();
  });

  it("omits the compaction note unless the flag is set", () => {
    open(baseProps({ providerLabel: "Claude" }));
    expect(screen.queryByText(/automatically compacts/)).toBeNull();
  });

  it("switches to the danger token past the warning threshold", () => {
    open(baseProps({ usage: snapshot({ used_tokens: 190_000 }) }));
    expect(screen.getByTestId("context-usage-trigger").className).toContain(
      "text-danger",
    );
    expect(screen.getByRole("progressbar").firstElementChild?.className).toContain(
      "bg-danger/80",
    );
  });

  it("stays neutral at or below the warning threshold", () => {
    // Exactly 90% is not yet a warning.
    open(baseProps({ usage: snapshot({ used_tokens: 180_000 }) }));
    expect(screen.getByTestId("context-usage-trigger").className).not.toContain(
      "text-danger",
    );
    expect(screen.getByRole("progressbar").firstElementChild?.className).toContain(
      "bg-foreground/40",
    );
  });
});
