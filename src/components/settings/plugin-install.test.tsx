import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const preview = {
  id: "hello",
  name: "Hello",
  description: "Says hello",
  version: "0.1.0",
  source: "git" as const,
  repo: "https://github.com/owner/hello",
  sha: "6f37720efb9796e334957f1046285ca939e5731e",
  subdir: null,
  tier: "unverified" as const,
  replaces: null as string | null,
};
const pluginsPreviewInstall = vi.fn(async (..._a: unknown[]) => preview);
const pluginsInstall = vi.fn(async (..._a: unknown[]) => preview);
vi.mock("@/contrib/ipc", () => ({
  pluginsPreviewInstall: (...a: unknown[]) => pluginsPreviewInstall(...a),
  pluginsInstall: (...a: unknown[]) => pluginsInstall(...a),
  pluginsCatalog: vi.fn(async () => ({
    catalog: { schemaVersion: 1, revision: 1, plugins: [], blocked: [] },
    fetchedAt: null,
    error: null,
  })),
}));
vi.mock("@/contrib/boot", () => ({ reloadPlugins: vi.fn(async () => {}) }));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { FULL_ACCESS_WARNING, usePluginInstaller } from "./plugin-install";

function Harness() {
  const installer = usePluginInstaller();
  return (
    <>
      <button type="button" onClick={installer.promptUrl}>
        open
      </button>
      {installer.dialog}
    </>
  );
}

afterEach(() => {
  cleanup();
  pluginsInstall.mockClear();
  pluginsPreviewInstall.mockClear();
});

async function review() {
  render(<Harness />);
  fireEvent.click(screen.getByText("open"));
  fireEvent.change(screen.getByLabelText("GitHub repository URL"), {
    target: { value: "https://github.com/owner/hello" },
  });
  fireEvent.click(screen.getByText("Continue"));
  await screen.findByText(/Install Hello\?/);
}

describe("plugin install dialog", () => {
  it("shows name, source, commit, label and the full-access warning before installing", async () => {
    await review();
    expect(pluginsPreviewInstall).toHaveBeenCalledWith({ kind: "git", url: "https://github.com/owner/hello" });
    expect(screen.getByText("https://github.com/owner/hello")).toBeTruthy();
    expect(screen.getByText("6f37720efb97")).toBeTruthy();
    expect(screen.getByText("Unverified")).toBeTruthy();
    expect(screen.getByText(FULL_ACCESS_WARNING)).toBeTruthy();
    expect(pluginsInstall).not.toHaveBeenCalled();
  });

  it("installs off by default, and on only from the second button", async () => {
    await review();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Install" }));
    });
    await waitFor(() => expect(pluginsInstall).toHaveBeenCalledTimes(1));
    expect(pluginsInstall.mock.calls[0]).toEqual([
      { kind: "git", url: "https://github.com/owner/hello" },
      preview.sha,
      false,
    ]);
    cleanup();
    await review();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Install and turn on" }));
    });
    await waitFor(() => expect(pluginsInstall).toHaveBeenCalledTimes(2));
    expect(pluginsInstall.mock.calls[1][2]).toBe(true);
  });

  it("shows a refusal (e.g. a blocked commit) instead of installing", async () => {
    pluginsPreviewInstall.mockRejectedValueOnce(new Error("Blocked by the Codemux catalog: malware"));
    render(<Harness />);
    fireEvent.click(screen.getByText("open"));
    fireEvent.change(screen.getByLabelText("GitHub repository URL"), {
      target: { value: "https://github.com/owner/hello" },
    });
    fireEvent.click(screen.getByText("Continue"));
    expect((await screen.findByRole("alert")).textContent).toContain("Blocked by the Codemux catalog: malware");
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
  });
});
