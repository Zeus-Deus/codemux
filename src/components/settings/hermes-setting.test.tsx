import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { cleanup, render, screen, fireEvent, waitFor, within } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { HermesSetting } from "./hermes-setting";
import { useHermes } from "@/stores/hermes-store";

const profile = {
  schema_version: 1,
  host: "local",
  installation: "/mock/bin/hermes",
  root: "/mock/hermes",
  id: "coding",
  home: "/mock/hermes/profiles/coding",
  identity: "mock-coding",
};

describe("HermesSetting", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    useHermes.setState({ profiles: [profile], catalogs: {} });
  });
  afterEach(cleanup);

  it("asks before disconnecting a profile", async () => {
    render(<HermesSetting />);
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(invoke).not.toHaveBeenCalledWith("hermes_disconnect", expect.anything());

    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Disconnect" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("hermes_disconnect", { profile }),
    );
    expect(await screen.findByText(/^Disconnected\./)).toBeInTheDocument();
  });

  it("cancelling the confirmation leaves the profile connected", async () => {
    render(<HermesSetting />);
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(invoke).not.toHaveBeenCalledWith("hermes_disconnect", expect.anything());
  });

  it("disables Save and refresh while it is running", async () => {
    let finish: (v: unknown) => void = () => {};
    invoke.mockImplementation((cmd: string) =>
      cmd === "hermes_profiles" ? new Promise((r) => { finish = r; }) : Promise.resolve(null),
    );
    render(<HermesSetting />);
    const save = screen.getByRole("button", { name: "Save and refresh" });
    fireEvent.click(save);
    await waitFor(() => expect(save).toBeDisabled());
    finish([profile]);
    await waitFor(() => expect(save).toBeEnabled());
    expect(screen.getByText("1 profiles found.")).toBeInTheDocument();
  });
});
