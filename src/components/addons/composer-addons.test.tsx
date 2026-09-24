/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
} from "@testing-library/react";
vi.mock("@/lib/addons/platform", () => ({ executeAddon: vi.fn() }));
vi.mock("./addon-view", () => ({
  AddonView: (props: Record<string, unknown>) => (
    <div data-testid="addon-view-stub" data-props={JSON.stringify(props)} />
  ),
}));
import { executeAddon } from "@/lib/addons/platform";
import { useAddonsStore } from "@/stores/addons-store";
import type { AddonInstallation, AddonManifest } from "@/lib/addons/types";
import {
  ComposerAddonAccessory,
  useAddonComposerActions,
} from "./composer-addons";
const plugin = {
  installationId: "installation",
  manifest: {
    id: "test.plugin",
    name: "Fixture",
    contributes: {
      commands: [],
      panels: [],
      composerActions: [
        { id: "insert", title: "Add project brief", icon: "file-text" },
      ],
      composerViews: [{ id: "issues", title: "Issues", icon: "github" }],
    },
  } as unknown as AddonManifest,
  desiredEnabled: true,
  status: "enabled-idle",
} as AddonInstallation;
const accessory = {
  pluginId: "test.plugin",
  view: "issues",
  composerId: "composer-1",
  workspaceId: "workspace",
};
beforeEach(() => {
  vi.clearAllMocks();
  useAddonsStore.setState({ installed: [plugin], paused: false, accessory: null });
});
afterEach(cleanup);
describe("composer Add-ons actions", () => {
  const actions = (
    binding: Parameters<typeof useAddonComposerActions>[0],
  ) => renderHook(() => useAddonComposerActions(binding)).result.current;
  it("lists each action under Add-ons, attributed, and runs it for this composer", () => {
    const [row] = actions({
      id: "composer-1",
      registered: true,
      unavailable: null,
    });
    expect(row).toMatchObject({
      id: "addon:test.plugin:insert",
      label: "Add project brief",
      description: "Fixture",
      group: "Add-ons",
      disabled: false,
    });
    row.onSelect();
    expect(executeAddon).toHaveBeenCalledExactlyOnceWith(
      "test.plugin",
      "insert",
      "composerActions",
      "composer-1",
    );
  });
  it.each([
    [
      "Add-ons are unavailable in remote workspaces",
      "Add-ons are unavailable in remote workspaces · Fixture",
    ],
    [
      "Open a local workspace to use add-on actions",
      "Open a local workspace to use add-on actions · Fixture",
    ],
    [null, "Connecting to this draft… · Fixture"],
  ])("shows the specific reason when it cannot run: %s", (reason, description) => {
    const [row] = actions({ id: "", registered: false, unavailable: reason });
    expect(row).toMatchObject({ disabled: true, description });
  });
  it("offers nothing while paused or for a disabled add-on", () => {
    const binding = { id: "composer-1", registered: true, unavailable: null };
    useAddonsStore.setState({ paused: true });
    expect(actions(binding)).toEqual([]);
    useAddonsStore.setState({
      paused: false,
      installed: [
        { ...plugin, desiredEnabled: false, status: "installed-disabled" },
      ],
    });
    expect(actions(binding)).toEqual([]);
  });
});
describe("composer accessory", () => {
  it("renders nothing, not even a wrapper, unless opened for this composer", () => {
    const { container } = render(
      <ComposerAddonAccessory composerId="composer-1" />,
    );
    expect(container).toBeEmptyDOMElement();
    act(() => {
      useAddonsStore.setState({
        accessory: { ...accessory, composerId: "composer-2" },
      });
    });
    expect(container).toBeEmptyDOMElement();
  });
  it("never matches a composer that is not registered", () => {
    useAddonsStore.setState({ accessory: { ...accessory, composerId: "" } });
    const { container } = render(<ComposerAddonAccessory composerId="" />);
    expect(container).toBeEmptyDOMElement();
  });
  it("mounts the add-on's composer view for this composer, labelled with both names", () => {
    useAddonsStore.setState({ accessory });
    render(<ComposerAddonAccessory composerId="composer-1" />);
    const region = screen.getByRole("region", { name: "Issues — Fixture" });
    expect(region).toHaveTextContent("Issues");
    expect(region).toHaveTextContent("Fixture");
    expect(
      JSON.parse(screen.getByTestId("addon-view-stub").dataset.props!),
    ).toEqual({
      id: "test.plugin",
      view: "issues",
      workspaceId: "workspace",
      composerId: "composer-1",
      kind: "composerViews",
      label: "Issues — Fixture",
      region: false,
    });
  });
  it("closes on request and hands focus back", () => {
    const onClose = vi.fn();
    useAddonsStore.setState({ accessory });
    render(<ComposerAddonAccessory composerId="composer-1" onClose={onClose} />);
    fireEvent.click(
      screen.getByRole("button", { name: "Close add-on accessory" }),
    );
    expect(useAddonsStore.getState().accessory).toBeNull();
    expect(onClose).toHaveBeenCalledOnce();
    expect(screen.queryByRole("region")).toBeNull();
  });
  it("hides while add-ons are paused or the add-on is disabled", () => {
    useAddonsStore.setState({ accessory, paused: true });
    const { container } = render(
      <ComposerAddonAccessory composerId="composer-1" />,
    );
    expect(container).toBeEmptyDOMElement();
    act(() => {
      useAddonsStore.setState({
        paused: false,
        installed: [{ ...plugin, status: "failed-disabled" }],
      });
    });
    expect(container).toBeEmptyDOMElement();
    act(() => {
      useAddonsStore.setState({ installed: [plugin] });
    });
    expect(screen.getByRole("region")).toBeInTheDocument();
  });
});
