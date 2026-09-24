import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
vi.mock("@/lib/addons/bridge", () => ({
  addonInvoke: vi.fn().mockResolvedValue(null),
  mountAddon: vi.fn(),
}));
import { registerAddonComposer } from "@/lib/addons/composer-registry";
import { AddonView } from "./addon-view";
import { addonInvoke, mountAddon } from "@/lib/addons/bridge";
import { addonTreeKey, useAddonsStore } from "@/stores/addons-store";
import type { AddonInstallation, AddonNode } from "@/lib/addons/types";
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
  vi.mocked(addonInvoke).mockResolvedValue(null);
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
  it("rebinds a panel when its composer registers late, becomes ambiguous, or closes", async () => {
    const cleanupComposers: Array<() => void> = [];
    try {
      render(
        <AddonView id="test.plugin" view="panel" workspaceId="workspace" />,
      );
      await waitFor(() =>
        expect(mountAddon).toHaveBeenLastCalledWith(
          "test.plugin",
          "panel",
          "panels",
          "workspace",
          null,
        ),
      );
      await act(async () => {
        cleanupComposers.push(
          registerAddonComposer("first", {
            workspaceId: "workspace",
            append: () => 1,
          }),
        );
      });
      await waitFor(() =>
        expect(mountAddon).toHaveBeenLastCalledWith(
          "test.plugin",
          "panel",
          "panels",
          "workspace",
          "first",
        ),
      );
      await act(async () => {
        cleanupComposers.push(
          registerAddonComposer("second", {
            workspaceId: "workspace",
            append: () => 1,
          }),
        );
      });
      await waitFor(() =>
        expect(mountAddon).toHaveBeenLastCalledWith(
          "test.plugin",
          "panel",
          "panels",
          "workspace",
          null,
        ),
      );
      await act(async () => {
        cleanupComposers.shift()!();
      });
      await waitFor(() =>
        expect(mountAddon).toHaveBeenLastCalledWith(
          "test.plugin",
          "panel",
          "panels",
          "workspace",
          "second",
        ),
      );
      await act(async () => {
        cleanupComposers.shift()!();
      });
      await waitFor(() =>
        expect(mountAddon).toHaveBeenLastCalledWith(
          "test.plugin",
          "panel",
          "panels",
          "workspace",
          null,
        ),
      );
      expect(addonInvoke).toHaveBeenCalledWith(
        "addon_unmount",
        expect.objectContaining({ viewId: "view-1" }),
      );
    } finally {
      cleanup();
      for (const remove of cleanupComposers) remove();
    }
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

const element = (
  id: string,
  name: string,
  properties: Record<string, unknown> = {},
  children: AddonNode[] = [],
  press?: string,
): AddonNode => ({
  id,
  type: 1,
  element: name,
  properties,
  attributes: {},
  eventListeners: press ? { press: { callbackId: press } } : {},
  children,
});
const textNode = (data: string): AddonNode => ({
  ...element(`text-${data}`, ""),
  type: 3,
  data,
});
/** Mount the view and publish a tree for its first generation. */
async function showTree(children: AddonNode[], revision = 1) {
  await act(async () => {
    useAddonsStore.setState((s) => ({
      trees: {
        ...s.trees,
        [addonTreeKey("generation-1", "view-1")]: {
          type: "tree",
          pluginId: "test.plugin",
          generation: "generation-1",
          viewId: "view-1",
          revision,
          tree: { children },
        },
      },
    }));
  });
}
const refused = (code: string, message: string) => ({
  message,
  data: { code },
});
describe("refused actions are transient", () => {
  it("keeps the tree usable when a click was refused as stale", async () => {
    vi.mocked(addonInvoke).mockImplementation((command) =>
      command === "addon_ui_event"
        ? Promise.reject(refused("CONTEXT_STALE", "UI callback expired"))
        : Promise.resolve(null),
    );
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([
      element("refresh", "cmx-button", { label: "Refresh" }, [], "cb-1"),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(
      await screen.findByText(
        "This view changed before your action reached the add-on. Try again.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    // The view is still there and still answers clicks.
    expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss add-on notice" }));
    expect(screen.queryByRole("status")).toBeNull();
    expect(addonInvoke).toHaveBeenCalledWith(
      "addon_ui_event",
      expect.objectContaining({ callbackId: "cb-1", event: "press" }),
    );
  });
  it("keeps the tree when a link is refused and says why", async () => {
    useAddonsStore.setState({
      installed: [
        {
          ...installed,
          manifest: { ...installed.manifest, permissions: ["external.open"] },
        },
      ],
    });
    vi.mocked(addonInvoke).mockImplementation((command) =>
      command === "addon_ui_link"
        ? Promise.reject(
            refused("PERMISSION_DENIED", "This operation was not granted"),
          )
        : Promise.resolve(null),
    );
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([
      element("md", "cmx-markdown", {}, [textNode("[Docs](https://example.com)")]),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Docs" }));
    expect(
      await screen.findByText("This add-on is not allowed to open links."),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Docs" })).toBeTruthy();
    expect(screen.queryByText("Add-on unavailable")).toBeNull();
  });
  it("shows links as text for an add-on without external.open", async () => {
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([
      element("md", "cmx-markdown", {}, [textNode("[Docs](https://example.com)")]),
    ]);
    expect(screen.getByText("Docs")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Docs" })).toBeNull();
  });
  it("still shows a failed mount as the host-owned diagnostic", async () => {
    vi.mocked(mountAddon).mockRejectedValueOnce(
      refused("RESOURCE_LIMIT", "Too many mounted views"),
    );
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Too many mounted views",
    );
  });
});
describe("typing that outruns the add-on", () => {
  // An add-on that passes a new closure on every render gives its field a
  // new callback each time, and the broker refuses an edit sent to the old one.
  const field = (callbackId: string, value = "") => ({
    ...element("query", "cmx-text-field", { label: "Query", value }),
    eventListeners: { change: { callbackId } },
  });
  const edits = () =>
    vi
      .mocked(addonInvoke)
      .mock.calls.filter(([command]) => command === "addon_ui_event")
      .map(([, args]) => {
        const { callbackId, value } = args as {
          callbackId: string;
          value: unknown;
        };
        return [callbackId, value];
      });
  function refuseCallback(stale: string) {
    vi.mocked(addonInvoke).mockImplementation((command, args) =>
      command === "addon_ui_event" &&
      (args as { callbackId: string }).callbackId === stale
        ? Promise.reject(refused("CONTEXT_STALE", "UI callback expired"))
        : Promise.resolve(null),
    );
  }
  it("sends the refused edit again to the field's next callback", async () => {
    refuseCallback("cb-1");
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([field("cb-1")]);
    const input = screen.getByRole("textbox", { name: "Query" });
    fireEvent.change(input, { target: { value: "a" } });
    fireEvent.change(input, { target: { value: "ab" } });
    await act(async () => {});
    // Nothing to "try again": the user is still typing and keeps the text.
    expect(screen.queryByRole("status")).toBeNull();
    await showTree([field("cb-2", "a")], 2);
    await waitFor(() => expect(edits()).toContainEqual(["cb-2", "ab"]));
    // Only the latest text is resent, once.
    expect(edits()).toEqual([
      ["cb-1", "a"],
      ["cb-1", "ab"],
      ["cb-2", "ab"],
    ]);
    expect(input).toHaveValue("ab");
    expect(screen.queryByRole("status")).toBeNull();
  });
  it("resends at once when the new tree arrived before the refusal", async () => {
    let refuse!: (reason: unknown) => void;
    vi.mocked(addonInvoke).mockImplementation((command, args) =>
      command === "addon_ui_event" &&
      (args as { callbackId: string }).callbackId === "cb-1"
        ? new Promise((_, reject) => {
            refuse = reject;
          })
        : Promise.resolve(null),
    );
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([field("cb-1")]);
    fireEvent.change(screen.getByRole("textbox", { name: "Query" }), {
      target: { value: "a" },
    });
    await showTree([field("cb-2")], 2);
    await act(async () => {
      refuse(refused("CONTEXT_STALE", "UI callback expired"));
    });
    await waitFor(() => expect(edits()).toContainEqual(["cb-2", "a"]));
  });
  it("does not resend a newer keystroke's text or an edit to a removed field", async () => {
    refuseCallback("cb-1");
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([field("cb-1")]);
    fireEvent.change(screen.getByRole("textbox", { name: "Query" }), {
      target: { value: "a" },
    });
    await act(async () => {});
    await showTree([element("done", "cmx-text", {}, [textNode("Saved")])], 2);
    await act(async () => {});
    expect(edits()).toEqual([["cb-1", "a"]]);
  });
  it("still explains a refused switch, which shows the add-on's value", async () => {
    refuseCallback("cb-1");
    render(<AddonView id="test.plugin" view="panel" workspaceId="workspace" />);
    await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
    await showTree([
      {
        ...element("sync", "cmx-switch", { label: "Sync", checked: false }),
        eventListeners: { change: { callbackId: "cb-1" } },
      },
    ]);
    fireEvent.click(screen.getByRole("switch", { name: "Sync" }));
    expect(
      await screen.findByText(
        "This view changed before your action reached the add-on. Try again.",
      ),
    ).toBeTruthy();
  });
});
describe("add-on view containment and labels", () => {
  it("contains a render failure to the view and recovers on the next tree", async () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <>
          <p>Core content</p>
          <AddonView id="test.plugin" view="panel" workspaceId="workspace" />
        </>,
      );
      await waitFor(() => expect(mountAddon).toHaveBeenCalledTimes(1));
      // A tree the renderer cannot handle (options must be a list).
      await showTree([element("bad", "cmx-select", { options: "oops" })]);
      expect(screen.getByRole("alert")).toHaveTextContent(
        "CodeMux could not display this add-on view",
      );
      expect(screen.getByText("Core content")).toBeTruthy();
      await showTree([element("ok", "cmx-button", { label: "Retry" })], 2);
      expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
      expect(screen.queryByRole("alert")).toBeNull();
    } finally {
      quiet.mockRestore();
    }
  });
  it("keeps its labelled surface and gives honest advice when the view itself fails", async () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      // A failure outside the add-on's tree (here: a broken inventory row).
      useAddonsStore.setState({
        installed: [
          {
            ...installed,
            get manifest(): never {
              throw new Error("broken row");
            },
          } as AddonInstallation,
        ],
      });
      render(
        <AddonView
          id="test.plugin"
          view="panel"
          workspaceId="workspace"
          label="Brief — Fixture"
        />,
      );
      const region = screen.getByRole("region", { name: "Brief — Fixture" });
      expect(region).toBe(screen.getByTestId("addon-view"));
      expect(screen.getByRole("alert")).toHaveTextContent(
        "CodeMux could not display this add-on view. Close it and open it again to retry.",
      );
      expect(screen.getByRole("alert")).not.toHaveTextContent(
        "try again when the add-on updates it",
      );
    } finally {
      quiet.mockRestore();
    }
  });
  it("names its region after the panel and the add-on", async () => {
    render(
      <AddonView
        id="test.plugin"
        view="panel"
        workspaceId="workspace"
        label="Brief — Fixture"
      />,
    );
    expect(screen.getByRole("region", { name: "Brief — Fixture" })).toBe(
      screen.getByTestId("addon-view"),
    );
  });
  it("renders no second region inside a surface that already has one", () => {
    render(
      <AddonView
        id="test.plugin"
        view="issues"
        workspaceId="workspace"
        kind="composerViews"
        composerId="composer"
        region={false}
      />,
    );
    expect(screen.queryByRole("region")).toBeNull();
    expect(screen.getByTestId("addon-view")).toBeTruthy();
  });
});
