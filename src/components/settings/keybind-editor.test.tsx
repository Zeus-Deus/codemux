/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { KeybindEditor } from "./keybind-editor";

let updateSettingSpy: ReturnType<typeof vi.fn>;

function rowFor(label: string): HTMLElement {
  const row = screen.getByText(label).closest(".group\\/kb");
  if (!row) throw new Error(`no row for ${label}`);
  return row as HTMLElement;
}

beforeEach(() => {
  updateSettingSpy = vi.fn().mockResolvedValue(undefined);
  const settings = useSyncedSettingsStore.getState().settings;
  useSyncedSettingsStore.setState({
    settings: { ...settings, keyboard: { ...settings.keyboard, shortcuts: {} } },
    updateSetting: updateSettingSpy,
  } as never);
});

afterEach(() => {
  cleanup();
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  vi.clearAllMocks();
});

describe("KeybindEditor native shortcuts", () => {
  it("lists Reload interface with a fixed, non-editable keycap", () => {
    render(<KeybindEditor />);
    const row = rowFor("Reload interface");
    expect(within(row).getByText(/Recovers a blank or frozen window/)).toBeInTheDocument();
    expect(within(row).getByText("Ctrl+Alt+R").tagName).toBe("SPAN");
    expect(within(row).queryByRole("button")).toBeNull();
  });

  it("keeps rebindable shortcuts clickable", () => {
    render(<KeybindEditor />);
    expect(within(rowFor("Command palette")).getByRole("button", { name: "Ctrl+K" })).toBeInTheDocument();
  });

  it("refuses to bind another action to the native combo", () => {
    render(<KeybindEditor />);
    const row = rowFor("Command palette");
    fireEvent.click(within(row).getByRole("button", { name: "Ctrl+K" }));
    fireEvent.keyDown(window, { key: "r", ctrlKey: true, altKey: true });

    expect(within(row).getByText(/Ctrl\+Alt\+R is reserved for/)).toBeInTheDocument();
    expect(within(row).getByText("Reload interface")).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: "Override" })).toBeNull();
    expect(updateSettingSpy).not.toHaveBeenCalled();

    fireEvent.click(within(row).getByRole("button", { name: "Cancel" }));
    expect(within(row).queryByText(/is reserved for/)).toBeNull();
    expect(updateSettingSpy).not.toHaveBeenCalled();
  });

  it("hides native shortcuts from a web-remote client", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    render(<KeybindEditor />);
    expect(screen.queryByText("Reload interface")).toBeNull();
    expect(screen.getByText("Command palette")).toBeInTheDocument();
  });
});
