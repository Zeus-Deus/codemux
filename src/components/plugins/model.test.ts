import { describe, expect, it } from "vitest";
import {
  buildPluginSettingsPatch,
  describePluginCapabilities,
  initialPluginGrants,
  isPluginActive,
  parsePluginSourceInput,
  pluginActionEntries,
  pluginApprovalBacklogStep,
  pluginAttention,
  pluginBacklogDecision,
  pluginFailure,
  pluginInactiveReason,
  pluginRecoveryActions,
  pluginReviewSubject,
  pluginRunnableInstallations,
  pluginStatusPresentation,
  pluginViewEntries,
} from "./model";
import { DIGEST_A, DIGEST_B, installation, manifest } from "./test-fixtures";

describe("isPluginActive", () => {
  it("requires enabled, matching consent, compatibility and a running status", () => {
    expect(isPluginActive(installation())).toBe(true);
    expect(isPluginActive(installation({ enabled: false }))).toBe(false);
    expect(isPluginActive(installation({ consentedDigest: DIGEST_B }))).toBe(false);
    expect(isPluginActive(installation({ compatibility: { available: false, missingCapabilities: ["x"] } }))).toBe(false);
    expect(isPluginActive(installation({ status: "quarantined" }))).toBe(false);
  });

  it("offers nothing while plugin code is off", () => {
    expect(pluginRunnableInstallations({ installations: [installation()], executionDisabled: true })).toEqual([]);
    expect(pluginRunnableInstallations(null)).toEqual([]);
  });

  it("lists actions by placement and views only with ui granted, never conversation views", () => {
    const plugin = installation({
      manifest: manifest({
        actions: [{ id: "summarize", title: "Summarize", placements: ["thread-menu", "palette"] }],
        views: [
          { id: "home", title: "Home", entry: "home.html", placement: "page" },
          { id: "card", title: "Card", entry: "card.html", placement: "conversation" },
        ],
      }),
    });
    expect(pluginActionEntries([plugin], "thread-menu").map((entry) => entry.action.id)).toEqual(["summarize"]);
    expect(pluginActionEntries([plugin], "message-selection")).toEqual([]);
    expect(pluginViewEntries([plugin]).map((entry) => entry.view.id)).toEqual(["home"]);
    expect(pluginViewEntries([{ ...plugin, grants: { capabilities: [], projectIds: [] } }])).toEqual([]);
  });
});

describe("status", () => {
  it("keeps the user's on choice visible when a plugin becomes unavailable after an update", () => {
    const broken = installation({
      compatibility: { available: false, missingCapabilities: ["future.cap"] },
      status: "incompatible",
    });
    expect(pluginStatusPresentation(broken)).toEqual({ label: "Unavailable", tone: "error", primary: "none" });
    expect(broken.enabled).toBe(true);
    expect(pluginAttention(broken)?.message).toContain("future.cap");
    expect(pluginInactiveReason(broken)).toContain("future.cap");
  });

  it("asks for review when the installed bytes differ from the approved ones", () => {
    expect(pluginStatusPresentation(installation({ consentedDigest: null, status: "needs-consent" })).label).toBe(
      "Needs review",
    );
    expect(pluginStatusPresentation(installation({ consentedDigest: DIGEST_B })).label).toBe("Changed, needs review");
  });

  it("offers a restart for faults and nothing in safe mode", () => {
    expect(pluginStatusPresentation(installation({ status: "quarantined" })).primary).toBe("resume");
    expect(pluginStatusPresentation(installation(), { executionDisabled: true }).label).toBe("Paused · plugin code off");
  });

  it("does not nag about paused events while the plugin is off", () => {
    expect(pluginAttention(installation({ observerError: "Delivery failed" }))?.kind).toBe("events-paused");
    expect(pluginAttention(installation({ enabled: false, observerError: "Delivery failed" }))).toBeNull();
  });
});

describe("review", () => {
  it("approves the staged digest, not the running one, and lists what changed", () => {
    const plugin = installation({
      manifest: manifest({ capabilities: ["ui", "storage"], settings: [{ key: "token", label: "Token", type: "secret" }] }),
      pendingUpdate: {
        manifest: manifest({
          version: "2.0.0",
          capabilities: ["ui", "threads.read"],
          settings: [{ key: "token", label: "Token", type: "string" }],
        }),
        source: { kind: "local", directory: "/plugins/realm" },
        digest: DIGEST_B,
        generation: "gen-2",
        updatedAt: "2026-10-24T11:00:00.000Z",
      },
    });
    const subject = pluginReviewSubject(plugin);
    expect(subject.digest).toBe(DIGEST_B);
    expect(subject.revision).toBe("update");
    expect(subject.newCapabilities).toEqual(["threads.read"]);
    expect(subject.removedCapabilities).toEqual(["storage"]);
    expect(subject.settingChanges).toEqual([{ key: "token", label: "Token", change: "type-changed" }]);
  });

  it("never widens grants on re-approval without a click", () => {
    const plugin = installation({
      manifest: manifest({ capabilities: ["ui", "storage"] }),
      grants: { capabilities: ["ui"], projectIds: ["p1"] },
    });
    expect(initialPluginGrants(plugin)).toEqual({ capabilities: ["ui"], projectIds: ["p1"] });
    expect(initialPluginGrants({ ...plugin, consentedDigest: null }).capabilities).toEqual(["ui", "storage"]);
  });

  it("marks unknown required capabilities as blocking and unknown optional ones as not", () => {
    const described = describePluginCapabilities({ capabilities: ["ui", "teleport"], optionalCapabilities: ["warp"] });
    expect(described.find((entry) => entry.capability === "teleport")).toMatchObject({ known: false, optional: false });
    expect(described.find((entry) => entry.capability === "warp")).toMatchObject({ known: false, optional: true });
  });
});

describe("recovery", () => {
  it("offers repair, staging a current repair, and the previous version", () => {
    const plugin = installation({
      recovery: {
        activeSource: "upstream",
        previous: { version: "0.9.0", digest: DIGEST_B, source: { kind: "local", directory: "/old" } },
        repair: { workspaceId: "w1", directory: "/repairs/w1", baseDigest: DIGEST_A, baseGeneration: "gen-1", draft: "" },
      },
    });
    expect(pluginRecoveryActions(plugin, true)).toEqual({
      repair: "open",
      stageRepair: true,
      usePrevious: { version: "0.9.0", digest: DIGEST_B },
      runningLocalFix: false,
    });
    expect(pluginRecoveryActions(plugin, false).repair).toBeNull();
  });

  it("only asks about missed runs when some were missed", () => {
    expect(pluginBacklogDecision({ count: 0, afterSequence: 1, throughSequence: 4, requiresChoice: false })).toEqual({
      kind: "none",
    });
    expect(pluginBacklogDecision({ count: 0, afterSequence: 1, throughSequence: 4, requiresChoice: true })).toEqual({
      kind: "resolved",
      eventBacklog: { policy: "skip", throughSequence: 4 },
    });
    expect(pluginBacklogDecision({ count: 3, afterSequence: 1, throughSequence: 4, requiresChoice: true })).toEqual({
      kind: "choose",
      count: 3,
      throughSequence: 4,
    });
  });

  it("asks the missed-runs choice on the step that starts observing", () => {
    expect(pluginApprovalBacklogStep(installation({ consentedDigest: null }))).toBeNull();
    expect(pluginApprovalBacklogStep(installation())).toBe("consent");
    expect(pluginApprovalBacklogStep(installation({ enabled: false }))).toBe("enable");
  });
});

describe("sources, settings and failures", () => {
  it("accepts absolute folders and https git with a ref, rejects the rest", () => {
    expect(parsePluginSourceInput("/home/me/plugin")).toEqual({ ok: true, source: { kind: "local", directory: "/home/me/plugin" } });
    expect(parsePluginSourceInput("https://github.com/a/b.git#v1.2")).toEqual({
      ok: true,
      source: { kind: "git", url: "https://github.com/a/b.git", ref: "v1.2" },
    });
    expect(parsePluginSourceInput("https://github.com/a/b.git#feature%2Fx")).toEqual({
      ok: true,
      source: { kind: "git", url: "https://github.com/a/b.git", ref: "feature/x" },
    });
    // A malformed escape is feedback, not a thrown submit.
    for (const malformed of ["https://example.com/repo.git#%", "https://example.com/repo.git#v1%E0%A4%A", "https://example.com/r#%zz"]) {
      expect(parsePluginSourceInput(malformed)).toEqual({ ok: false, message: "The ref after # has an invalid % escape." });
    }
    expect(parsePluginSourceInput("http://github.com/a/b").ok).toBe(false);
    expect(parsePluginSourceInput("https://me:pw@github.com/a/b").ok).toBe(false);
    expect(parsePluginSourceInput("relative/dir").ok).toBe(false);
  });

  it("sends only changed values, keeps an empty secret, and deletes a cleared one", () => {
    const settings = [
      { key: "name", label: "Name", type: "string" as const },
      { key: "count", label: "Count", type: "number" as const, default: 2 },
      { key: "token", label: "Token", type: "secret" as const },
    ];
    expect(buildPluginSettingsPatch(settings, { name: "a" }, { name: "a", count: "3", token: "" })).toEqual({
      ok: true,
      values: { count: 3 },
    });
    expect(buildPluginSettingsPatch(settings, {}, { name: "", count: "2", token: "" }, new Set(["token"]))).toEqual({
      ok: true,
      values: { token: "" },
    });
    expect(buildPluginSettingsPatch(settings, {}, { name: "", count: "x", token: "" })).toEqual({
      ok: false,
      errors: { count: "Enter a number." },
    });
  });

  it("reads failures from objects, JSON strings and reason-prefixed strings", () => {
    expect(pluginFailure({ reason: "quarantined", message: "Stopped" })).toEqual({ reason: "quarantined", message: "Stopped" });
    expect(pluginFailure('{"reason":"busy","message":"Later"}')).toEqual({ reason: "busy", message: "Later" });
    expect(pluginFailure("invalid_source: Choose a folder")).toEqual({ reason: "invalid_source", message: "Choose a folder" });
    expect(pluginFailure("plain")).toEqual({ reason: "operation_failed", message: "plain" });
    expect(pluginFailure(undefined).message).toBe("The plugin request failed.");
  });
});
