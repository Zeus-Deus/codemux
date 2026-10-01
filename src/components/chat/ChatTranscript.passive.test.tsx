import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { readLocalChatImage } from "@/tauri/commands";
import { ChatTranscript } from "./ChatTranscript";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext } from "./transcript-cache-binding";

// jsdom has no layout; substitute only the virtualizer, not the rendering chain.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return { LegendList: React.forwardRef(function List(props: Record<string, any>, ref) {
    const node = React.useRef<HTMLDivElement>(null);
    React.useImperativeHandle(ref, () => ({
      getScrollableNode: () => node.current,
      getState: () => ({ scroll: 0, scrollLength: 500, isAtEnd: true, listen: () => () => {} }),
      scrollToEnd: () => Promise.resolve(), scrollToIndex: () => Promise.resolve(),
      scrollToOffset: () => Promise.resolve(),
    }));
    return <div ref={node}>{props.ListHeaderComponent}{props.data.map((item: unknown, index: number) =>
      <React.Fragment key={props.keyExtractor(item, index)}>{props.renderItem({ item, index })}</React.Fragment>
    )}{props.ListFooterComponent}</div>;
  }) };
});
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/tauri/commands")>(),
  readLocalChatImage: vi.fn(),
}));
vi.mock("@/hooks/use-chat-code-plugin", () => ({ useChatCodePlugin: () => undefined }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const prose = "[External reference](https://private.example.test/page)\n\n![Remote image](https://private.example.test/secret.png)\n\n![Local image](/project/private.png)\n\n[Local screenshot](/project/private.png)\n\n`/project/private.png`";
const messages = [
  { kind: "user_message", id: "user", seq: 1, text: prose, turn_id: "t" },
  { kind: "assistant_message", id: "assistant", seq: 2, text: prose, streaming: false, turn_id: "t" },
] as ChatViewItem[];
function tree(passive: boolean) {
  const binding = { key: "snapshot", workspaceId: "ws", threadKey: "snapshot", provider: "claude" as const, cwd: "/project" };
  return <TooltipProvider><TranscriptCacheProvider activeKey="snapshot" validKeys={["snapshot"]}>
    <TranscriptBindingContext.Provider value={binding}>
      <ChatTranscript messages={messages} streaming={false} threadKey="snapshot" workspaceId="ws" cwd="/project"
        provider="claude" passive={passive} onRespondToRequest={vi.fn()} onAcceptPlan={vi.fn()} onRejectPlan={vi.fn()} />
    </TranscriptBindingContext.Provider>
  </TranscriptCacheProvider></TooltipProvider>;
}

describe("passive snapshot prose through the real cached rendering chain", () => {
  it("renders assistant and user references without favicon, remote image or local preview requests", async () => {
    const view = render(tree(true));
    expect(view.container.querySelector('[data-message-id="user"]')?.textContent).toContain("![Remote image](https://private.example.test/secret.png)");
    expect(view.getByRole("link", { name: /xternal reference/ })).toBeInTheDocument();
    const proseRoot = view.container.querySelector(".chat-markdown")!;
    expect(proseRoot).not.toBeNull();
    expect(proseRoot.querySelectorAll("img, iframe, video, audio, source")).toHaveLength(0);
    expect(view.getByText("Remote image")).toBeInTheDocument();
    expect(view.getByText("Local image")).toBeInTheDocument();
    expect(readLocalChatImage).not.toHaveBeenCalled();
    fireEvent.click(view.getByRole("link", { name: /xternal reference/ }));
    expect(view.getByRole("alertdialog")).toBeInTheDocument();
  });
  it("retains automatic rich resources for ordinary live prose", async () => {
    const view = render(tree(false));
    await waitFor(() => expect(view.container.querySelector('.chat-markdown img[src*="google.com/s2/favicons"]')).not.toBeNull());
    expect(view.container.querySelector('.chat-markdown img[src="https://private.example.test/secret.png"]')).not.toBeNull();
    expect(view.container.querySelector('[data-chat-local-image]')).not.toBeNull();
  });
});
