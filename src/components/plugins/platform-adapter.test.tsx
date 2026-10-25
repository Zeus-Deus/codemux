import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";

const answers = vi.hoisted(() => ({ catalogue: [] as unknown[] }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (_command: string, args: { method: string }) =>
    args.method === "catalogue" ? answers.catalogue.shift() : undefined,
  ),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import {
  pluginChangeKey,
  refreshPlatformCatalogue,
  resetPlatformPluginsForTests,
  usePlatformCatalogue,
  usePlatformPluginsStore,
} from "./platform-adapter";

const hostCatalogue = () => ({
  installations: [
    {
      installationId: "plugin-1",
      generation: "gen-1",
      manifest: { id: "acme.realm", name: "Realm", version: "1.0.0", apiVersion: 1, description: null, actions: [], views: [], settings: [] },
      source: { kind: "local", directory: "/p" },
      digest: "a",
      enabled: true,
      consentedDigest: "a",
      status: "idle",
      error: null,
      grants: { capabilities: [], projectIds: [] },
      settings: {},
      updatedAt: "",
      pendingUpdate: null,
    },
  ],
  recoverySupported: true,
});

afterEach(() => resetPlatformPluginsForTests());

describe("platform catalogue store", () => {
  it("keeps the same catalogue object when a re-read changed nothing", async () => {
    resetPlatformPluginsForTests();
    answers.catalogue = [hostCatalogue(), hostCatalogue()];
    await refreshPlatformCatalogue();
    const first = usePlatformPluginsStore.getState().catalogue;
    expect(first?.installations[0]?.pendingUpdate).toBeUndefined();
    await refreshPlatformCatalogue();
    expect(usePlatformPluginsStore.getState().catalogue).toBe(first);
  });

  it("doesn't re-render catalogue readers for another surface's change signal", () => {
    resetPlatformPluginsForTests({ catalogue: { installations: [] }, loaded: true });
    let renders = 0;
    function Reader() {
      usePlatformCatalogue();
      renders += 1;
      return null;
    }
    render(<Reader />);
    const before = renders;
    act(() =>
      usePlatformPluginsStore.setState({
        revisions: new Map([[pluginChangeKey({ installationId: "plugin-1", generation: "gen-1", topic: "tree:x" }), 1]]),
      }),
    );
    expect(renders).toBe(before);
  });
});
