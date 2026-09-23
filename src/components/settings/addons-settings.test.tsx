import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@/lib/addons/platform", () => ({
  refreshAddons: vi.fn().mockResolvedValue(undefined),
  activeAddonWorkspace: () => "workspace-1",
}));
vi.mock("@/lib/addons/bridge", () => ({
  addonInvoke: vi.fn().mockResolvedValue(null),
  resubscribeAddons: vi.fn().mockResolvedValue(undefined),
}));
const remote = vi.hoisted(() => ({ client: false }));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => remote.client,
}));
import { AddonsSettings } from "./addons-settings";
import { useAddonsStore } from "@/stores/addons-store";
import { addonInvoke, resubscribeAddons } from "@/lib/addons/bridge";
import { activeAddonWorkspace } from "@/lib/addons/platform";
import type {
  AddonInstallation,
  AddonManifest,
  AddonReview,
} from "@/lib/addons/types";
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
beforeEach(() => {
  vi.mocked(resubscribeAddons).mockClear();
  useAddonsStore.setState({
    installed: [],
    loaded: true,
    paused: false,
    warnings: [],
    error: null,
    developmentReview: null,
    credentialStates: {},
    registryError: null,
    interruptedActivations: [],
  });
});
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
  let configurationLoads = 0;
  vi.mocked(addonInvoke)
    .mockReset()
    .mockImplementation((command) =>
      command !== "addon_settings_get"
        ? Promise.resolve(null)
        : configurationLoads++ === 0
          ? pending
          : Promise.resolve({
              owner: "current-owner",
              repository: "current-repository",
            }),
    );
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

// ── Manager states against the host contract ─────────────────────────
const catalogInstall = (
  fields: Partial<AddonInstallation> = {},
): AddonInstallation => ({
  ...installation,
  installationId: "catalog-fixture",
  source: {
    kind: "catalog",
    publisher: "Reviewed Publisher",
    repository: "https://github.com/example/issue-companion",
  },
  catalog: {
    publisher: "Reviewed Publisher",
    repository: "https://github.com/example/issue-companion",
    tier: "community",
    listed: true,
  },
  updateAvailable: null,
  ...fields,
});
const noAccess = { permissions: [], http: [], credentials: [] };
const reviewOf = (fields: Partial<AddonReview> = {}): AddonReview => ({
  token: "review-token",
  manifest: { ...(manifest as AddonManifest), version: "1.1.0" },
  digest: "b".repeat(64),
  source: {
    kind: "catalog",
    publisher: "Reviewed Publisher",
    repository: "https://github.com/example/issue-companion",
  },
  replacesSource: false,
  expandsAccess: true,
  compressedBytes: 4096,
  retainedData: null,
  installed: null,
  added: {
    permissions: ["composer.append", "external.open"],
    http: [{ origin: "https://api.github.com", methods: ["GET"] }],
    credentials: [],
  },
  removed: noAccess,
  catalog: {
    publisher: "Reviewed Publisher",
    repository: "https://github.com/example/issue-companion",
    tier: "community",
    listed: true,
  },
  ...fields,
});
const installedRelease = (desiredEnabled: boolean) => ({
  version: "1.0.0",
  digest: "a".repeat(64),
  source: reviewOf().source,
  desiredEnabled,
  capabilities: { permissions: [], http: [], credentials: [] },
});
/** Route commands to results; anything else resolves null. */
function answer(
  results: Record<string, (args?: Record<string, unknown>) => unknown>,
) {
  vi.mocked(addonInvoke)
    .mockReset()
    .mockImplementation((command, args) =>
      command in results
        ? Promise.resolve().then(() => results[command](args))
        : Promise.resolve(null),
    );
}
const reject = (code: string, message: string): never => {
  throw { message, data: { code } };
};
const row = (name: string) =>
  screen.getByRole("heading", { name: new RegExp(name) }).closest("article")!;
async function openReview(review: AddonReview) {
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Install from link / ID" }),
  );
  fireEvent.change(
    screen.getByRole("textbox", { name: "Add-on install link or ID" }),
    { target: { value: review.manifest.id } },
  );
  fireEvent.click(screen.getByRole("button", { name: "Find release" }));
  return screen.findByRole("dialog", {
    name: `Review ${review.manifest.name}`,
  });
}

it("offers Retry and Disable for a failed add-on, and Disable revokes it", async () => {
  // Effects of the add-on are fenced for the whole disable call.
  const fenced: (number | undefined)[] = [];
  answer({
    addon_disable: () => {
      fenced.push(useAddonsStore.getState().revoking["codemux.issue-companion"]);
      return null;
    },
  });
  useAddonsStore.setState({
    revoking: {},
    installed: [
      {
        ...installation,
        status: "failed-disabled",
        failure: "Plugin host stopped: Plugin CPU deadline exceeded",
      },
    ],
  });
  render(<AddonsSettings />);
  const failed = within(row("Issue Companion"));
  expect(
    failed.getByText("Plugin host stopped: Plugin CPU deadline exceeded"),
  ).toBeInTheDocument();
  fireEvent.click(failed.getByRole("button", { name: "Disable" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_disable", {
      id: "codemux.issue-companion",
    }),
  );
  await waitFor(() =>
    expect(failed.getByRole("button", { name: "Retry" })).toBeEnabled(),
  );
  expect(fenced).toEqual([1]);
  expect(useAddonsStore.getState().revoking).toEqual({});
  fireEvent.click(failed.getByRole("button", { name: "Retry" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_enable", {
      id: "codemux.issue-companion",
    }),
  );
});

it("never offers Enable for blocked or incompatible releases and says why", () => {
  useAddonsStore.setState({
    installed: [
      catalogInstall({
        desiredEnabled: false,
        status: "blocked-disabled",
        failure: "Catalog block: Sends data to an undeclared service",
        catalog: { ...catalogInstall().catalog!, listed: false },
      }),
      {
        ...installation,
        installationId: "incompatible-fixture",
        manifest: {
          ...(manifest as AddonManifest),
          id: "local.future",
          name: "Future Package",
          api: "^2.0.0",
        },
        desiredEnabled: false,
        status: "incompatible-disabled",
        failure: null,
        compatibility: {
          api: "^2.0.0",
          hostApi: "1.0.0",
          platforms: ["linux-x64"],
          platform: "linux-x64",
          compatible: false,
          reason: "Requires plugin API ^2.0.0; this CodeMux supports 1.0.0",
        },
      },
    ],
  });
  render(<AddonsSettings />);
  const blocked = within(row("Issue Companion"));
  expect(blocked.queryByRole("button", { name: "Enable" })).toBeNull();
  // Disabling would record the blocked release as merely disabled.
  expect(blocked.queryByRole("button", { name: "Disable" })).toBeNull();
  expect(
    blocked.getByText("Catalog block: Sends data to an undeclared service"),
  ).toBeInTheDocument();
  expect(blocked.getByText(/No longer listed/)).toBeInTheDocument();
  const incompatible = within(row("Future Package"));
  expect(incompatible.queryByRole("button", { name: "Enable" })).toBeNull();
  expect(incompatible.queryByRole("button", { name: "Disable" })).toBeNull();
  expect(
    incompatible.getByText(
      "Requires plugin API ^2.0.0; this CodeMux supports 1.0.0",
    ),
  ).toBeInTheDocument();
});

it("lets an incompatible add-on that is still meant to run be disabled", async () => {
  answer({});
  // The host keeps desiredEnabled while this CodeMux cannot run the release,
  // so it would start again by itself after an upgrade.
  useAddonsStore.setState({
    installed: [
      {
        ...installation,
        status: "incompatible-disabled",
        failure: "Requires plugin API ^2.0.0; this CodeMux supports 1.0.0",
      },
    ],
  });
  render(<AddonsSettings />);
  const item = within(row("Issue Companion"));
  expect(item.queryByRole("button", { name: "Enable" })).toBeNull();
  expect(item.queryByRole("button", { name: "Retry" })).toBeNull();
  fireEvent.click(item.getByRole("button", { name: "Disable" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_disable", {
      id: "codemux.issue-companion",
    }),
  );
});

it("shows the reviewed catalog publisher, not the manifest author, as the identity", () => {
  useAddonsStore.setState({ installed: [catalogInstall()] });
  render(<AddonsSettings />);
  const item = within(row("Issue Companion"));
  expect(item.getByText(/Published by Reviewed Publisher/)).toBeInTheDocument();
  expect(item.getByText(/Community/)).toBeInTheDocument();
  // The manifest claims "CodeMux"; author text never becomes the identity.
  expect(item.queryByText(/CodeMux/)).toBeNull();
  // Only an official listing carries the official mark.
  expect(item.queryByRole("img", { name: "Official" })).toBeNull();
});

it("shows available updates and reports an up-to-date check inline", async () => {
  answer({
    addon_check_update: () => ({
      upToDate: true,
      installedVersion: "1.0.0",
      availableVersion: null,
      review: null,
    }),
  });
  useAddonsStore.setState({
    installed: [catalogInstall({ updateAvailable: "1.2.0" })],
  });
  render(<AddonsSettings />);
  const item = within(row("Issue Companion"));
  expect(item.getByText("Update 1.2.0 available")).toBeInTheDocument();
  fireEvent.click(item.getByRole("button", { name: "Check for update" }));
  expect(await item.findByRole("status")).toHaveTextContent("Up to date");
  expect(addonInvoke).toHaveBeenCalledWith("addon_check_update", {
    id: "codemux.issue-companion",
  });
  expect(addonInvoke).not.toHaveBeenCalledWith(
    "addon_catalog_review",
    expect.anything(),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("explains a failed update check on its own row", async () => {
  answer({
    addon_check_update: () =>
      reject("NETWORK_DENIED", "Add-on download failed"),
  });
  useAddonsStore.setState({ installed: [catalogInstall()] });
  render(<AddonsSettings />);
  const item = within(row("Issue Companion"));
  fireEvent.click(item.getByRole("button", { name: "Check for update" }));
  expect(await item.findByRole("alert")).toHaveTextContent(
    "Network unavailable. Add-on download failed",
  );
});

it("updates a disabled add-on with one Update button that keeps it disabled", async () => {
  const update = reviewOf({
    installed: installedRelease(false),
    added: {
      permissions: ["external.open"],
      http: [{ origin: "https://uploads.example.com", methods: ["PUT"] }],
      credentials: [],
    },
    removed: { permissions: ["git.read"], http: [], credentials: [] },
  });
  answer({
    addon_check_update: () => ({
      upToDate: false,
      installedVersion: "1.0.0",
      availableVersion: "1.1.0",
      review: update,
    }),
  });
  useAddonsStore.setState({
    installed: [
      catalogInstall({ desiredEnabled: false, status: "installed-disabled" }),
    ],
  });
  render(<AddonsSettings />);
  fireEvent.click(
    within(row("Issue Companion")).getByRole("button", {
      name: "Check for update",
    }),
  );
  const dialog = within(
    await screen.findByRole("dialog", { name: "Review Issue Companion" }),
  );
  expect(dialog.getByText(/Update 1\.0\.0 → 1\.1\.0/)).toBeInTheDocument();
  const added = within(dialog.getByRole("region", { name: "New access" }));
  expect(
    added.getByText("Open HTTPS links after your interaction"),
  ).toBeInTheDocument();
  expect(added.getByText("PUT https://uploads.example.com")).toBeInTheDocument();
  const removed = within(
    dialog.getByRole("region", { name: "Removed access" }),
  );
  expect(removed.getByText("Read a bounded Git summary")).toBeInTheDocument();
  expect(dialog.queryByRole("button", { name: "Install & enable" })).toBeNull();
  expect(dialog.queryByRole("checkbox", { name: /Enable/ })).toBeNull();
  fireEvent.click(dialog.getByRole("button", { name: "Update to 1.1.0" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_accept_review", {
      token: "review-token",
      enable: false,
      replaceSource: false,
      restoreData: false,
    }),
  );
});

it("says when an update adds no access", async () => {
  const update = reviewOf({
    installed: installedRelease(true),
    expandsAccess: false,
    added: noAccess,
  });
  answer({ addon_catalog_review: () => update });
  const dialog = within(await openReview(update));
  expect(dialog.getByText("No new access.")).toBeInTheDocument();
  expect(dialog.queryByRole("region", { name: "New access" })).toBeNull();
  fireEvent.click(dialog.getByRole("button", { name: "Update to 1.1.0" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith(
      "addon_accept_review",
      expect.objectContaining({ enable: true }),
    ),
  );
});

it("installs a new add-on only with an explicit enablement choice", async () => {
  const fresh = reviewOf({ manifest: manifest as AddonManifest });
  answer({ addon_catalog_review: () => fresh });
  const dialog = within(await openReview(fresh));
  expect(
    dialog.getByText(/Version 1\.0\.0 · Published by Reviewed Publisher/),
  ).toBeInTheDocument();
  expect(
    dialog.getByText(/Author \(as stated by the package\): CodeMux/),
  ).toBeInTheDocument();
  expect(dialog.getByText(/1 composer view$/)).toBeInTheDocument();
  expect(dialog.queryByRole("checkbox")).toBeNull();
  expect(dialog.getByRole("button", { name: "Install & enable" })).toBeEnabled();
  fireEvent.click(dialog.getByRole("button", { name: "Install" }));
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_accept_review", {
      token: "review-token",
      enable: false,
      replaceSource: false,
      restoreData: false,
    }),
  );
});

it("keeps both install buttons disabled until a source replacement is confirmed", async () => {
  const replacement = reviewOf({
    manifest: manifest as AddonManifest,
    replacesSource: true,
    installed: installedRelease(true),
    removed: { permissions: ["git.read"], http: [], credentials: [] },
  });
  answer({ addon_catalog_review: () => replacement });
  const dialog = within(await openReview(replacement));
  const install = dialog.getByRole("button", { name: "Install" });
  const enable = dialog.getByRole("button", { name: "Install & enable" });
  expect(install).toBeDisabled();
  expect(enable).toBeDisabled();
  expect(
    within(dialog.getByRole("region", { name: "Removed access" })).getByText(
      "Read a bounded Git summary",
    ),
  ).toBeInTheDocument();
  fireEvent.click(
    dialog.getByRole("checkbox", { name: /Replace the existing source/ }),
  );
  expect(install).toBeEnabled();
  fireEvent.click(enable);
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_accept_review", {
      token: "review-token",
      enable: true,
      replaceSource: true,
      restoreData: false,
    }),
  );
});

it("tells paused users that an accepted install starts only after Resume", async () => {
  const fresh = reviewOf({ manifest: manifest as AddonManifest });
  answer({ addon_catalog_review: () => fresh });
  useAddonsStore.setState({ paused: true });
  const dialog = within(await openReview(fresh));
  expect(dialog.getByRole("note")).toHaveTextContent(/Add-ons are paused/);
});

it("shows a rejected link lookup inside the open dialog, naming the cause", async () => {
  answer({
    addon_catalog_review: () =>
      reject(
        "INCOMPATIBLE_API",
        "Version 1.0.0 of codemux.issue-companion is not available for Linux x64",
      ),
  });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Install from link / ID" }),
  );
  fireEvent.change(
    screen.getByRole("textbox", { name: "Add-on install link or ID" }),
    {
      target: {
        value:
          "https://codemux.org/addons/codemux.issue-companion?version=1.0.0",
      },
    },
  );
  fireEvent.click(screen.getByRole("button", { name: "Find release" }));
  const alert = await within(screen.getByRole("dialog")).findByRole("alert");
  expect(alert).toHaveTextContent(
    "Not available for this device. Version 1.0.0 of codemux.issue-companion is not available for Linux x64",
  );
  answer({
    addon_catalog_review: () =>
      reject(
        "INCOMPATIBLE_API",
        "No release of codemux.issue-companion supports add-on API 1.0.0; version 2.0.0 requires ^2.0.0",
      ),
  });
  fireEvent.click(screen.getByRole("button", { name: "Find release" }));
  await waitFor(() =>
    expect(
      within(screen.getByRole("dialog")).getByRole("alert"),
    ).toHaveTextContent(/^Incompatible add-on API\. No release/),
  );
});

it("shows a failed install inside the review dialog", async () => {
  const fresh = reviewOf({ manifest: manifest as AddonManifest });
  answer({
    addon_catalog_review: () => fresh,
    addon_accept_review: () =>
      reject("STORAGE_UNAVAILABLE", "Not enough disk space"),
  });
  const dialog = within(await openReview(fresh));
  fireEvent.click(dialog.getByRole("button", { name: "Install & enable" }));
  expect(await dialog.findByRole("alert")).toHaveTextContent(
    "Not enough disk space",
  );
  expect(
    screen.getByRole("dialog", { name: "Review Issue Companion" }),
  ).toBeInTheDocument();
});

it("shows each credential's state and clears a stored one", async () => {
  answer({
    addon_settings_get: () => ({ owner: "", repository: "" }),
    addon_credential_clear: () => [],
  });
  useAddonsStore.setState({
    installed: [installation],
    credentialStates: {
      "codemux.issue-companion": { "github-token": "saved" },
    },
  });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  expect(
    screen.getByText("Stored in the system credential store"),
  ).toBeInTheDocument();
  fireEvent.click(
    screen.getByRole("button", { name: "Clear GitHub token (optional)" }),
  );
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith("addon_credential_clear", {
      id: "codemux.issue-companion",
      credentialId: "github-token",
    }),
  );
  await act(async () =>
    useAddonsStore.setState({
      credentialStates: {
        "codemux.issue-companion": { "github-token": "not-configured" },
      },
    }),
  );
  expect(screen.getByText("Not configured")).toBeInTheDocument();
  expect(
    screen.getByText(
      /Requests to https:\/\/api\.github\.com are sent without a credential/,
    ),
  ).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Clear/ })).toBeNull();
});

it("keeps what was typed in another credential field while one is saved", async () => {
  let finish!: () => void;
  answer({
    addon_settings_get: () => ({ owner: "", repository: "" }),
    addon_credential_set: () =>
      new Promise<null>((resolve) => {
        finish = () => resolve(null);
      }),
  });
  const base = manifest as AddonManifest;
  useAddonsStore.setState({
    installed: [
      {
        ...installation,
        manifest: {
          ...base,
          credentials: [
            ...base.credentials,
            {
              id: "uploads-token",
              label: "Uploads token",
              origin: "https://uploads.example.com",
              type: "bearer",
            },
          ],
        },
      },
    ],
  });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  const github = screen.getByLabelText("GitHub token (optional)");
  const uploads = screen.getByLabelText("Uploads token");
  fireEvent.change(github, { target: { value: "synthetic-github" } });
  fireEvent.click(screen.getAllByRole("button", { name: "Save" })[0]);
  await waitFor(() =>
    expect(addonInvoke).toHaveBeenCalledWith(
      "addon_credential_set",
      expect.objectContaining({ credentialId: "github-token" }),
    ),
  );
  // The other field stays editable while the first value is stored.
  fireEvent.change(uploads, { target: { value: "synthetic-uploads" } });
  await act(async () => finish());
  await waitFor(() => expect(github).toHaveValue(""));
  expect(uploads).toHaveValue("synthetic-uploads");
});

it("explains compatibility and shows bounded diagnostics without log text", async () => {
  answer({
    addon_settings_get: () => ({ owner: "", repository: "" }),
    addon_diagnostics: () => ({
      received: 1500,
      logs: [
        { at: Date.UTC(2026, 8, 23, 9, 0), level: "error", bytes: 120 },
        { at: Date.UTC(2026, 8, 23, 9, 1), level: "warn", bytes: 64 },
        { at: Date.UTC(2026, 8, 23, 9, 2), level: "log", bytes: 32 },
      ],
    }),
  });
  useAddonsStore.setState({
    installed: [
      {
        ...installation,
        previous: {
          manifest: { ...(manifest as AddonManifest), version: "0.9.0" },
        },
        compatibility: {
          api: "^1.0.0",
          hostApi: "1.0.0",
          platforms: ["linux-x64", "windows-x64"],
          platform: "linux-x64",
          compatible: true,
          reason: null,
        },
      },
    ],
  });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Configure / Permissions" }),
  );
  expect(
    screen.getByText(
      "Requires add-on API ^1.0.0; this CodeMux provides 1.0.0.",
    ),
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      /Built for Linux x64, Windows x64; this device is Linux x64/,
    ),
  ).toBeInTheDocument();
  expect(
    await screen.findByText(
      /1500 log entries this session · the latest 3 are kept/,
    ),
  ).toBeInTheDocument();
  expect(
    screen.getByText("1 error · 1 warning · 1 log"),
  ).toBeInTheDocument();
  expect(
    within(
      screen.getByRole("list", { name: "Recent log entries" }),
    ).getAllByRole("listitem"),
  ).toHaveLength(3);
  expect(addonInvoke).toHaveBeenCalledWith("addon_diagnostics", {
    id: "codemux.issue-companion",
  });
  // Rollback is reachable from the detail view too.
  fireEvent.click(screen.getByRole("button", { name: "Rollback" }));
  expect(
    await screen.findByRole("dialog", { name: "Restore 0.9.0?" }),
  ).toBeInTheDocument();
});

it("offers a registry reset, then reopens the add-on event stream", async () => {
  answer({
    addon_registry_reset: () => "/data/addons-v1-backup-20260923T101500Z",
  });
  useAddonsStore.setState({
    paused: true,
    error:
      "The add-on registry at /data/addons-v1 could not be opened: file is not a database. Reset it to start with no add-ons; the current files are kept as a backup.",
    registryError: { path: "/data/addons-v1", cause: "file is not a database" },
  });
  render(<AddonsSettings />);
  expect(
    screen.queryByText(/Your installed packages and settings are kept/),
  ).toBeNull();
  expect(screen.getByRole("alert")).toHaveTextContent(
    /could not be opened: file is not a database/,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Reset add-on registry" }),
  );
  const dialog = within(
    await screen.findByRole("dialog", { name: "Reset the add-on registry?" }),
  );
  fireEvent.click(dialog.getByRole("button", { name: "Reset registry" }));
  expect(
    await screen.findByText(
      "The add-on registry was reset. The previous files were moved to /data/addons-v1-backup-20260923T101500Z.",
    ),
  ).toBeInTheDocument();
  expect(addonInvoke).toHaveBeenCalledWith("addon_registry_reset");
  expect(resubscribeAddons).toHaveBeenCalledWith(activeAddonWorkspace);
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("keeps a failed registry reset inside its dialog", async () => {
  answer({
    addon_registry_reset: () =>
      reject(
        "STORAGE_UNAVAILABLE",
        "Could not move /data/addons-v1 aside. Close other CodeMux windows and retry.",
      ),
  });
  useAddonsStore.setState({
    paused: true,
    error:
      "The add-on registry at /data/addons-v1 could not be opened: locked.",
    registryError: { path: "/data/addons-v1", cause: "locked" },
  });
  render(<AddonsSettings />);
  fireEvent.click(
    screen.getByRole("button", { name: "Reset add-on registry" }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Reset registry" }),
  );
  expect(
    await within(screen.getByRole("dialog")).findByRole("alert"),
  ).toHaveTextContent("Could not move /data/addons-v1 aside.");
  expect(resubscribeAddons).not.toHaveBeenCalled();
});

it("offers Reset instead of Resume for a broken registry and leaves no alert once it recovers", async () => {
  answer({
    addon_registry_reset: () => {
      // The host opens a fresh registry; its inventory no longer fails.
      useAddonsStore.setState({
        paused: false,
        error: null,
        registryError: null,
      });
      return "/data/addons-v1-backup-20260923T101500Z";
    },
  });
  useAddonsStore.setState({
    paused: true,
    error:
      "The add-on registry at /data/addons-v1 could not be opened: file is not a database. Reset it to start with no add-ons; the current files are kept as a backup.",
    registryError: { path: "/data/addons-v1", cause: "file is not a database" },
  });
  render(<AddonsSettings />);
  expect(screen.getAllByRole("alert")).toHaveLength(1);
  expect(screen.queryByRole("button", { name: "Resume add-ons" })).toBeNull();
  fireEvent.click(
    screen.getByRole("button", { name: "Reset add-on registry" }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Reset registry" }),
  );
  expect(
    await screen.findByText(/The add-on registry was reset/),
  ).toBeInTheDocument();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(
    screen.getByRole("button", { name: "Pause all add-ons" }),
  ).toBeInTheDocument();
});

it("replaces an earlier page error once a dialog operation succeeds", async () => {
  answer({
    addon_disable: () =>
      reject("STORAGE_UNAVAILABLE", "Could not save the add-on state"),
    addon_remove: () => [],
  });
  useAddonsStore.setState({ installed: [installation] });
  render(<AddonsSettings />);
  const item = within(row("Issue Companion"));
  fireEvent.click(item.getByRole("button", { name: "Disable" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not save the add-on state",
  );
  await waitFor(() =>
    expect(item.getByRole("button", { name: "Disable" })).toBeEnabled(),
  );
  fireEvent.click(item.getByRole("button", { name: "Remove" }));
  const dialog = within(
    await screen.findByRole("dialog", { name: "Remove Issue Companion?" }),
  );
  fireEvent.click(dialog.getByRole("button", { name: "Remove add-on" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(addonInvoke).toHaveBeenCalledWith("addon_remove", {
    id: "codemux.issue-companion",
    keepData: false,
  });
  expect(screen.queryByRole("alert")).toBeNull();
});

it("reopens the event stream after Resume retries a failed open", async () => {
  answer({});
  useAddonsStore.setState({
    paused: true,
    error: "App resources are unavailable",
  });
  render(<AddonsSettings />);
  fireEvent.click(screen.getByRole("button", { name: "Resume add-ons" }));
  await waitFor(() =>
    expect(resubscribeAddons).toHaveBeenCalledWith(activeAddonWorkspace),
  );
  expect(addonInvoke).toHaveBeenCalledWith("addon_resume");
});

it("names the add-on whose activation an unclean exit interrupted", () => {
  useAddonsStore.setState({
    paused: true,
    installed: [installation],
    interruptedActivations: ["codemux.issue-companion"],
  });
  render(<AddonsSettings />);
  expect(
    screen.getByText(/CodeMux closed while Issue Companion was starting/),
  ).toBeInTheDocument();
  expect(
    screen.queryByText(/All add-ons are paused\. Your installed/),
  ).toBeNull();
});

it("shows remote browsers only the explanatory entry and calls no add-on command", () => {
  remote.client = true;
  try {
    vi.mocked(addonInvoke).mockClear();
    useAddonsStore.setState({ installed: [installation] });
    render(<AddonsSettings />);
    expect(
      screen.getByText(/unavailable in a remote browser/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
    expect(addonInvoke).not.toHaveBeenCalled();
  } finally {
    remote.client = false;
  }
});
