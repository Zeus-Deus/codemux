import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@/lib/addons/platform", () => ({
  refreshAddons: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/lib/addons/bridge", () => ({
  addonInvoke: vi.fn().mockResolvedValue(null),
}));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => false,
}));
import { AddonsSettings } from "./addons-settings";
import { useAddonsStore } from "@/stores/addons-store";
beforeEach(() =>
  useAddonsStore.setState({
    installed: [],
    loaded: true,
    paused: false,
    warnings: [],
    error: null,
    developmentReview: null,
  }),
);
afterEach(cleanup);
it("Escape dismisses only the install dialog and restores its opener", async () => {
  const outerEscape = vi.fn();
  window.addEventListener("keydown", outerEscape);
  try {
    render(<AddonsSettings />);
    const opener = screen.getByRole("button", {
      name: "Install from link / ID",
    });
    opener.focus();
    fireEvent.click(opener);
    const input = screen.getByRole("textbox", {
      name: "Add-on install link or ID",
    });
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.keyDown(input, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
    expect(outerEscape).not.toHaveBeenCalled();
    expect(
      screen.getByRole("heading", { name: "Add-ons" }),
    ).toBeInTheDocument();
  } finally {
    window.removeEventListener("keydown", outerEscape);
  }
});
