import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS, useSettingsStore } from "@/stores/settings-store";

import { AgentOrb } from "./agent-orb";

function setMatchActivity(on: boolean) {
  useSettingsStore.setState({
    loaded: true,
    settings: { "agents.orb_match_activity": on ? "true" : "false" },
  });
}

/** The rendered state is exposed as `data-orb-state` on the canvas. */
function orbState(): string | null {
  return screen.getByRole("img").getAttribute("data-orb-state");
}

describe("AgentOrb", () => {
  beforeEach(() => {
    useSettingsStore.setState({ loaded: true, settings: {} });
  });

  // Vitest runs without globals here, so testing-library's auto-cleanup
  // never registers; without this every render stacks up in one document.
  afterEach(cleanup);

  it("defaults to matching the activity", () => {
    expect(SETTINGS_DEFAULTS["agents.orb_match_activity"]).toBe("true");
    render(<AgentOrb toolName="Grep" />);
    expect(orbState()).toBe("searching");
  });

  it("follows the activity when the toggle is on", () => {
    setMatchActivity(true);
    render(<AgentOrb toolName="Write" />);
    expect(orbState()).toBe("composing");
  });

  it("pins every orb to working when the toggle is off", () => {
    setMatchActivity(false);
    const { rerender } = render(<AgentOrb toolName="Write" />);
    expect(orbState()).toBe("working");
    rerender(<AgentOrb toolName="Grep" />);
    expect(orbState()).toBe("working");
    rerender(<AgentOrb awaitingUser />);
    expect(orbState()).toBe("working");
    rerender(<AgentOrb queued />);
    expect(orbState()).toBe("working");
  });

  it("stays neutral when the caller offers no signal", () => {
    render(<AgentOrb />);
    expect(orbState()).toBe("working");
  });

  it("renders monochrome — no color or tint prop reaches the orb", () => {
    render(<AgentOrb toolName="Grep" />);
    const canvas = screen.getByRole("img");
    // The library inks from the theme alone. Anything that pinned a color
    // here would be a hardcoded color in a component.
    expect(canvas.getAttribute("tint")).toBeNull();
    expect(canvas.getAttribute("color")).toBeNull();
    expect(canvas.style.color).toBe("");
  });

  it("can be hidden from assistive tech where a row owns the status", () => {
    render(<AgentOrb toolName="Grep" aria-hidden />);
    // `getByRole` skips aria-hidden nodes, so query the canvas directly.
    const canvas = document.querySelector("canvas")!;
    expect(canvas.getAttribute("aria-hidden")).toBe("true");
  });

  it("takes a stable label where the surface wants one", () => {
    render(<AgentOrb aria-label="Agent working" />);
    expect(screen.getByLabelText("Agent working")).toBeInTheDocument();
  });
});
