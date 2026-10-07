/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";

import { StatusIndicator } from "./status-indicator";

afterEach(() => cleanup());

function renderDot(status: Parameters<typeof StatusIndicator>[0]["status"]) {
  const { container } = render(
    <StatusIndicator status={status} withTooltip={false} />,
  );
  const root = container.querySelector(`[data-status="${status}"]`)!;
  return {
    ping: root.querySelector("[data-status-ping]"),
    dot: root.lastElementChild!,
  };
}

describe("StatusIndicator", () => {
  it("reserves the ping for an agent that needs the user", () => {
    const { ping, dot } = renderDot("permission");
    expect(ping).toHaveClass("motion-safe:animate-ping", "bg-status-attention");
    // The ring keeps it distinct in greyscale and with motion reduced.
    expect(dot).toHaveClass("bg-status-attention", "ring-2");
  });

  it("lets a working agent breathe instead of pinging", () => {
    const { ping, dot } = renderDot("working");
    expect(ping).toBeNull();
    expect(dot).toHaveClass("bg-status-working", "cm-breathe");
    expect(dot).not.toHaveClass("ring-2");
  });

  it.each(["monitoring", "review"] as const)("keeps %s steady", (status) => {
    const { ping, dot } = renderDot(status);
    expect(ping).toBeNull();
    expect(dot).not.toHaveClass("cm-breathe");
  });
});
