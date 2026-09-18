import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  act,
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
import { addonInvoke } from "@/lib/addons/bridge";
import type { AddonInstallation, AddonManifest } from "@/lib/addons/types";
import manifest from "../../../examples/addons/issue-companion/manifest.json";
const installation: AddonInstallation = {
  installationId: "configuration-fixture",
  manifest: manifest as AddonManifest,
  source: { kind: "local", identity: "fixture" },
  digest: "fixture-digest",
  dataGeneration: "fixture-data",
  desiredEnabled: true,
  status: "enabled-idle",
  failure: null,
  previous: null,
};
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
it("waits for stored configuration before accepting an edit or save", async () => {
  let finish!: (settings: Record<string, unknown>) => void;
  const pending = new Promise<Record<string, unknown>>((resolve) => {
    finish = resolve;
  });
  vi.mocked(addonInvoke)
    .mockReset()
    .mockImplementation((command) =>
      command === "addon_settings_get" ? pending : Promise.resolve(null),
    );
  useAddonsStore.setState({ installed: [installation] });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  const owner = screen.getByRole("textbox", { name: "Repository owner" });
  const save = screen.getByRole("button", { name: "Save settings" });
  expect(owner).toBeDisabled();
  expect(save).toBeDisabled();
  fireEvent.submit(save.closest("form")!);
  expect(addonInvoke).not.toHaveBeenCalledWith(
    "addon_settings_set",
    expect.anything(),
  );
  await act(async () =>
    finish({ owner: "stored-owner", repository: "stored-repository" }),
  );
  await waitFor(() => expect(owner).toBeEnabled());
  expect(owner).toHaveValue("stored-owner");
  fireEvent.change(owner, { target: { value: "edited-owner" } });
  fireEvent.click(save);
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_settings_set", {
      id: "codemux.issue-companion",
      settings: { owner: "edited-owner", repository: "stored-repository" },
    }),
  );
});
it("loads the accepted release's configuration and ignores the disposed load's error", async () => {
  let rejectOld!: (error: Error) => void;
  const pending = new Promise<Record<string, unknown>>((_, reject) => {
    rejectOld = reject;
  });
  vi.mocked(addonInvoke)
    .mockReset()
    .mockReturnValueOnce(pending)
    .mockResolvedValue({
      owner: "current-owner",
      repository: "current-repository",
    });
  useAddonsStore.setState({ installed: [installation] });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  await act(async () => {
    useAddonsStore.setState({
      installed: [
        { ...installation, digest: "new-digest", dataGeneration: "new-data" },
      ],
    });
  });
  await waitFor(() =>
    expect(
      screen.getByRole("textbox", { name: "Repository owner" }),
    ).toHaveValue("current-owner"),
  );
  const owner = screen.getByRole("textbox", { name: "Repository owner" });
  fireEvent.change(owner, { target: { value: "my-current-edit" } });
  await act(async () => rejectOld(new Error("Obsolete configuration request")));
  expect(owner).toHaveValue("my-current-edit");
  expect(screen.queryByText("Obsolete configuration request")).toBeNull();
});
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
it("clears a credential-store failure after explicit session-only recovery", async () => {
  vi.mocked(addonInvoke)
    .mockReset()
    .mockImplementation((command, args) => {
      if (command === "addon_settings_get")
        return Promise.resolve({ owner: "", repository: "" });
      if (command === "addon_credential_set" && !args?.sessionOnly)
        return Promise.reject(
          new Error("Credential store unavailable or locked"),
        );
      return Promise.resolve(null);
    });
  useAddonsStore.setState({ installed: [installation] });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  const input = screen.getByLabelText("GitHub token (optional)");
  fireEvent.change(input, { target: { value: "synthetic-test-only" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await screen.findByText("Credential store unavailable or locked");
  expect(input).toHaveValue("synthetic-test-only");
  const fallback = screen.getByLabelText(
    "Store new values for this session only",
  );
  expect(fallback).not.toBeChecked();
  fireEvent.click(fallback);
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(input).toHaveValue(""));
  expect(
    screen.queryByText("Credential store unavailable or locked"),
  ).toBeNull();
  expect(addonInvoke).toHaveBeenLastCalledWith("addon_credential_set", {
    id: "codemux.issue-companion",
    credentialId: "github-token",
    value: "synthetic-test-only",
    sessionOnly: true,
  });
});
