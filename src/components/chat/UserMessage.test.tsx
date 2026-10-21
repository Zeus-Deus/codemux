import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import type { UserMessageItem } from "@/lib/agent-chat/types";

// The IPC read fallback the hydrated fs-path images lean on.
const readChatImageMock = vi.fn();
const getSessionMock = vi.fn();
const openChatMock = vi.fn();
vi.mock("./delegation-actions", () => ({openDelegatedChat:(...args:unknown[])=>openChatMock(...args)}));
vi.mock("@/tauri/commands", () => ({
  readChatImage: (...args: unknown[]) => readChatImageMock(...args),
  agentChatGetSession: (...args: unknown[]) => getSessionMock(...args),
}));

// Force `resolveAssetSrc` to hand an absolute fs path straight to the
// `<img>` (as a webview / dev mock that can't reach the asset protocol
// would), so the load errors and exercises the IPC-read fallback. The
// real `isAbsoluteFsPath` is kept so the fallback's path guard is tested.
vi.mock("@/lib/asset-url", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/asset-url")>();
  return { ...actual, resolveAssetSrc: (src: string | undefined) => src };
});

import { UserMessage } from "./UserMessage";
import { applyEvent, createEmptyThreadState } from "@/lib/agent-chat/reducer";

afterEach(cleanup);

function makeItem(images: UserMessageItem["images"]): UserMessageItem {
  return { kind: "user_message", id: "u1", seq: 0, text: "hi", images };
}

describe("UserMessage image fallback", () => {
  beforeEach(() => {
    readChatImageMock.mockReset();
    // jsdom lacks createObjectURL — stub it so the blob path resolves.
    (
      globalThis.URL as unknown as { createObjectURL: unknown }
    ).createObjectURL = vi.fn(() => "blob:mock-url");
  });

  it("reads the bytes over IPC and swaps in a blob URL when an fs-path image fails to load", async () => {
    readChatImageMock.mockResolvedValue({
      bytes: new Uint8Array([1, 2, 3]),
      media_type: "image/png",
    });
    // Unique path per test so the module-level blob cache doesn't bleed.
    const path = "/abs/hydrated-a.png";
    const { container } = render(
      <UserMessage item={makeItem([{ src: path, mediaType: "image/png" }])} />,
    );

    const img = container.querySelector("img") as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("src")).toBe(path);

    // Simulate the asset-protocol load failing.
    fireEvent.error(img);

    await waitFor(() => {
      expect(readChatImageMock).toHaveBeenCalledWith(path);
    });
    await waitFor(() => {
      const swapped = container.querySelector("img") as HTMLImageElement;
      expect(swapped.getAttribute("src")).toBe("blob:mock-url");
    });
  });

  it("shows the broken-image placeholder when the IPC read also fails", async () => {
    readChatImageMock.mockRejectedValue(new Error("gone"));
    const path = "/abs/hydrated-b.png";
    const { container } = render(
      <UserMessage item={makeItem([{ src: path, mediaType: "image/png" }])} />,
    );

    const img = container.querySelector("img") as HTMLImageElement;
    fireEvent.error(img);

    await waitFor(() => {
      expect(readChatImageMock).toHaveBeenCalledWith(path);
    });
    await waitFor(() => {
      expect(
        container.querySelector('[aria-label="Image failed to load"]'),
      ).not.toBeNull();
    });
  });

  it("never reaches the IPC fallback for a data: URL (passes through)", () => {
    const dataUrl = "data:image/png;base64,AQIDBA==";
    const { container } = render(
      <UserMessage
        item={makeItem([{ src: dataUrl, mediaType: "image/png" }])}
      />,
    );
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img.getAttribute("src")).toBe(dataUrl);
    // No error fired → the read fallback is untouched.
    expect(readChatImageMock).not.toHaveBeenCalled();
  });
});

describe("UserMessage turn revert", () => {
  it("renders a checkpoint-bound action and delegates confirmation upstream", () => {
    const onRevert = vi.fn();
    const { getByRole } = render(
      <UserMessage item={makeItem([])} onRevert={onRevert} />,
    );

    fireEvent.click(
      getByRole("button", { name: "Revert to before this turn" }),
    );

    expect(onRevert).toHaveBeenCalledTimes(1);
  });

  it("does not offer revert on a queued turn", () => {
    const item = {
      ...makeItem([]),
      queued: { queuedId: "queued-1" },
    };
    const { queryByRole } = render(
      <UserMessage item={item} onRevert={() => undefined} />,
    );

    expect(
      queryByRole("button", { name: "Revert to before this turn" }),
    ).toBeNull();
  });
});

describe("queued message delivery actions", () => {
  const queued: UserMessageItem = { kind: "user_message", id: "queued", seq: 1, text: "Use SQLite", queued: { queuedId: "q1" } };
  it("keeps steering separate from interruption", () => {
    const steer = vi.fn();
    const interrupt = vi.fn();
    const { getByRole } = render(<UserMessage item={queued} onSteerQueued={steer} onSendQueuedNow={interrupt} />);
    fireEvent.click(getByRole("button", { name: "Steer with queued message" }));
    expect(steer).toHaveBeenCalledWith("q1");
    expect(interrupt).not.toHaveBeenCalled();
  });
  it("does not offer unsupported steering", () => {
    const { queryByRole, getByRole } = render(<UserMessage item={queued} onSendQueuedNow={vi.fn()} />);
    expect(queryByRole("button", { name: "Steer with queued message" })).toBeNull();
    expect(getByRole("button", { name: "Interrupt and send queued message" })).toBeTruthy();
  });
});


describe("UserMessage provenance", () => {
  it("opens the sending chat through the attribution action", async () => {
    getSessionMock.mockResolvedValue({thread_id:"sender",title:"Review agent"});
    const {getByRole,container}=render(<UserMessage item={{...makeItem([]),created_by:"agent",creation_source:"provider",sender_thread_id:"sender",sender_title:"Review agent"}} />);
    const button=await waitFor(()=>getByRole("button",{name:"Open sending chat"}));
    fireEvent.click(button);
    expect(openChatMock).toHaveBeenCalledWith("sender");
    expect(container.querySelector('[data-user-message-attribution="agent"]')?.textContent).toBe("Sent by another agent");
  });
  it("keeps the title snapshot as plain text for a deleted sender", async () => {
    getSessionMock.mockResolvedValue(null);
    const {queryByRole,getByText}=render(<UserMessage item={{...makeItem([]),created_by:"agent",sender_thread_id:"deleted",sender_title:"Previous reviewer"}} />);
    await waitFor(()=>expect(getSessionMock).toHaveBeenCalledWith("deleted"));
    expect(getByText("Sent by another agent · Previous reviewer")).toBeTruthy();
    expect(queryByRole("button",{name:"Open sending chat"})).toBeNull();
  });
  it("attributes app-authored input to Codemux", () => {
    const {getByText}=render(<UserMessage item={{...makeItem([]),created_by:"system",creation_source:"codemux"}} />);
    expect(getByText("Sent by Codemux")).toBeTruthy();
  });
  it.each([ ["queued_turn","Queued","Queued behind the active turn"], ["steer","Steer","Steered the active turn"], ["promoted_queued_to_steer","Steer","Originally queued, then promoted to steer the active turn"] ] as const)("renders persisted %s intent after completion", (input_intent,label,title) => {
    const {getByText}=render(<UserMessage item={{...makeItem([]),input_intent,inflight:false}} />);
    expect(getByText(label).getAttribute("title")).toBe(title);
  });
  it.each(["turn_start", "queued_turn"] as const)("renders a dispatched scheduler message with %s intent", (input_intent) => {
    let state = applyEvent(createEmptyThreadState(), {
      type: "turn_queued", thread_id: "thread", queued_id: "queue", client_nonce: null, text: "Hello", input_intent,
    });
    state = applyEvent(state, {
      type: "queued_turn_dispatched", thread_id: "thread", queued_id: "queue", turn_id: "turn", text: "Hello",
    });
    const {queryByText} = render(<UserMessage item={state.messages[0] as UserMessageItem} />);
    expect(queryByText("Queued") !== null).toBe(input_intent === "queued_turn");
  });
  it("does not guess intent from a legacy inflight flag", () => {
    const {queryByText}=render(<UserMessage item={{...makeItem([]),inflight:true}} />);
    expect(queryByText("Steer")).toBeNull();
    expect(queryByText("Steered")).toBeNull();
  });
});
