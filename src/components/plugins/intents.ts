/**
 * Local intents: what a plugin result or view may ask of the client that
 * invoked it. They are applied only to that client, and only against where
 * the user was when they acted; a reply that lands after the user moved on
 * never writes into whatever became active.
 */
import type { PluginClientIntent, PluginInvokeResult, PluginManifest } from "./contract";

/** Where the user invoked a plugin, captured before the request goes out. */
export interface PluginInvocationOrigin {
  readonly installationId: string;
  /** Compared with the client's location when the result arrives. */
  readonly locationKey: string;
  /** The composer the action was invoked against, if any; inserts go only here. */
  readonly composerKey: string | null;
  readonly threadId: string | null;
}

type NavigationIntent = Exclude<PluginClientIntent, { readonly type: "composer.insert" }>;

export type PluginIntentStep =
  | {
      readonly kind: "insert";
      readonly composerKey: string;
      readonly text: string;
      /** False when the user left the origin; the client says where the text went. */
      readonly visible: boolean;
    }
  | {
      readonly kind: "navigate";
      readonly intent: NavigationIntent;
      /** `offer` leaves the user where they are and asks first. */
      readonly mode: "apply" | "offer";
    }
  | { readonly kind: "rejected"; readonly intent: PluginClientIntent; readonly reason: string };

function quoteText(text: string, quote: boolean | undefined): string {
  if (quote !== true) return text;
  return text
    .split("\n")
    .map((line) => (line === "" ? ">" : `> ${line}`))
    .join("\n");
}

/**
 * How each intent reaches the invoking client. At most one navigation applies
 * directly, and only while the user is still where they invoked; the rest are
 * offered.
 */
export function planPluginIntents(input: {
  readonly origin: PluginInvocationOrigin;
  readonly manifest: Pick<PluginManifest, "views">;
  readonly intents: ReadonlyArray<PluginClientIntent>;
  readonly currentLocationKey: string;
  /** Whether this client can open the url inside Codemux for the origin thread. */
  readonly canOpenPreview: boolean;
}): ReadonlyArray<PluginIntentStep> {
  const stayed = input.currentLocationKey === input.origin.locationKey;
  let navigated = false;
  // An applied navigation that leaves the origin thread hides its composer,
  // wherever the insert sits in the list.
  let leaves = false;
  const leavesOrigin = (intent: NavigationIntent): boolean => {
    switch (intent.type) {
      case "thread.reveal":
        return intent.threadId !== input.origin.threadId;
      case "view.open":
        // Side panels open beside the origin thread; anything else opens as a page.
        return !(
          input.origin.threadId !== null &&
          input.manifest.views.some((view) => view.id === intent.viewId && view.placement === "side-panel")
        );
      case "preview.open":
        return false;
    }
  };
  const navigate = (intent: NavigationIntent, canApply: boolean): PluginIntentStep => {
    const apply = stayed && !navigated && canApply;
    if (apply) {
      navigated = true;
      leaves = leavesOrigin(intent);
    }
    return { kind: "navigate", intent, mode: apply ? "apply" : "offer" };
  };
  const steps = input.intents.map((intent): PluginIntentStep => {
    switch (intent.type) {
      case "composer.insert":
        if (input.origin.composerKey === null) {
          return { kind: "rejected", intent, reason: "There is no composer for this action." };
        }
        if (intent.text.trim() === "") return { kind: "rejected", intent, reason: "The plugin returned empty text." };
        return {
          kind: "insert",
          composerKey: input.origin.composerKey,
          text: quoteText(intent.text, intent.quote),
          visible: stayed,
        };
      case "view.open": {
        const view = input.manifest.views.find((entry) => entry.id === intent.viewId);
        if (view === undefined) return { kind: "rejected", intent, reason: `The plugin has no view "${intent.viewId}".` };
        if (view.placement === "conversation") {
          return { kind: "rejected", intent, reason: "That view only opens from a card in chat." };
        }
        return navigate(intent, true);
      }
      case "thread.reveal":
        return navigate(intent, true);
      case "preview.open":
        return navigate(intent, input.canOpenPreview && input.origin.threadId !== null);
    }
  });
  return leaves ? steps.map((step) => (step.kind === "insert" ? { ...step, visible: false } : step)) : steps;
}

/** Intents from a newer plugin API that this client could not read. */
export function pluginUnsupportedIntentCount(result: PluginInvokeResult): number {
  const count = result.unsupportedIntentCount;
  return typeof count === "number" && Number.isSafeInteger(count) && count > 0 ? count : 0;
}

/** One notice per result, however many intents were dropped. */
export function pluginUnsupportedIntentNotice(pluginName: string, count: number) {
  return {
    title: `${pluginName} asked for something this app can't do yet`,
    description:
      count === 1
        ? "One request needs a newer version of Codemux and was skipped."
        : `${count} requests need a newer version of Codemux and were skipped.`,
  };
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const boundedText = (value: unknown, max: number): value is string =>
  typeof value === "string" && value.length <= max;
const IDENTIFIER = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/;

/**
 * A known intent, validated, or null. Views talk to the client directly, so
 * anything they send is untrusted input; unknown fields are dropped.
 */
export function decodePluginIntent(value: unknown): PluginClientIntent | null {
  if (!isRecord(value)) return null;
  switch (value.type) {
    case "composer.insert":
      if (!boundedText(value.text, 64_000)) return null;
      if (value.quote !== undefined && typeof value.quote !== "boolean") return null;
      return { type: "composer.insert", text: value.text, ...(value.quote === true ? { quote: true } : {}) };
    case "preview.open":
      if (!boundedText(value.url, 4_096) || !/^https?:\/\//.test(value.url)) return null;
      return { type: "preview.open", url: value.url };
    case "thread.reveal":
      if (!boundedText(value.threadId, 256) || value.threadId === "") return null;
      if (value.messageId !== undefined && !boundedText(value.messageId, 256)) return null;
      return {
        type: "thread.reveal",
        threadId: value.threadId,
        ...(typeof value.messageId === "string" ? { messageId: value.messageId } : {}),
      };
    case "view.open":
      if (typeof value.viewId !== "string" || !IDENTIFIER.test(value.viewId)) return null;
      return { type: "view.open", viewId: value.viewId };
    default:
      return null;
  }
}
