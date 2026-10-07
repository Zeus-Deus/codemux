/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { useReducedMotionConfig } from "motion/react";

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(async () => ({})),
  dbSetSetting: vi.fn(async () => undefined),
}));

import { useSettingsStore } from "@/stores/settings-store";
import { ReducedMotionProvider } from "./reduced-motion-provider";

function MotionProbe() {
  return <span>{useReducedMotionConfig() ? "reduced" : "full"}</span>;
}

beforeEach(() => {
  useSettingsStore.setState({ settings: {}, loaded: true });
});

afterEach(() => {
  cleanup();
  document.documentElement.classList.remove("reduce-motion");
});

describe("ReducedMotionProvider", () => {
  it("follows the OS preference by default", () => {
    render(
      <ReducedMotionProvider>
        <MotionProbe />
      </ReducedMotionProvider>,
    );
    expect(document.documentElement).not.toHaveClass("reduce-motion");
    expect(screen.getByText("full")).toBeInTheDocument();
  });

  it("forces reduced motion for CSS and motion/react when the setting is on", () => {
    render(
      <ReducedMotionProvider>
        <MotionProbe />
      </ReducedMotionProvider>,
    );
    act(() => {
      useSettingsStore.setState({
        settings: { "appearance.reduce_motion": "true" },
      });
    });
    expect(document.documentElement).toHaveClass("reduce-motion");
    expect(screen.getByText("reduced")).toBeInTheDocument();

    act(() => {
      useSettingsStore.setState({
        settings: { "appearance.reduce_motion": "false" },
      });
    });
    expect(document.documentElement).not.toHaveClass("reduce-motion");
  });
});
