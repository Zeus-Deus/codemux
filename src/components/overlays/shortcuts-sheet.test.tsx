/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

import { resolveKeybinds } from "@/hooks/use-resolved-keybinds";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { buildShortcutGroups, ShortcutsSheet } from "./shortcuts-sheet";

function rowsOf(overrides: Record<string, string>, query = "") {
  return buildShortcutGroups(resolveKeybinds(overrides).keybindMap, query).flatMap(
    (group) => group.rows,
  );
}

describe("buildShortcutGroups", () => {
  it("folds a regular numbered series into one row", () => {
    const rows = rowsOf({});
    expect(rows.find((r) => r.id === "switchTab1")).toEqual({
      id: "switchTab1",
      label: "Switch to tab 1–9",
      keys: "Ctrl+1–9",
    });
    expect(rows.some((r) => r.id === "switchTab2")).toBe(false);
  });

  it("keeps a series expanded once a rebind breaks its pattern", () => {
    const rows = rowsOf({ switchTab3: "Alt+F3" });
    expect(rows.filter((r) => r.id.startsWith("switchTab"))).toHaveLength(9);
    expect(rows.find((r) => r.id === "switchTab3")?.keys).toBe("Alt+F3");
  });

  it("shows the user's binding and drops unbound actions", () => {
    const rows = rowsOf({ toggleSidebar: "Ctrl+Alt+S", newTab: "" });
    expect(rows.find((r) => r.id === "toggleSidebar")?.keys).toBe("Ctrl+Alt+S");
    expect(rows.some((r) => r.id === "newTab")).toBe(false);
  });

  it("leaves out the reload guards", () => {
    expect(rowsOf({}).some((r) => r.id.startsWith("block"))).toBe(false);
  });

  it("filters by label or keys and drops empty groups", () => {
    const groups = buildShortcutGroups(resolveKeybinds({}).keybindMap, "split");
    expect(groups.map((g) => g.label)).toEqual(["Panes"]);
    expect(rowsOf({}, "ctrl+shift+b").map((r) => r.id)).toEqual(["toggleRightPanel"]);
  });
});

describe("ShortcutsSheet", () => {
  const original = useSyncedSettingsStore.getState().settings;

  beforeEach(() => {
    useUIStore.setState({ showShortcutsSheet: true, showSettings: false, settingsSection: null });
  });

  afterEach(() => {
    cleanup();
    useSyncedSettingsStore.setState({ settings: original });
  });

  it("lists bindings by category, following rebinds", () => {
    useSyncedSettingsStore.setState({
      settings: {
        ...original,
        keyboard: { ...original.keyboard, shortcuts: { commandPalette: "Ctrl+P" } },
      },
    });
    render(<ShortcutsSheet />);
    const general = screen.getByRole("region", { name: "General" });
    const row = within(general).getByText("Command palette").closest("li")!;
    expect(row).toHaveTextContent("Ctrl+P");
  });

  it("filters as you type", () => {
    render(<ShortcutsSheet />);
    fireEvent.change(screen.getByLabelText("Filter shortcuts"), {
      target: { value: "zzz-nothing" },
    });
    expect(screen.getByText(/No shortcuts match/)).toBeInTheDocument();
  });

  it("gives a lone group the full width instead of half of two columns", () => {
    render(<ShortcutsSheet />);
    const layout = () => screen.getByRole("region", { name: "Panes" }).parentElement!;
    expect(layout()).toHaveClass("sm:columns-2");
    fireEvent.change(screen.getByLabelText("Filter shortcuts"), {
      target: { value: "split" },
    });
    expect(layout()).not.toHaveClass("sm:columns-2");
  });

  it("names the bound close key in the footer and hides it when unbound", () => {
    const { unmount } = render(<ShortcutsSheet />);
    expect(screen.getByText("escape to close")).toBeInTheDocument();
    unmount();
    useSyncedSettingsStore.setState({
      settings: {
        ...original,
        keyboard: { ...original.keyboard, shortcuts: { closeOverlay: "" } },
      },
    });
    render(<ShortcutsSheet />);
    expect(screen.queryByText(/to close$/)).toBeNull();
  });

  it("pins the sheet near the top of the mobile shell", () => {
    render(<ShortcutsSheet />);
    const dialog = document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!;
    expect(dialog).toHaveClass("in-[[data-mobile]]:top-[calc(var(--mobile-top,0px)+12px)]!");
    expect(dialog).toHaveClass("in-[[data-mobile]]:p-0!");
  });

  it("hands off to Settings ▸ Shortcuts for rebinding", () => {
    render(<ShortcutsSheet />);
    fireEvent.click(screen.getByRole("button", { name: "Customize…" }));
    const ui = useUIStore.getState();
    expect(ui.showShortcutsSheet).toBe(false);
    expect(ui.showSettings).toBe(true);
    expect(ui.settingsSection).toBe("shortcuts");
  });
});
