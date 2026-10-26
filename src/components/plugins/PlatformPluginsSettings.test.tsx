import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { PluginCatalogue, PluginInstallation } from "./contract";
import { DIGEST_A, DIGEST_B, installation, manifest } from "./test-fixtures";

const requests: { method: string; params: Record<string, unknown> }[] = [];
let catalogue: PluginCatalogue = { installations: [] };
let respond: (method: string, params: Record<string, unknown>) => unknown = () => undefined;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (_command: string, args: { method: string; params: Record<string, unknown> }) => {
    requests.push(args);
    if (args.method === "catalogue") return catalogue;
    return respond(args.method, args.params);
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), error: vi.fn(), warning: vi.fn() },
  reportFailure: () => () => {},
}));

import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { resetPlatformPluginsForTests, usePlatformPluginsStore } from "./platform-adapter";
import { PlatformPluginsSection } from "./PlatformPluginsSettings";
import { editPluginSettings, usePluginSettingsDrafts } from "./plugin-settings-drafts";
import { readPluginCatalogue } from "./wire";

function show(installations: PluginInstallation[], extra: Partial<PluginCatalogue> = {}) {
  catalogue = { installations, ...extra };
  resetPlatformPluginsForTests({ catalogue, loaded: true });
  return render(<PlatformPluginsSection />);
}

/** A catalogue push, as the host sends after any change. */
function push(installations: PluginInstallation[], extra: Partial<PluginCatalogue> = {}) {
  act(() => usePlatformPluginsStore.setState({ catalogue: { installations, ...extra } }));
}

const called = (method: string) => requests.filter((request) => request.method === method).map((request) => request.params);
const rowNames = () =>
  within(screen.getByRole("list", { name: "Isolated plugins" }))
    .queryAllByRole("button", { name: /.+/ })
    .filter((button) => button.hasAttribute("data-plugin-select"))
    .map((button) => button.getAttribute("aria-label"));
const heading = (name: string) => screen.queryByRole("heading", { level: 4, name });
const editor = () => document.querySelector("[data-platform-plugin-editor]") as HTMLElement;

function lineup() {
  return [
    installation({ installationId: "inst-board", manifest: manifest({ id: "acme.board", name: "Workflow Board" }) }),
    installation({
      installationId: "inst-lens",
      manifest: manifest({ id: "acme.lens", name: "Repository Lens", description: "Shows a repository overview." }),
    }),
    installation({ installationId: "inst-notes", manifest: manifest({ id: "acme.notes", name: "Notes" }) }),
  ];
}

function configurable(overrides: Partial<PluginInstallation> = {}) {
  return installation({
    installationId: "inst-board",
    manifest: manifest({
      id: "acme.board",
      name: "Workflow Board",
      settings: [
        { key: "label", label: "Board label", type: "string", default: "Board" },
        { key: "limit", label: "Card limit", type: "number", default: 10 },
        { key: "compact", label: "Compact cards", type: "boolean", default: false },
        { key: "token", label: "API token", type: "secret" },
      ],
    }),
    settings: { label: "Board" },
    storedSecrets: ["token"],
    ...overrides,
  });
}

beforeEach(() => {
  requests.length = 0;
  respond = () => undefined;
  usePluginSettingsDrafts.setState({ drafts: {}, saving: {} });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("PlatformPluginsSection", () => {
  it("selects the installation requested by a Details link and consumes the request", () => {
    useUIStore.getState().openPluginSettings("inst-board");
    show(lineup());
    expect(heading("Workflow Board")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Workflow Board" }).getAttribute("aria-current")).toBe("true");
    expect(useUIStore.getState().pendingPluginSettingsSelection).toBeNull();
    act(() => useUIStore.getState().openPluginSettings("inst-lens"));
    expect(heading("Repository Lens")).toBeTruthy();
    expect(useUIStore.getState().settingsSection).toBe("plugins");
  });

  it("approves exactly the reviewed fingerprint with the chosen grants, then turns the plugin on", async () => {
    const fresh = installation({
      enabled: false,
      consentedDigest: null,
      status: "needs-consent",
      manifest: manifest({ capabilities: ["ui", "storage"] }),
      grants: { capabilities: [], projectIds: [] },
    });
    respond = (method) =>
      method === "consent" ? { ...fresh, consentedDigest: DIGEST_A, status: "disabled" } : { ...fresh, enabled: true };
    show([fresh]);
    expect(screen.getByRole("button", { name: "Realm" }).textContent).toContain("Needs review");
    fireEvent.click(within(editor()).getByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(DIGEST_A)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Private storage" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Approve and turn on" }));
    await waitFor(() => expect(called("setEnabled")).toHaveLength(1));
    expect(called("consent")).toEqual([
      { installationId: "inst-1", digest: DIGEST_A, grants: { capabilities: ["ui"], projectIds: [] } },
    ]);
    expect(called("setEnabled")).toEqual([{ installationId: "inst-1", enabled: true }]);
  });

  it("sends turning on a plugin that needs review to its review instead", async () => {
    show([installation({ enabled: false, consentedDigest: null, status: "needs-consent" })]);
    fireEvent.click(screen.getByRole("switch", { name: "Turn on Realm" }));
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(called("setEnabled")).toEqual([]);
  });

  it("reviews a staged update against its own digest and keeps the plugin's on/off choice", async () => {
    const staged = installation({
      pendingUpdate: {
        manifest: manifest({ version: "2.0.0" }),
        source: { kind: "local", directory: "/plugins/realm" },
        digest: DIGEST_B,
        generation: "gen-2",
        updatedAt: "2026-10-24T11:00:00.000Z",
      },
    });
    respond = () => staged;
    show([staged]);
    expect(screen.getByRole("button", { name: "Realm" }).textContent).toContain("Update 2.0.0 ready");
    fireEvent.click(screen.getByRole("button", { name: "Review update" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/keeps running until you approve/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Approve update" }));
    await waitFor(() => expect(called("consent")).toHaveLength(1));
    expect(called("consent")[0]!.digest).toBe(DIGEST_B);
    expect(called("setEnabled")).toEqual([]);
  });

  it("asks what to do with missed runs before resuming events", async () => {
    const paused = installation({ observerError: "Delivery stopped after a crash." });
    respond = (method) =>
      method === "backlog" ? { count: 2, afterSequence: 3, throughSequence: 5, requiresChoice: true } : paused;
    show([paused], { recoverySupported: true });
    expect(screen.getByText("Delivery stopped after a crash.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Resume events" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Realm missed 2 runs")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Skip missed runs" }));
    await waitFor(() => expect(called("resume")).toHaveLength(1));
    expect(called("resume")).toEqual([{ installationId: "inst-1", eventBacklog: { policy: "skip", throughSequence: 5 } }]);
  });

  it("keeps the on switch for an unavailable plugin but never turns one on", () => {
    show([
      installation({ compatibility: { available: false, missingCapabilities: ["future.cap"] }, status: "incompatible" }),
      installation({
        installationId: "inst-2",
        enabled: false,
        manifest: manifest({ id: "acme.other", name: "Other" }),
        compatibility: { available: false, missingCapabilities: ["future.cap"] },
        status: "incompatible",
      }),
    ]);
    const on = screen.getByRole("switch", { name: "Turn off Realm" });
    expect(on.getAttribute("aria-checked")).toBe("true");
    expect((on as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("switch", { name: "Turn on Other" }) as HTMLButtonElement).disabled).toBe(true);
    // Off and unable to run here: explained, but not a problem to act on.
    expect(within(editor()).queryByRole("alert")).toBeNull();
    expect(within(editor()).getByRole("status").textContent).toContain("future.cap");
    fireEvent.click(screen.getByRole("button", { name: "Realm" }));
    expect(within(editor()).getByRole("alert").textContent).toContain("future.cap");
    expect(screen.getByRole("button", { name: "Needs attention (1)" })).toBeTruthy();
  });

  it("says plainly when isolated plugins are unavailable, without blocking the page", () => {
    resetPlatformPluginsForTests({ catalogue: null, loaded: true, error: "unknown command" });
    render(<PlatformPluginsSection />);
    expect(screen.getByText(/Isolated plugins are unavailable: unknown command/)).toBeTruthy();
  });

  it("explains safe mode and offers no restart while plugin code is off, but keeps repair and removal", () => {
    show([installation({ status: "quarantined" })], { executionDisabled: true, recoverySupported: true });
    expect(screen.getByText(/Plugin code is turned off for this launch/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Restart/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Repair with agent" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove…" })).toBeTruthy();
  });

  it("restarts a stopped plugin from its problem", async () => {
    respond = () => installation();
    show([installation({ status: "error", error: "Crashed" })]);
    fireEvent.click(within(editor()).getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(called("resume")).toEqual([{ installationId: "inst-1" }]));
  });

  it("shows a read-only record without management", () => {
    show([installation({ recordIssue: "newer" })]);
    expect(screen.getByText(/Saved by a newer version of Codemux/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Remove…" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Review permissions" })).toBeNull();
    expect((screen.getByRole("switch", { name: /Realm/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getAllByRole("button", { name: "View details" }).length).toBeGreaterThan(0);
  });
});

describe("plugin list", () => {
  it("filters by search and attention without reordering rows when a status changes", () => {
    show(lineup());
    expect(rowNames()).toEqual(["Notes", "Repository Lens", "Workflow Board"]);
    expect(screen.queryByRole("button", { name: /Needs attention/ })).toBeNull();

    const [board, lens, notes] = lineup();
    push([board!, { ...lens!, status: "error", error: "Crashed" }, notes!]);
    expect(rowNames()).toEqual(["Notes", "Repository Lens", "Workflow Board"]);
    fireEvent.click(screen.getByRole("button", { name: "Needs attention (1)" }));
    expect(rowNames()).toEqual(["Repository Lens"]);

    fireEvent.change(screen.getByRole("searchbox", { name: "Search plugins" }), { target: { value: "board" } });
    expect(screen.getByText("No plugins match.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(rowNames()).toHaveLength(3);

    fireEvent.change(screen.getByRole("searchbox", { name: "Search plugins" }), { target: { value: "overview" } });
    expect(rowNames()).toEqual(["Repository Lens"]);
    expect(screen.getByText("1 of 3")).toBeTruthy();
  });

  it("selects a plugin without turning it on, and keeps the selection through pushes", () => {
    const off = lineup().map((entry) => ({ ...entry, enabled: false, status: "disabled" as const }));
    show(off);
    expect(heading("Notes")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    expect(heading("Workflow Board")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Workflow Board" }).getAttribute("aria-current")).toBe("true");
    push([{ ...off[0]!, updatedAt: "2026-10-25T00:00:00.000Z" }, { ...off[1]!, status: "error", error: "Crashed" }, off[2]!]);
    expect(heading("Workflow Board")).toBeTruthy();
    expect(called("setEnabled")).toEqual([]);
  });

  it("says so when the selected plugin is removed elsewhere", () => {
    show(lineup());
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    const [, lens, notes] = lineup();
    push([lens!, notes!]);
    expect(screen.getByText("This plugin is no longer installed.")).toBeTruthy();
    expect(heading("Workflow Board")).toBeNull();
  });

  it("moves between rows with the arrow keys", () => {
    show(lineup());
    const first = screen.getByRole("button", { name: "Notes" });
    first.focus();
    fireEvent.keyDown(first, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Repository Lens" }));
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Workflow Board" }));
  });

  it("tells same-named plugins from different publishers apart", () => {
    show([
      installation({ installationId: "one", manifest: manifest({ id: "acme.board", name: "Board" }) }),
      installation({ installationId: "two", manifest: manifest({ id: "other.board", name: "Board" }) }),
    ]);
    expect(screen.getByRole("button", { name: "Board (acme.board)" })).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Turn off Board (other.board)" })).toBeTruthy();
  });

  it("removes the selected plugin with its data only when asked, then shows the next one", async () => {
    const [board, lens] = lineup();
    show([board!, lens!]);
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Also delete its settings and stored data" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove and delete data" }));
    await waitFor(() => expect(called("remove")).toEqual([{ installationId: "inst-board", deleteData: true }]));
    await waitFor(() => expect(heading("Repository Lens")).toBeTruthy());
  });

  it("drills into one plugin on a narrow screen and returns focus to its row", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 360 } as DOMRect);
    show(lineup());
    expect(heading("Notes")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Repository Lens" }));
    expect(document.activeElement).toBe(heading("Repository Lens"));
    expect(screen.queryByRole("list", { name: "Isolated plugins" })).toBeNull();
    // With the list gone, the editor carries the switch.
    expect(screen.getByRole("switch", { name: "Turn off Repository Lens" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "All plugins" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Repository Lens" }));
  });

  it("returns focus to search when the plugin left behind is filtered out", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 360 } as DOMRect);
    const [board, lens, notes] = lineup();
    const crashed = { status: "error" as const, error: "Crashed" };
    show([{ ...board!, ...crashed }, lens!, { ...notes!, ...crashed }]);
    fireEvent.click(screen.getByRole("button", { name: "Needs attention (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    push([{ ...board!, ...crashed }, lens!, notes!]);
    fireEvent.click(screen.getByRole("button", { name: "All plugins" }));
    expect(screen.queryByRole("button", { name: "Notes" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("searchbox", { name: "Search plugins" }));
  });

  it("opens a side panel view from Settings as a page", () => {
    const panel = installation({
      manifest: manifest({ views: [{ id: "panel", title: "Inspector", entry: "inspector.html", placement: "side-panel" }] }),
    });
    show([panel]);
    useUIStore.setState({ showSettings: true } as never);
    fireEvent.click(screen.getByRole("button", { name: "Open Inspector" }));
    expect(useUIStore.getState().showSettings).toBe(false);
  });
});

describe("inline settings", () => {
  it("keeps an unsaved edit through pushes and plugin switches, and saves only what changed", async () => {
    const [, lens] = lineup();
    show([configurable(), lens!]);
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Board label" }), { target: { value: "Sprint board" } });
    expect(screen.getByText("Unsaved changes")).toBeTruthy();

    // It stops on its host, and another device renames it, while the user types.
    push([configurable({ status: "error", error: "Crashed", settings: { label: "Elsewhere" } }), lens!]);
    expect((screen.getByRole("textbox", { name: "Board label" }) as HTMLInputElement).value).toBe("Sprint board");

    fireEvent.click(screen.getByRole("button", { name: "Repository Lens" }));
    expect(screen.getByRole("button", { name: "Workflow Board" }).textContent).toContain("Unsaved settings");
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    expect((screen.getByRole("textbox", { name: "Board label" }) as HTMLInputElement).value).toBe("Sprint board");

    respond = () => configurable({ settings: { label: "Sprint board" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(called("updateSettings")).toEqual([{ installationId: "inst-board", values: { label: "Sprint board" } }]));
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).toBeNull());
  });

  it("keeps a slow Save in flight across a plugin switch, without dropping an edit made after it", async () => {
    const [, lens] = lineup();
    let finish: () => void = () => {};
    respond = (method) =>
      method === "updateSettings"
        ? new Promise((resolve) => {
            finish = () => resolve(configurable({ settings: { label: "Sprint board" } }));
          })
        : undefined;
    show([configurable(), lens!]);
    const label = () => screen.getByRole("textbox", { name: "Board label" }) as HTMLInputElement;
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    fireEvent.change(label(), { target: { value: "Sprint board" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(called("updateSettings")).toHaveLength(1));

    // Switching away and back remounts the form; the Save is still running.
    fireEvent.click(screen.getByRole("button", { name: "Repository Lens" }));
    fireEvent.click(screen.getByRole("button", { name: "Workflow Board" }));
    expect(screen.getByRole("button", { name: "Saving…" })).toBeTruthy();
    expect(label().disabled).toBe(true);

    // Even an edit that lands while it runs is newer than what was sent, so it stays.
    act(() =>
      editPluginSettings(configurable(), {
        draft: { label: "Newer", limit: "10", compact: false, token: "" },
        clearedSecrets: new Set(),
      }),
    );
    await act(async () => finish());
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeTruthy());
    expect(label().value).toBe("Newer");
    expect(screen.getByText("Unsaved changes")).toBeTruthy();
  });

  it("follows the stored values while nothing is edited, and reverts an edit", () => {
    show([configurable()]);
    const label = () => screen.getByRole("textbox", { name: "Board label" }) as HTMLInputElement;
    push([configurable({ settings: { label: "Renamed elsewhere" } })]);
    expect(label().value).toBe("Renamed elsewhere");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(label(), { target: { value: "Mine" } });
    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    expect(label().value).toBe("Renamed elsewhere");
    expect(called("updateSettings")).toEqual([]);
  });

  it("names switches, validates numbers, and deletes a stored secret only when asked", async () => {
    respond = () => configurable();
    show([configurable()]);
    expect(screen.getByRole("switch", { name: "Compact cards" })).toBeTruthy();
    expect((screen.getByLabelText("API token") as HTMLInputElement).placeholder).toBe("Saved. Type to replace.");
    fireEvent.change(screen.getByRole("textbox", { name: "Card limit" }), { target: { value: "many" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(screen.getByText("Enter a number.")).toBeTruthy();
    expect(called("updateSettings")).toEqual([]);

    fireEvent.change(screen.getByRole("textbox", { name: "Card limit" }), { target: { value: "10" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete the saved API token" }));
    expect(screen.getByText("The saved value is deleted when you save.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(called("updateSettings")).toEqual([{ installationId: "inst-board", values: { token: "" } }]));
  });

  it("says unsaved settings go with a removed plugin", async () => {
    show([configurable()]);
    fireEvent.change(screen.getByRole("textbox", { name: "Board label" }), { target: { value: "Sprint" } });
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Your unsaved settings changes are discarded/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(called("remove")).toHaveLength(1));
    expect(usePluginSettingsDrafts.getState().drafts["inst-board"]).toBeUndefined();
  });
});

describe("RemovePlatformPluginDialog", () => {
  it("keeps data unless the user asks to delete it", async () => {
    const { RemovePlatformPluginDialog } = await import("./PlatformPluginDialogs");
    const onClose = vi.fn();
    resetPlatformPluginsForTests({ catalogue: { installations: [installation()] }, loaded: true });
    render(<RemovePlatformPluginDialog installation={installation()} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(called("remove")).toEqual([{ installationId: "inst-1" }]);

    cleanup();
    requests.length = 0;
    render(<RemovePlatformPluginDialog installation={installation()} onClose={onClose} />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Also delete its settings and stored data" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove and delete data" }));
    await waitFor(() => expect(called("remove")).toEqual([{ installationId: "inst-1", deleteData: true }]));
  });

  it("renders the host's real record shape (nulls included) and grants any known project, archived ones too", async () => {
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [{ workspace_id: "ws-1", project_uid: "uid-app", project_root: "/code/app", cwd: "/code/app", surfaces: [] }],
        archived_workspaces: [{ workspace_id: "ws-old", project_uid: null, project_root: "/code/old app", cwd: "/code/old app" }],
      } as never,
    } as never);
    const host = {
      ...installation({
        manifest: manifest({ capabilities: ["threads.read"] }),
        grants: { capabilities: ["threads.read"], projectIds: ["uid-gone"] },
      }),
      pendingUpdate: null,
      observerError: null,
      recovery: { activeSource: "upstream", upstreamSource: null, previous: null, repair: null },
    };
    const normalized = readPluginCatalogue({ installations: [host], recoverySupported: true })!;
    show([...normalized.installations], { recoverySupported: true });
    expect(screen.getByRole("button", { name: "Realm" }).textContent).toContain("Ready");
    expect(screen.getByText("Projects: Unknown project")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review permissions" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("checkbox", { name: "app" })).toBeTruthy();
    expect(within(dialog).getByRole("checkbox", { name: "old app" })).toBeTruthy();
    expect(within(dialog).getByText("/code/old app · archived")).toBeTruthy();
    // A granted project Codemux no longer knows can still be withdrawn.
    expect((within(dialog).getByRole("checkbox", { name: "Unknown project" }) as HTMLButtonElement).getAttribute("aria-checked")).toBe(
      "true",
    );
    useAppStore.setState({ appState: null } as never);
  });
});
