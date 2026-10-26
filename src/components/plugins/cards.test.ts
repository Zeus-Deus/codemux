import { describe, expect, it } from "vitest";
import type { ToolCallItem } from "@/lib/agent-chat/types";
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

const call = (overrides: Partial<ToolCallItem> = {}): ToolCallItem => ({
  kind: "tool_call",
  id: "item-1",
  seq: 1,
  tool_use_id: "call-1",
  tool_name: "mcp__codemux__plugin_invoke",
  input: {},
  status: "done",
  result_content: { card },
  approval_request_id: null,
  ...overrides,
});

// What Codemux's transport returns for a plugin result (`mcp_result`): the
// result as one JSON text block, echoed as structuredContent.
const mcpResult = (value: Record<string, unknown>) => ({
  isError: false,
  content: [{ type: "text", text: JSON.stringify(value) }],
  structuredContent: value,
});

// Codex app-server items as recorded: `input` is the started item, the result
// the completed one.
const dynamicCall = (completed: Record<string, unknown>) => {
  const started = {
    type: "dynamicToolCall",
    id: "call_dyn",
    tool: "codemux_mcp__codemux__plugin_invoke",
    arguments: '{"pluginId":"realm","action":"deploy"}',
    contentItems: null,
    success: null,
    status: "inProgress",
  };
  return call({ tool_name: "dynamicToolCall", input: started, result_content: { ...started, ...completed } });
};
const mcpCall = (completed: Record<string, unknown>, server = "codemux") => {
  const started = {
    type: "mcpToolCall",
    id: "call_mcp",
    server,
    tool: "plugin_invoke",
    arguments: { pluginId: "realm", action: "deploy" },
    result: null,
    error: null,
    status: "inProgress",
  };
  return call({ tool_name: "mcpToolCall", input: started, result_content: { ...started, ...completed } });
};

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

  it("reads recorded Codex dynamic-tool envelopes from their contentItems", () => {
    expect(
      pluginCardFromToolItem(
        dynamicCall({
          status: "completed",
          success: true,
          contentItems: [{ type: "inputText", text: JSON.stringify({ message: "Deployed", card }) }],
        }),
      ),
    ).toEqual(card);
    // A result with images keeps the JSON block first; the image block is skipped.
    expect(
      pluginCardFromToolItem(
        dynamicCall({
          status: "completed",
          success: true,
          contentItems: [
            { type: "inputText", text: JSON.stringify({ card }) },
            { type: "inputImage", imageUrl: "data:image/png;base64,AA==" },
          ],
        }),
      ),
    ).toEqual(card);
  });

  it("reads recorded Codex MCP envelopes from their server-scoped result", () => {
    expect(pluginCardFromToolItem(mcpCall({ status: "completed", result: mcpResult({ card }) }))).toEqual(card);
    // Structured content alone, without an echoing text block.
    expect(
      pluginCardFromToolItem(
        mcpCall({
          status: "completed",
          result: { content: [{ type: "text", text: "Deployed" }], structuredContent: { card } },
        }),
      ),
    ).toEqual(card);
  });

  it("matches only Codemux's plugin_invoke, not same-named tools elsewhere", () => {
    expect(pluginCardFromToolItem(call({ tool_name: "mcp__other__plugin_invoke" }))).toBeUndefined();
    expect(pluginCardFromToolItem(mcpCall({ status: "completed", result: mcpResult({ card }) }, "other"))).toBeUndefined();
    const elsewhere = dynamicCall({
      status: "completed",
      success: true,
      contentItems: [{ type: "inputText", text: JSON.stringify({ card }) }],
    });
    expect(
      pluginCardFromToolItem({
        ...elsewhere,
        input: { ...(elsewhere.input as object), tool: "codemux_mcp__other__plugin_invoke" },
      }),
    ).toBeUndefined();
  });

  it("rejects failed Codex envelopes even when the blocks carry a card", () => {
    const blocks = [{ type: "inputText", text: JSON.stringify({ card }) }];
    expect(pluginCardFromToolItem(dynamicCall({ status: "completed", success: false, contentItems: blocks }))).toBeUndefined();
    expect(pluginCardFromToolItem(dynamicCall({ status: "failed", success: null, contentItems: blocks }))).toBeUndefined();
    expect(
      pluginCardFromToolItem(mcpCall({ status: "completed", result: { ...mcpResult({ card }), isError: true } })),
    ).toBeUndefined();
    expect(
      pluginCardFromToolItem(mcpCall({ status: "failed", result: mcpResult({ card }), error: { message: "server gone" } })),
    ).toBeUndefined();
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
