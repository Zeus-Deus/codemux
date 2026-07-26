/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(async () => ({})),
  dbSetSetting: vi.fn(async () => undefined),
}));

import { invoke } from "@tauri-apps/api/core";
import { dbSetSetting } from "@/tauri/commands";
import { useSettingsStore } from "@/stores/settings-store";

import { SmoothScrollingSection } from "./smooth-scrolling-section";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;
const dbSetSettingMock = dbSetSetting as unknown as ReturnType<typeof vi.fn>;

// jsdom's UA ("…AppleWebKit/537.36 … jsdom/…", no Chrome or Mac token)
// classifies as Linux WebKitGTK, so the section renders by default. Tests that
// need another engine stub `navigator.userAgent` explicitly.
function stubUserAgent(userAgent: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(userAgent);
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("SmoothScrollingSection", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    dbSetSettingMock.mockReset();
    dbSetSettingMock.mockResolvedValue(undefined);
    useSettingsStore.setState({ settings: {}, loaded: true });
  });

  it("renders the toggle OFF by default with its explanation", () => {
    render(<SmoothScrollingSection />);

    expect(screen.getByText("Smooth scrolling")).toBeInTheDocument();
    expect(screen.getByText(/Off by default/i)).toBeInTheDocument();
    expect(screen.getByRole("switch")).toHaveAttribute(
      "data-state",
      "unchecked",
    );
  });

  it("reflects a persisted ON value", () => {
    useSettingsStore.setState({
      settings: { "appearance.smooth_scrolling": "true" },
      loaded: true,
    });

    render(<SmoothScrollingSection />);

    expect(screen.getByRole("switch")).toHaveAttribute("data-state", "checked");
  });

  it("persists the choice and pushes it to the webview when turned on", async () => {
    const user = userEvent.setup();
    render(<SmoothScrollingSection />);

    await user.click(screen.getByRole("switch"));

    expect(useSettingsStore.getState().get("appearance.smooth_scrolling")).toBe(
      "true",
    );
    await waitFor(() =>
      expect(dbSetSettingMock).toHaveBeenCalledWith(
        "appearance.smooth_scrolling",
        "true",
      ),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_smooth_scrolling", {
        enabled: true,
      }),
    );
  });

  it("pushes the off state when turned back off", async () => {
    const user = userEvent.setup();
    useSettingsStore.setState({
      settings: { "appearance.smooth_scrolling": "true" },
      loaded: true,
    });

    render(<SmoothScrollingSection />);
    await user.click(screen.getByRole("switch"));

    expect(useSettingsStore.getState().get("appearance.smooth_scrolling")).toBe(
      "false",
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_smooth_scrolling", {
        enabled: false,
      }),
    );
  });

  it("keeps the persisted value when the webview command is unavailable", async () => {
    const user = userEvent.setup();
    invokeMock.mockRejectedValue(new Error("no such command"));
    vi.spyOn(console, "error").mockImplementation(() => {});

    render(<SmoothScrollingSection />);
    await user.click(screen.getByRole("switch"));

    expect(useSettingsStore.getState().get("appearance.smooth_scrolling")).toBe(
      "true",
    );
    await waitFor(() => expect(console.error).toHaveBeenCalled());
    // Still checked — a failed webview call must not revert the UI.
    expect(screen.getByRole("switch")).toHaveAttribute("data-state", "checked");
  });

  it("renders nothing outside Linux WebKitGTK (the setting is inert there)", () => {
    stubUserAgent(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15",
    );
    const { container } = render(<SmoothScrollingSection />);
    expect(container).toBeEmptyDOMElement();

    cleanup();

    stubUserAgent(
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    );
    const chromium = render(<SmoothScrollingSection />);
    expect(chromium.container).toBeEmptyDOMElement();
  });
});
