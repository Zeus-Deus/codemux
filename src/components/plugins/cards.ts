/**
 * Conversation cards: what an agent's `plugin_invoke` call leaves in the
 * transcript. The recorded summary is the durable record and always renders;
 * the plugin's live view mounts only while this host can serve it now.
 */
import type { ToolCallItem } from "@/lib/agent-chat/types";
import {
  MAX_PLUGIN_CARD_BYTES,
  MAX_PLUGIN_CARD_DATA_BYTES,
  type PluginCardReference,
  type PluginInstallation,
  type PluginView,
} from "./contract";
import { isPluginActive, pluginInactiveReason } from "./model";

// Claude sees Codemux tools as `mcp__codemux__plugin_invoke` (or with the
// server prefix repeated), Codex as `codemux_mcp__codemux__plugin_invoke`;
// all end in the server-scoped name after `__`. A same-named tool on another
// MCP server does not match.
const TOOL_NAME = /(?:^|__)codemux__plugin_invoke$/;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** The tool's own name. Codex wraps dynamic tool calls in an `{ tool, arguments }` envelope. */
function toolName(item: Pick<ToolCallItem, "tool_name" | "input">): string {
  const input = item.input;
  if (
    (item.tool_name === "dynamicToolCall" || item.tool_name === "mcpToolCall") &&
    isRecord(input) &&
    typeof input.tool === "string"
  ) {
    return input.tool;
  }
  return item.tool_name;
}

export const isPluginInvokeTool = (item: Pick<ToolCallItem, "tool_name" | "input">) =>
  TOOL_NAME.test(toolName(item));

const IDENTIFIER = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/;
const encoder = new TextEncoder();

function readCardReference(value: unknown): PluginCardReference | undefined {
  if (!isRecord(value)) return undefined;
  const { viewId, summary, data, installationId, generation, dataOwner } = value;
  if (
    typeof viewId !== "string" ||
    !IDENTIFIER.test(viewId) ||
    typeof summary !== "string" ||
    summary.trim() === "" ||
    summary.length > 1_000 ||
    typeof installationId !== "string" ||
    !IDENTIFIER.test(installationId) ||
    typeof generation !== "string" ||
    !IDENTIFIER.test(generation) ||
    typeof dataOwner !== "string" ||
    !IDENTIFIER.test(dataOwner)
  ) {
    return undefined;
  }
  const card: PluginCardReference = {
    viewId,
    summary,
    installationId,
    generation,
    dataOwner,
    ...(data === undefined ? {} : { data }),
  };
  try {
    if (data !== undefined && encoder.encode(JSON.stringify(data)).byteLength > MAX_PLUGIN_CARD_DATA_BYTES) {
      return undefined;
    }
    if (encoder.encode(JSON.stringify(card)).byteLength > MAX_PLUGIN_CARD_BYTES) return undefined;
  } catch {
    return undefined;
  }
  return card;
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

/**
 * The invoke result object inside whatever the provider recorded: the object
 * itself, `{ structuredContent }`, a JSON string, or an MCP content array (or
 * `{ content: [...] }`) whose text block is that JSON.
 */
function resultObject(content: unknown, depth = 0): Record<string, unknown> | undefined {
  if (depth > 3 || content === null || content === undefined) return undefined;
  if (typeof content === "string") return resultObject(parseJson(content), depth + 1);
  if (Array.isArray(content)) {
    for (const block of content) {
      const text = isRecord(block) && typeof block.text === "string" ? block.text : null;
      const found = text === null ? undefined : resultObject(text, depth + 1);
      if (found?.card !== undefined) return found;
    }
    return undefined;
  }
  if (!isRecord(content)) return undefined;
  if (isRecord(content.structuredContent)) return content.structuredContent;
  if ("card" in content) return content;
  if (content.content !== undefined) return resultObject(content.content, depth + 1);
  return undefined;
}

type CardSource = Pick<ToolCallItem, "tool_name" | "input" | "status" | "result_content">;

/** Parsed once per recorded result: the transcript asks about every tool row on each pass. */
const cardCache = new WeakMap<object, { readonly content: unknown; readonly card: PluginCardReference | undefined }>();

/**
 * The card a completed `plugin_invoke` call carries, if any. The host stamped
 * its provenance; this only decides whether to ask for the view.
 */
export function pluginCardFromToolItem(item: CardSource): PluginCardReference | undefined {
  if (item.status !== "done" || !isPluginInvokeTool(item)) return undefined;
  const cached = cardCache.get(item);
  if (cached !== undefined && cached.content === item.result_content) return cached.card;
  const result = resultObject(item.result_content);
  const card = result === undefined || result.isError === true ? undefined : readCardReference(result.card);
  cardCache.set(item, { content: item.result_content, card });
  return card;
}

export type PluginCardPresentation =
  | { readonly kind: "live"; readonly installation: PluginInstallation; readonly view: PluginView }
  | {
      readonly kind: "summary";
      /** The plugin's current name, when it is still installed. */
      readonly pluginName: string | null;
      /** Why the card is only a summary; null while the plugin list is still loading. */
      readonly reason: string | null;
    };

export function resolvePluginCardPresentation(input: {
  readonly card: PluginCardReference;
  /** The host serves card views at all; null until known. */
  readonly supported: boolean | null;
  /** The whole catalogue (not the runnable subset), null until loaded. */
  readonly installations: ReadonlyArray<PluginInstallation> | null;
  readonly executionDisabled: boolean;
}): PluginCardPresentation {
  // The host serves a card only to the installation that stamped it (a
  // same-source reinstall keeps that id) while its data owner is unchanged.
  // A shared plugin id from another source never matches.
  const installation = input.installations?.find((entry) => entry.installationId === input.card.installationId) ?? null;
  const summary = (reason: string | null): PluginCardPresentation => ({
    kind: "summary",
    pluginName: installation?.manifest.name ?? null,
    reason,
  });
  if (input.supported === null) return summary(null);
  if (!input.supported) return summary("This version of Codemux cannot show plugin cards.");
  if (input.installations === null) return summary(null);
  if (input.executionDisabled) return summary("Plugin code is turned off.");
  if (installation === null) return summary("This card's plugin is not installed here.");
  if (installation.dataOwner !== undefined && installation.dataOwner !== input.card.dataOwner) {
    return summary("The plugin's data for this card was deleted.");
  }
  if (!isPluginActive(installation)) return summary(pluginInactiveReason(installation) ?? "The plugin is not running.");
  if (
    !installation.grants.capabilities.includes("ui") ||
    !installation.grants.capabilities.includes("ui.conversation-cards")
  ) {
    return summary("The plugin is not allowed to show cards.");
  }
  const view = installation.manifest.views.find((entry) => entry.id === input.card.viewId);
  if (view === undefined || view.placement !== "conversation") {
    return summary("This version of the plugin no longer shows this card.");
  }
  return { kind: "live", installation, view };
}
