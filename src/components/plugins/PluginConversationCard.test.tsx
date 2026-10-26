import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { PluginCardReference } from "./contract";
import { installation, manifest } from "./test-fixtures";

const requests: { method: string; params: Record<string, unknown> }[] = [];
let readCardView: () => Promise<unknown> = async () => ({ html: "<p>card</p>" });
const readViews = new Map<string, Promise<unknown>>();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (_command: string, args: { method: string; params: Record<string, unknown> }) => {
    requests.push(args);
    if (args.method === "readCardView") return readCardView();
    if (args.method === "readView") return readViews.get(String(args.params.generation));
    return undefined;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/components/chat/HtmlRenderFrame", () => ({
  useHtmlRenderTheme: () => ({ scheme: "dark", variables: { "--background": "#000" } }),
}));

import { resetPlatformPluginsForTests } from "./platform-adapter";
import { PluginConversationCard } from "./PluginConversationCard";
import { PluginViewFrame } from "./PluginViewFrame";

const card: PluginCardReference = {
  viewId: "card",
  summary: "Deployed build 42",
  installationId: "inst-1",
  generation: "gen-1",
  dataOwner: "owner-1",
};

const cardPlugin = installation({
  manifest: manifest({ views: [{ id: "card", title: "Deploy", entry: "card.html", placement: "conversation" }] }),
  grants: { capabilities: ["ui", "ui.conversation-cards"], projectIds: [] },
});

beforeEach(() => {
  requests.length = 0;
  readCardView = async () => ({ html: "<p>card</p>" });
});
afterEach(() => cleanup());

describe("PluginConversationCard", () => {
  it("keeps the recorded summary readable when the plugin is gone", () => {
    resetPlatformPluginsForTests({ catalogue: { installations: [] }, loaded: true });
    render(<PluginConversationCard card={card} workspaceId="ws-1" threadId="t-1" />);
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
    expect(screen.getByText("This card's plugin is not installed here.")).toBeTruthy();
    expect(requests).toEqual([]);
  });

  it("keeps the summary on a host without the platform", () => {
    resetPlatformPluginsForTests({ catalogue: null, loaded: true, error: "unknown command" });
    render(<PluginConversationCard card={card} workspaceId="ws-1" threadId="t-1" />);
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
  });

  it("mounts the live card in a scripts-only sandbox, asking the host to check the card", async () => {
    resetPlatformPluginsForTests({ catalogue: { installations: [cardPlugin] }, loaded: true });
    const { container } = render(<PluginConversationCard card={card} workspaceId="ws-1" threadId="t-1" />);
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
    await waitFor(() => expect(container.querySelector("iframe")).not.toBeNull());
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
    const frame = container.querySelector("iframe")!;
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("srcdoc")).toContain("Content-Security-Policy");
    expect(requests).toEqual([
      { method: "readCardView", params: { installationId: "inst-1", generation: "gen-1", card } },
    ]);
  });

  it("falls back to the summary with a retry when the host refuses the card", async () => {
    readCardView = async () => {
      throw "permission_denied: This card belongs to another plugin.";
    };
    resetPlatformPluginsForTests({ catalogue: { installations: [cardPlugin] }, loaded: true });
    render(<PluginConversationCard card={card} workspaceId="ws-1" threadId="t-1" />);
    expect(await screen.findByText("This card belongs to another plugin.")).toBeTruthy();
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Try again/ })).toBeTruthy();
  });

  it("keeps the recorded summary when plugin data can't even be read, instead of losing the transcript", () => {
    const broken = { ...cardPlugin };
    Object.defineProperty(broken, "manifest", {
      get() {
        throw new TypeError("manifest from a newer host");
      },
    });
    resetPlatformPluginsForTests({ catalogue: { installations: [broken] }, loaded: true });
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <div>
        <p>Earlier reply</p>
        <PluginConversationCard card={card} workspaceId="ws-1" threadId="t-1" />
      </div>,
    );
    expect(screen.getByText("Earlier reply")).toBeTruthy();
    expect(screen.getByText("Deployed build 42")).toBeTruthy();
    expect(screen.getByText("This card couldn't be shown.")).toBeTruthy();
    error.mockRestore();
  });
});

describe("PluginViewFrame", () => {
  it("never shows or binds a document read for an earlier generation", async () => {
    const page = { id: "home", title: "Home", entry: "home.html", placement: "page" as const };
    const first = installation({ manifest: manifest({ views: [page] }) });
    resetPlatformPluginsForTests({ catalogue: { installations: [first] }, loaded: true });
    readViews.set("gen-1", Promise.resolve({ html: "<p>old generation</p>" }));
    let serveSecond: (value: unknown) => void = () => {};
    readViews.set("gen-2", new Promise((resolve) => (serveSecond = resolve)));
    const props = { view: page, placement: "page" as const, workspaceId: null, threadId: null };
    // Deliberately unkeyed: the frame itself must not carry the old document across.
    const { container, rerender } = render(<PluginViewFrame installation={first} {...props} />);
    await waitFor(() => expect(container.querySelector("iframe")?.getAttribute("srcdoc")).toContain("old generation"));
    rerender(<PluginViewFrame installation={{ ...first, generation: "gen-2" }} {...props} />);
    expect(container.querySelector("iframe")).toBeNull();
    serveSecond({ html: "<p>new generation</p>" });
    await waitFor(() => expect(container.querySelector("iframe")?.getAttribute("srcdoc")).toContain("new generation"));
    expect(container.querySelector("iframe")?.getAttribute("srcdoc")).not.toContain("old generation");
  });
});
