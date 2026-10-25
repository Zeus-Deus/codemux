import { describe, expect, it } from "vitest";
import { decodePluginIntent, planPluginIntents, type PluginInvocationOrigin } from "./intents";

const origin: PluginInvocationOrigin = {
  installationId: "inst-1",
  locationKey: "|ws-1|pane-1",
  composerKey: "ws-1\0thread-1",
  threadId: "thread-1",
};
const views = [
  { id: "panel", title: "Panel", entry: "panel.html", placement: "side-panel" as const },
  { id: "home", title: "Home", entry: "home.html", placement: "page" as const },
  { id: "card", title: "Card", entry: "card.html", placement: "conversation" as const },
];

describe("planPluginIntents", () => {
  it("applies one navigation while the user stayed, and offers the rest", () => {
    const steps = planPluginIntents({
      origin,
      manifest: { views },
      intents: [
        { type: "view.open", viewId: "panel" },
        { type: "thread.reveal", threadId: "thread-2" },
      ],
      currentLocationKey: origin.locationKey,
      canOpenPreview: false,
    });
    expect(steps.map((step) => (step.kind === "navigate" ? step.mode : step.kind))).toEqual(["apply", "offer"]);
  });

  it("only offers once the user moved, and says where an insert went", () => {
    const steps = planPluginIntents({
      origin,
      manifest: { views },
      intents: [
        { type: "composer.insert", text: "a\n\nb", quote: true },
        { type: "view.open", viewId: "home" },
      ],
      currentLocationKey: "|ws-2|pane-9",
      canOpenPreview: false,
    });
    expect(steps[0]).toEqual({ kind: "insert", composerKey: origin.composerKey, text: "> a\n>\n> b", visible: false });
    expect(steps[1]).toMatchObject({ kind: "navigate", mode: "offer" });
  });

  it("hides an insert when an applied navigation leaves the chat", () => {
    const steps = planPluginIntents({
      origin,
      manifest: { views },
      intents: [
        { type: "composer.insert", text: "hello" },
        { type: "view.open", viewId: "home" },
      ],
      currentLocationKey: origin.locationKey,
      canOpenPreview: false,
    });
    expect(steps[0]).toMatchObject({ kind: "insert", visible: false });
    expect(steps[1]).toMatchObject({ kind: "navigate", mode: "apply" });
  });

  it("rejects inserts without a composer, unknown views and conversation views", () => {
    const steps = planPluginIntents({
      origin: { ...origin, composerKey: null },
      manifest: { views },
      intents: [
        { type: "composer.insert", text: "x" },
        { type: "view.open", viewId: "missing" },
        { type: "view.open", viewId: "card" },
      ],
      currentLocationKey: origin.locationKey,
      canOpenPreview: false,
    });
    expect(steps.map((step) => step.kind)).toEqual(["rejected", "rejected", "rejected"]);
  });

  it("offers links when no in-app preview can open them", () => {
    const [step] = planPluginIntents({
      origin,
      manifest: { views },
      intents: [{ type: "preview.open", url: "https://example.com" }],
      currentLocationKey: origin.locationKey,
      canOpenPreview: false,
    });
    expect(step).toMatchObject({ kind: "navigate", mode: "offer" });
  });
});

describe("decodePluginIntent", () => {
  it("accepts known shapes and drops extra fields", () => {
    expect(decodePluginIntent({ type: "view.open", viewId: "home", extra: 1 })).toEqual({ type: "view.open", viewId: "home" });
    expect(decodePluginIntent({ type: "preview.open", url: "https://x.dev" })).toEqual({ type: "preview.open", url: "https://x.dev" });
  });

  it("refuses unknown types, bad urls and bad ids", () => {
    expect(decodePluginIntent({ type: "shell.exec", command: "rm" })).toBeNull();
    expect(decodePluginIntent({ type: "preview.open", url: "javascript:alert(1)" })).toBeNull();
    expect(decodePluginIntent({ type: "view.open", viewId: "../x" })).toBeNull();
    expect(decodePluginIntent("composer.insert")).toBeNull();
  });
});
