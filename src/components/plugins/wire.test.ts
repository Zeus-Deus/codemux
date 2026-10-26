import { describe, expect, it } from "vitest";
import {
  pluginApprovalBacklogStep,
  pluginRecoveryActions,
  pluginRecoveryDetails,
  pluginReviewSubject,
  pluginSettingsDraft,
  pluginSourceLabel,
  pluginStatusPresentation,
} from "./model";
import { readPluginCatalogue, readPluginInvokeResult } from "./wire";

/**
 * One installation exactly as `platform_transport.rs` serializes it: serde
 * writes `null` for every absent `Option`, sends secrets as `{ hasSecret }`,
 * keeps `requiredCapabilities` apart and unknown fields as they are.
 */
const hostInstallation = {
  installationId: "plugin-0123",
  generation: "6f0f3f5e-1d1b-4c38-9d4f-8a9e3b0c7d21",
  dataOwner: "1b5e1c90-6a3e-4c8f-a6a7-6f3b2c1d0e9f",
  manifest: {
    id: "acme.board",
    name: "Board",
    version: "1.0.0",
    apiVersion: 1,
    description: null,
    entry: "plugin.mjs",
    capabilities: ["ui"],
    requiredCapabilities: ["threads.read"],
    optionalCapabilities: [],
    actions: [{ id: "sync", title: "Sync", description: null, placements: ["palette"], agentCallable: false }],
    views: [
      { id: "home", title: "Home", entry: "home.html", placement: "page" },
      { id: "future", title: "Future", entry: "f.html", placement: "floating-window" },
    ],
    threadStatus: [{ id: "live", title: "Live", viewId: null }],
    nativeTrees: [{ id: "work", title: "Work", description: null }],
    sidebarViews: [{ id: "all", title: "All", groupBy: "project", quietAfterDays: null, emptyProjects: null }],
    settings: [
      { key: "token", label: "Token", type: "secret", description: null },
      { key: "team", label: "Team", type: "string", description: null },
      { key: "mode", label: "Mode", type: "select", description: null },
    ],
    futureField: { x: 1 },
  },
  source: { kind: "git", url: "https://example.com/board.git", ref: null, commit: "abc123", subdir: null },
  digest: "a".repeat(64),
  enabled: true,
  consentedDigest: "a".repeat(64),
  status: "idle",
  error: null,
  observerError: null,
  compatibility: {
    available: true,
    apiVersion: 1,
    missingCapabilities: [],
    unsupportedOptionalCapabilities: [],
    unsupportedRequirements: [],
  },
  grants: { capabilities: ["ui", "threads.read"], projectIds: ["/repo"] },
  settings: { token: { hasSecret: true }, team: null },
  updatedAt: "2026-10-24T10:00:00Z",
  pendingUpdate: null,
  recovery: { activeSource: "upstream", upstreamSource: null, previous: null, repair: null },
};

const read = (installation: unknown) => readPluginCatalogue({ installations: [installation] })!.installations[0]!;

describe("readPluginCatalogue on the host's wire shape", () => {
  it("turns nulls into absent fields so no model rule mistakes them for values", () => {
    const installation = read(hostInstallation);
    expect(installation.pendingUpdate).toBeUndefined();
    expect(installation.recovery).toEqual({ activeSource: "upstream" });
    expect(installation.source).toEqual({ kind: "git", url: "https://example.com/board.git", commit: "abc123" });
    expect(pluginSourceLabel(installation.source)).toBe("https://example.com/board.git");
    expect(installation.manifest.threadStatus).toEqual([{ id: "live", title: "Live" }]);
    expect(installation.manifest.sidebarViews).toEqual([{ id: "all", title: "All", groupBy: "project" }]);
    // Before normalizing, `pendingUpdate: null` crashed the review and recovery rows.
    expect(pluginReviewSubject(installation).kind).toBe("install");
    expect(pluginApprovalBacklogStep(installation)).toBe("consent");
    expect(() => pluginRecoveryDetails(installation, (iso) => iso)).not.toThrow();
    expect(pluginRecoveryActions(installation, true).usePrevious).toBeNull();
  });

  it("counts required capabilities and drops only what this client cannot use", () => {
    const installation = read(hostInstallation);
    expect(installation.manifest.capabilities).toEqual(["ui", "threads.read"]);
    expect(installation.manifest.views.map((view) => view.id)).toEqual(["home"]);
    expect(installation.manifest.settings.map((setting) => setting.key)).toEqual(["token", "team"]);
  });

  it("keeps secrets as markers, never as values, and drops null settings", () => {
    const installation = read(hostInstallation);
    expect(installation.settings).toEqual({});
    expect(installation.storedSecrets).toEqual(["token"]);
    expect(pluginSettingsDraft(installation)).toEqual({ token: "", team: "" });
  });

  it("reads a pending update and the kept previous version", () => {
    const installation = read({
      ...hostInstallation,
      pendingUpdate: {
        kind: "previous",
        manifest: { ...hostInstallation.manifest, version: "0.9.0" },
        source: { kind: "local", directory: "/plugins/board" },
        digest: "c".repeat(64),
        generation: `pending-${"c".repeat(64)}`,
        updatedAt: "2026-10-24T10:00:00Z",
      },
      recovery: {
        activeSource: "upstream",
        upstreamSource: null,
        previous: { version: "0.9.0", digest: "c".repeat(64), source: { kind: "local", directory: "/plugins/board" } },
        repair: null,
      },
    });
    expect(pluginReviewSubject(installation)).toMatchObject({ kind: "update", revision: "previous", digest: "c".repeat(64) });
    // The installed revision stays offered while the staged one waits.
    expect(installation.digest).toBe("a".repeat(64));
    expect(pluginRecoveryActions(installation, true).usePrevious).toBeNull();
  });

  it("keeps unreadable and future records visible without letting them run or crash", () => {
    const catalogue = readPluginCatalogue({
      installations: [
        // The host's placeholder for a record it could not read.
        {
          installationId: "plugin-broken",
          generation: "unavailable",
          dataOwner: "plugin-broken",
          recordIssue: "Plugin record schema 9 is newer than this Codemux",
          manifest: { id: "plugin-broken", name: "Unavailable plugin", version: "0.0.0", apiVersion: 0, entry: "", capabilities: [], actions: [], views: [], settings: [] },
          source: { kind: "local", directory: "" },
          digest: "",
          enabled: false,
          consentedDigest: null,
          status: "incompatible",
          error: "Plugin record schema 9 is newer than this Codemux",
          grants: { capabilities: [], projectIds: [] },
          settings: {},
          updatedAt: "",
        },
        { ...hostInstallation, installationId: "plugin-future", status: "migrating", manifest: { apiVersion: 2 } },
        { installationId: 42 },
        "garbage",
        null,
      ],
      apiVersion: 1,
    })!;
    expect(catalogue.installations.map((entry) => entry.installationId)).toEqual(["plugin-broken", "plugin-future"]);
    const future = catalogue.installations[1]!;
    expect(future.status).toBe("incompatible");
    expect(future.manifest).toMatchObject({ id: "plugin-future", name: "plugin-future", actions: [], views: [] });
    expect(pluginStatusPresentation(future)).toEqual({ label: "Not running", tone: "muted", primary: "none" });
    expect(pluginStatusPresentation(catalogue.installations[0]!).label).toBe("Read-only");
  });

  it("rejects an answer that is not a catalogue", () => {
    expect(readPluginCatalogue(null)).toBeNull();
    expect(readPluginCatalogue({ installations: "x" })).toBeNull();
    expect(readPluginCatalogue({ installations: [], unavailable: { message: "store locked" } })).toEqual({
      installations: [],
      unavailable: { message: "store locked" },
    });
  });
});

describe("readPluginInvokeResult", () => {
  it("drops nulls and counts intents this client cannot read instead of applying them", () => {
    expect(
      readPluginInvokeResult({
        message: null,
        data: { ok: true },
        intents: [{ type: "preview.open", url: "https://example.com" }, { type: "window.shake" }, { type: "preview.open", url: "javascript:alert(1)" }],
        unsupportedIntentCount: 1,
        images: [{ mimeType: "image/gif", data: "x" }],
      }),
    ).toEqual({
      data: { ok: true },
      intents: [{ type: "preview.open", url: "https://example.com" }],
      unsupportedIntentCount: 3,
    });
    expect(readPluginInvokeResult("nope")).toEqual({});
  });
});
