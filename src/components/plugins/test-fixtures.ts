import type { PluginInstallation, PluginManifest } from "./contract";

export const DIGEST_A = "a".repeat(64);
export const DIGEST_B = "b".repeat(64);

export function manifest(overrides: Partial<PluginManifest> = {}): PluginManifest {
  return {
    id: "acme.realm",
    name: "Realm",
    version: "1.0.0",
    apiVersion: 1,
    entry: "dist/index.js",
    capabilities: ["ui"],
    actions: [],
    views: [],
    settings: [],
    ...overrides,
  };
}

/** An approved, running installation unless overridden. */
export function installation(overrides: Partial<PluginInstallation> = {}): PluginInstallation {
  return {
    installationId: "inst-1",
    generation: "gen-1",
    dataOwner: "owner-1",
    manifest: manifest(),
    source: { kind: "local", directory: "/plugins/realm" },
    digest: DIGEST_A,
    enabled: true,
    consentedDigest: DIGEST_A,
    status: "idle",
    error: null,
    settings: {},
    grants: { capabilities: ["ui"], projectIds: [] },
    updatedAt: "2026-10-24T10:00:00.000Z",
    ...overrides,
  };
}
