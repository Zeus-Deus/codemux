import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, waitFor } from "@testing-library/react";
vi.mock("@/lib/addons/bridge", () => ({
  addonInvoke: vi.fn().mockResolvedValue(null),
  mountAddon: vi.fn(),
}));
import { AddonView } from "./addon-view";
import { addonInvoke, mountAddon } from "@/lib/addons/bridge";
import { useAddonsStore } from "@/stores/addons-store";
import type { AddonInstallation } from "@/lib/addons/types";
const installed = {
  installationId: "installation",
  manifest: { id: "test.plugin", name: "Fixture" },
  digest: "first",
  dataGeneration: "data-1",
  desiredEnabled: true,
  status: "enabled-running",
  source: { kind: "local", identity: "fixture" },
  failure: null,
  previous: null,
} as AddonInstallation;
beforeEach(() => {
  vi.clearAllMocks();
  let generation = 0;
  vi.mocked(mountAddon).mockImplementation(async () => ({
    generation: `generation-${++generation}`,
    viewId: `view-${generation}`,
  }));
  useAddonsStore.setState({
    installed: [installed],
    ready: true,
    paused: false,
    trees: {},
    failures: {},
    hostEpochs: {},
    revoking: {},
  });
});
afterEach(cleanup);
describe("view lifecycle across native generations", () => {
  it("remounts after an accepted update and after restoration of the previous release", async () => {
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await act(async () => {
      useAddonsStore.setState({
        installed: [
          { ...installed, digest: "second", dataGeneration: "data-2" },
        ],
      });
    });
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(2));
    expect(addonInvoke).toHaveBeenCalledWith(
      "addon_unmount",
      expect.objectContaining({ generation: "generation-1" }),
    );
    await act(async () => {
      useAddonsStore.setState({
        installed: [installed],
        hostEpochs: { "test.plugin": 1 },
      });
    });
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(3));
    expect(addonInvoke).toHaveBeenCalledWith(
      "addon_unmount",
      expect.objectContaining({ generation: "generation-2" }),
    );
  });
  it("keeps a failed release inert until explicit retry changes its native status", async () => {
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await act(async () => {
      useAddonsStore.setState({
        installed: [
          {
            ...installed,
            status: "failed-disabled",
            failure: "Synthetic failure",
          },
        ],
        hostEpochs: { "test.plugin": 1 },
      });
    });
    expect(mountAddon).toHaveBeenCalledTimes(1);
    await act(async () => {
      useAddonsStore.setState({
        installed: [{ ...installed, status: "enabled-idle" }],
      });
    });
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(2));
  });
});
