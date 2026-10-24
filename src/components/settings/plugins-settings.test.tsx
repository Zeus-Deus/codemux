/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { registry } from "@/contrib/registry";
import { usePluginsStore } from "@/contrib/plugins-store";
import type { PluginSetupStatus } from "@/contrib/plugin";
import { SETTINGS_PAGES_AREA } from "@/sdk/areas";

vi.mock("@/contrib/ipc", () => ({
  pluginsRemove: vi.fn(),
  pluginsRevealRoot: vi.fn(),
  pluginsCatalog: vi.fn(async () => ({
    catalog: { schemaVersion: 1, revision: 1, plugins: [], blocked: [] },
    fetchedAt: null,
    error: null,
  })),
}));
vi.mock("@/lib/toast", () => ({ reportFailure: () => () => {}, toast: { success: vi.fn(), error: vi.fn() } }));

import { PluginsSettings } from "./plugins-settings";

const disposers: (() => void)[] = [];
afterEach(() => {
  cleanup();
  for (const dispose of disposers.splice(0)) dispose();
  usePluginsStore.setState({ records: {}, decisions: {} });
});

function setup(status?: { get: () => PluginSetupStatus | null; subscribe: (fn: () => void) => () => void }) {
  usePluginsStore.setState({
    records: {
      demo: { id: "demo", name: "Demo", kind: "bundled", status: "loaded", tier: "official", defaultEnabled: true },
      plain: { id: "plain", name: "Plain", kind: "bundled", status: "loaded", tier: "official", defaultEnabled: true },
    },
  });
  disposers.push(registry.register({
    id: "demo:settings", source: "plugin:demo", area: SETTINGS_PAGES_AREA, title: "Demo", render: () => null,
    data: { pluginId: "demo", status },
  }));
}

describe("plugin rows", () => {
  it("shows a visible Settings action only for plugins with a settings page", async () => {
    setup();
    const onOpenSettings = vi.fn();
    render(<PluginsSettings onOpenSettings={onOpenSettings} />);
    const row = document.querySelector('[data-plugin-row="demo"]') as HTMLElement;
    await userEvent.setup().click(screen.getByRole("button", { name: "Settings" }));
    expect(onOpenSettings).toHaveBeenCalledWith("demo");
    expect(row).toContainElement(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getAllByRole("button", { name: "Settings" })).toHaveLength(1);
    // A bundled plugin without a folder needs no overflow menu.
    expect(screen.queryByRole("button", { name: "Demo actions" })).toBeNull();
  });

  it("shows the plugin's setup status separately from its switch and follows changes", () => {
    let current: PluginSetupStatus = { label: "Setup needed · driver not installed", tone: "attention" };
    const listeners = new Set<() => void>();
    setup({ get: () => current, subscribe: (fn) => { listeners.add(fn); return () => listeners.delete(fn); } });
    render(<PluginsSettings onOpenSettings={() => {}} />);
    expect(screen.getByText("Setup needed · driver not installed")).toHaveAttribute("data-plugin-setup", "attention");
    expect(screen.getByRole("switch", { name: "Turn off Demo" })).toBeChecked();
    act(() => {
      current = { label: "Ready to use", tone: "ready" };
      for (const fn of listeners) fn();
    });
    expect(screen.getByText("Ready to use")).toHaveAttribute("data-plugin-setup", "ready");
    expect(document.querySelector('[data-plugin-row="plain"] [data-plugin-setup]')).toBeNull();
  });
});
