import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { SettingsSyncIndicator } from "./settings-sync-indicator";

function visibleText(container: HTMLElement): string {
  const inner = container.querySelector("[aria-live] > span");
  return inner?.getAttribute("aria-hidden") === "true" ? "" : inner?.textContent ?? "";
}

describe("SettingsSyncIndicator", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useSyncedSettingsStore.setState({ isSyncing: false, syncIssue: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("stays silent until something is saved", () => {
    const { container } = render(<SettingsSyncIndicator />);
    expect(visibleText(container)).toBe("");
  });

  it("shows Saving…, then Saved, then fades", () => {
    const { container } = render(<SettingsSyncIndicator />);
    act(() => useSyncedSettingsStore.setState({ isSyncing: true }));
    expect(visibleText(container)).toBe("Saving…");

    act(() => useSyncedSettingsStore.setState({ isSyncing: false }));
    expect(visibleText(container)).toBe("Saved");

    act(() => vi.advanceTimersByTime(2000));
    expect(visibleText(container)).toBe("");
  });

  it("keeps an offline or failed save on screen until a later save succeeds", () => {
    const { container } = render(<SettingsSyncIndicator />);
    act(() => useSyncedSettingsStore.setState({ syncIssue: "saved-locally" }));
    act(() => vi.advanceTimersByTime(10_000));
    expect(visibleText(container)).toMatch(/Offline\. Saved on this device/);

    act(() => useSyncedSettingsStore.setState({ syncIssue: "failed" }));
    expect(visibleText(container)).toBe("Couldn't save");
    expect(screen.getByText("Couldn't save").closest("span")).toHaveClass("text-destructive");
  });
});
