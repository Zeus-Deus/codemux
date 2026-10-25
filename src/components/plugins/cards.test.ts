import { describe, expect, it } from "vitest";
import type { PluginCardReference } from "./contract";
import { pluginCardFromToolItem, resolvePluginCardPresentation } from "./cards";
import { installation, manifest } from "./test-fixtures";

const card: PluginCardReference = {
  viewId: "card",
  summary: "Deployed to staging",
  data: { id: 7 },
  installationId: "inst-1",
  generation: "gen-1",
  dataOwner: "owner-1",
};

const call = (overrides: Record<string, unknown> = {}) => ({
  tool_name: "mcp__codemux__plugin_invoke",
  input: {},
  status: "done" as const,
  result_content: { card },
  ...overrides,
});

describe("pluginCardFromToolItem", () => {
  it("reads the card from the result object, structuredContent, JSON text and MCP content blocks", () => {
    expect(pluginCardFromToolItem(call())).toEqual(card);
    expect(pluginCardFromToolItem(call({ result_content: { structuredContent: { card } } }))).toEqual(card);
    expect(pluginCardFromToolItem(call({ result_content: JSON.stringify({ card }) }))).toEqual(card);
    expect(
      pluginCardFromToolItem(call({ result_content: [{ type: "text", text: JSON.stringify({ message: "ok", card }) }] })),
    ).toEqual(card);
    expect(
      pluginCardFromToolItem(call({ result_content: { content: [{ type: "text", text: JSON.stringify({ card }) }] } })),
    ).toEqual(card);
  });

  it("matches Codex envelopes and server-prefixed names, not same-named tools elsewhere", () => {
    expect(
      pluginCardFromToolItem(call({ tool_name: "mcpToolCall", input: { tool: "codemux_mcp__codemux__plugin_invoke" } })),
    ).toEqual(card);
    expect(pluginCardFromToolItem(call({ tool_name: "mcp__other__plugin_invoke" }))).toBeUndefined();
  });

  it("has no card while running, on error, or when the reference is malformed or oversized", () => {
    expect(pluginCardFromToolItem(call({ status: "running" }))).toBeUndefined();
    expect(pluginCardFromToolItem(call({ result_content: { isError: true, card } }))).toBeUndefined();
    expect(pluginCardFromToolItem(call({ result_content: { card: { ...card, installationId: "../x" } } }))).toBeUndefined();
    expect(
      pluginCardFromToolItem(call({ result_content: { card: { ...card, data: { blob: "x".repeat(3_000) } } } })),
    ).toBeUndefined();
  });
});

describe("resolvePluginCardPresentation", () => {
  const live = installation({
    manifest: manifest({ views: [{ id: "card", title: "Card", entry: "card.html", placement: "conversation" }] }),
    grants: { capabilities: ["ui", "ui.conversation-cards"], projectIds: [] },
  });
  const resolve = (overrides: Partial<Parameters<typeof resolvePluginCardPresentation>[0]> = {}) =>
    resolvePluginCardPresentation({ card, supported: true, installations: [live], executionDisabled: false, ...overrides });

  it("mounts the view only when the owning installation can serve it now", () => {
    expect(resolve().kind).toBe("live");
  });

  it("never mounts another installation's view, even one claiming the same data owner", () => {
    // Another source with the same manifest id gets another installation id; the host refuses it.
    const otherSource = { ...live, installationId: "inst-2" };
    expect(resolve({ installations: [otherSource] })).toMatchObject({
      kind: "summary",
      pluginName: null,
      reason: "This card's plugin is not installed here.",
    });
  });

  it("keeps only the summary once the plugin's data for the card was deleted", () => {
    expect(resolve({ installations: [{ ...live, dataOwner: "owner-2" }] })).toEqual({
      kind: "summary",
      pluginName: "Realm",
      reason: "The plugin's data for this card was deleted.",
    });
  });

  it("falls back to the recorded summary with a reason", () => {
    expect(resolve({ installations: null })).toEqual({ kind: "summary", pluginName: null, reason: null });
    expect(resolve({ executionDisabled: true })).toMatchObject({ kind: "summary", reason: "Plugin code is turned off." });
    expect(resolve({ installations: [] })).toMatchObject({ kind: "summary", pluginName: null });
    expect(resolve({ installations: [{ ...live, enabled: false }] })).toMatchObject({
      kind: "summary",
      pluginName: "Realm",
      reason: "The plugin is turned off.",
    });
    expect(resolve({ installations: [{ ...live, grants: { capabilities: ["ui"], projectIds: [] } }] })).toMatchObject({
      reason: "The plugin is not allowed to show cards.",
    });
    expect(resolve({ installations: [{ ...live, manifest: manifest() }] })).toMatchObject({
      reason: "This version of the plugin no longer shows this card.",
    });
  });
});
