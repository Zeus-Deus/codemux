import { describe, expect, it } from "vitest";
import { pluginStatusPresentation } from "./model";
import {
  pluginDuplicateHints,
  pluginMatchesQuery,
  pluginNeedsAttention,
  pluginRowSummary,
  sortPluginInstallations,
} from "./plugin-browser";
import { DIGEST_A, DIGEST_B, installation, manifest } from "./test-fixtures";

const needs = (entry: ReturnType<typeof installation>) => pluginNeedsAttention(entry, pluginStatusPresentation(entry));

describe("plugin list", () => {
  it("orders by name, then installation id, whatever the catalogue order", () => {
    const list = [
      installation({ installationId: "b", manifest: manifest({ name: "zeta" }) }),
      installation({ installationId: "c", manifest: manifest({ name: "Alpha" }) }),
      installation({ installationId: "a", manifest: manifest({ name: "alpha" }) }),
    ];
    expect(sortPluginInstallations(list).map((entry) => entry.installationId)).toEqual(["a", "c", "b"]);
  });

  it("searches name, id, description and source", () => {
    const entry = installation({
      manifest: manifest({ description: "Shows a repository overview" }),
      source: { kind: "git", url: "https://example.com/acme/realm.git", ref: "v2" },
    });
    for (const needle of ["realm", "acme.realm", "overview", "example.com", "#v2", ""]) {
      expect(pluginMatchesQuery(entry, needle)).toBe(true);
    }
    expect(pluginMatchesQuery(entry, "lens")).toBe(false);
  });

  it("counts what the user can act on, not an off plugin that merely can't run here", () => {
    expect(needs(installation())).toBe(false);
    expect(needs(installation({ consentedDigest: null, status: "needs-consent" }))).toBe(true);
    expect(needs(installation({ status: "quarantined" }))).toBe(true);
    expect(needs(installation({ observerError: "Delivery stopped." }))).toBe(true);
    // Off: it observes nothing, so a paused delivery is not news yet.
    expect(needs(installation({ enabled: false, status: "disabled", observerError: "Delivery stopped." }))).toBe(false);
    const unavailable = { compatibility: { available: false, missingCapabilities: ["future.cap"] }, status: "incompatible" as const };
    expect(needs(installation(unavailable))).toBe(true);
    expect(needs(installation({ ...unavailable, enabled: false }))).toBe(false);
    expect(
      needs(
        installation({
          pendingUpdate: {
            manifest: manifest({ version: "2.0.0" }),
            source: { kind: "local", directory: "/plugins/realm" },
            digest: DIGEST_B,
            generation: "gen-2",
            updatedAt: "2026-10-24T11:00:00.000Z",
          },
        }),
      ),
    ).toBe(true);
  });

  it("tells same-named plugins apart by publisher id, or by source when the id is shared too", () => {
    const hints = pluginDuplicateHints([
      installation({ installationId: "one", manifest: manifest({ id: "acme.board", name: "Board" }) }),
      installation({ installationId: "two", manifest: manifest({ id: "other.board", name: "Board" }) }),
      installation({
        installationId: "three",
        manifest: manifest({ id: "other.board", name: "board" }),
        source: { kind: "git", url: "https://example.com/board.git" },
      }),
      installation({ installationId: "four", manifest: manifest({ id: "acme.notes", name: "Notes" }) }),
    ]);
    expect(hints.get("one")).toBe("acme.board");
    expect(hints.get("two")).toBe("/plugins/realm");
    expect(hints.get("three")).toBe("https://example.com/board.git");
    expect(hints.has("four")).toBe(false);
  });

  it("summarises state, what waits, the problem and unsaved settings in one line", () => {
    const crashed = installation({
      status: "error",
      error: "Crashed",
      pendingUpdate: {
        kind: "local-repair",
        manifest: manifest(),
        source: { kind: "local", directory: "/repair" },
        digest: DIGEST_B,
        generation: "gen-2",
        updatedAt: "2026-10-24T11:00:00.000Z",
      },
    });
    expect(pluginRowSummary(crashed, pluginStatusPresentation(crashed), { unsavedSettings: true })).toBe(
      "Error · Local fix ready · Stopped after a failure · Unsaved settings",
    );
    const readOnly = installation({ recordIssue: "newer", consentedDigest: DIGEST_A });
    expect(pluginRowSummary(readOnly, pluginStatusPresentation(readOnly))).toBe("Read-only");
  });
});
