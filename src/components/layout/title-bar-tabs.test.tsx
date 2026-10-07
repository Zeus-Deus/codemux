/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type {
  AgentChatSessionRecord,
} from "@/tauri/commands";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
const mocks = vi.hoisted(() => ({
  listSessions: vi.fn(),
  deleteSession: vi.fn().mockResolvedValue(undefined),
  activateTab: vi.fn().mockResolvedValue(undefined),
  closeTab: vi.fn().mockResolvedValue(undefined),
  reorderTabs: vi.fn().mockResolvedValue(undefined),
  renameTab: vi.fn().mockResolvedValue(undefined),
  splitPane: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/tauri/commands", () => ({
  agentChatListSessions: (...a: unknown[]) => mocks.listSessions(...a),
  agentChatDeleteSession: (...a: unknown[]) => mocks.deleteSession(...a),
  activateTab: (...a: unknown[]) => mocks.activateTab(...a),
  closeTab: (...a: unknown[]) => mocks.closeTab(...a),
  reorderTabs: (...a: unknown[]) => mocks.reorderTabs(...a),
  renameTab: (...a: unknown[]) => mocks.renameTab(...a),
  splitPane: (...a: unknown[]) => mocks.splitPane(...a),
  // Resume/new-chat SDK calls — unused in these tests but imported by the
  // shared session-actions hook.
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  agentChatListMessages: vi.fn().mockResolvedValue([]),
}));

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

import { useEditorStore } from "@/stores/editor-store";
import { TitleBarTabs } from "./title-bar-tabs";

function makeSession(
  overrides: Partial<AgentChatSessionRecord>,
): AgentChatSessionRecord {
  return {
    thread_id: "thread-x",
    sdk_session_id: "sdk-x",
    workspace_id: "ws-1",
    cwd: "/p",
    provider: "claude",
    title: null,
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    model: null,
    effort: null,
    context_window: null,
    permission_mode: null,
    ...overrides,
  };
}

function chatPane(threadId: string | null): PaneNodeSnapshot {
  return {
    kind: "agent_chat",
    pane_id: "pane-chat",
    title: "Agent Chat",
    thread_id: threadId,
    provider: "claude",
    cwd: "/p",
  };
}

function makeWorkspace(
  root: PaneNodeSnapshot,
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [
      {
        tab_id: "tab-chat",
        kind: "terminal",
        title: "Agent Chat",
        surface_id: "surface-1",
        browser_id: null,
        icon: null,
      },
    ],
    active_tab_id: "tab-chat",
    active_surface_id: "surface-1",
    surfaces: [
      { surface_id: "surface-1", title: "s", active_pane_id: root.pane_id, root },
    ],
    ...overrides,
  };
}

beforeEach(() => {
  mocks.listSessions.mockReset();
  mocks.activateTab.mockClear();
  mocks.closeTab.mockClear();
  mocks.reorderTabs.mockClear();
  mocks.renameTab.mockClear();
  mocks.splitPane.mockClear();
});

// Stubs a tab pill's on-screen box so `useTabReorder`'s midpoint math
// (which reads `getBoundingClientRect` on every `[data-tab-id]` element)
// resolves deterministically under jsdom, which otherwise reports 0 for
// every box — same technique as `use-sidebar-gap-width.test.ts`.
function stubRect(el: Element, left: number, width: number) {
  Object.defineProperty(el, "getBoundingClientRect", {
    value: () =>
      ({
        left,
        right: left + width,
        width,
        top: 0,
        bottom: 28,
        height: 28,
        x: left,
        y: 0,
        toJSON() {},
      }) as DOMRect,
    configurable: true,
  });
}

// Three plain terminal tabs (no chat pane involved) laid out left-to-right
// in 100px-wide slots, for exercising drag-to-reorder independent of the
// active-chat-tab dropdown affordance.
function makeThreeTabWorkspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [
      { tab_id: "tab-a", kind: "terminal", title: "term-a", surface_id: null, browser_id: null, icon: null },
      { tab_id: "tab-b", kind: "terminal", title: "term-b", surface_id: null, browser_id: null, icon: null },
      { tab_id: "tab-c", kind: "terminal", title: "term-c", surface_id: null, browser_id: null, icon: null },
    ],
    active_tab_id: "tab-a",
    active_surface_id: "surface-none",
    surfaces: [],
  };
}

describe("TitleBarTabs drag-to-reorder", () => {
  function renderThreeTabs() {
    const utils = render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    const pillFor = (title: string) =>
      screen.getByText(title).closest("[data-tab-id]") as HTMLElement;
    stubRect(pillFor("term-a"), 0, 100);
    stubRect(pillFor("term-b"), 100, 100);
    stubRect(pillFor("term-c"), 200, 100);
    return { ...utils, pillFor };
  }

  it("still activates on a plain click (down/up with no movement)", () => {
    const { pillFor } = renderThreeTabs();
    const pill = pillFor("term-b");
    fireEvent.pointerDown(pill, { pointerId: 1, button: 0, clientX: 150, clientY: 14 });
    fireEvent.pointerUp(document, { pointerId: 1, button: 0, clientX: 150, clientY: 14 });
    // Click lands on the inner activate button/label, same as a real
    // browser click — not the outer pill wrapper.
    fireEvent.click(screen.getByText("term-b"));
    expect(mocks.activateTab).toHaveBeenCalledWith("ws-1", "tab-b");
    expect(mocks.reorderTabs).not.toHaveBeenCalled();
  });

  it("calls reorderTabs with the new order once the drag threshold is crossed and dropped", () => {
    const { pillFor } = renderThreeTabs();
    const pill = pillFor("term-a");
    fireEvent.pointerDown(pill, { pointerId: 1, button: 0, clientX: 50, clientY: 14 });
    // Past the 5px threshold, and past term-c's midpoint (250) so the tab
    // drops at the end of the strip.
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });

    expect(mocks.reorderTabs).toHaveBeenCalledWith("ws-1", ["tab-b", "tab-c", "tab-a"]);
  });

  it("does not activate the tab when the drag actually moved it (drag suppresses the trailing click)", () => {
    const { pillFor } = renderThreeTabs();
    const pill = pillFor("term-b");
    fireEvent.pointerDown(pill, { pointerId: 1, button: 0, clientX: 150, clientY: 14 });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });
    // Browsers can still synthesize a trailing click after a released
    // drag, landing on the inner activate button; the component must
    // swallow it via onClickCapture on the pill wrapper.
    fireEvent.click(screen.getByText("term-b"));

    expect(mocks.activateTab).not.toHaveBeenCalled();
  });

  it("keeps the close button working and never starts a drag from it", () => {
    const { pillFor } = renderThreeTabs();
    const pill = pillFor("term-a");
    const closeBtn = pill.querySelector('[aria-label="Close tab"]') as HTMLElement;

    fireEvent.pointerDown(closeBtn, { pointerId: 1, button: 0, clientX: 5, clientY: 14 });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 260, clientY: 14 });
    fireEvent.click(closeBtn);

    // pointerdown on a `data-no-drag` control never starts a session, so
    // the move past threshold + drop is inert...
    expect(mocks.reorderTabs).not.toHaveBeenCalled();
    // ...and the close click still goes through untouched.
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "tab-a");
  });
});

afterEach(cleanup);

describe("TitleBarTabs", () => {
  it("renders the active chat tab with its history chevron trigger", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    expect(
      screen.getByTestId("titlebar-chat-tab-trigger"),
    ).toBeInTheDocument();
    expect(screen.getByText("Agent Chat")).toBeInTheDocument();
  });

  it("opens the history dropdown with grouped sessions + New Chat", async () => {
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockReturnValue(new Date("2026-04-24T15:00:00Z").getTime());
    try {
      mocks.listSessions.mockResolvedValue([
        makeSession({
          thread_id: "t-today",
          title: "Today chat",
          last_active_at: "2026-04-24 10:00:00",
        }),
        makeSession({
          thread_id: "t-yday",
          title: "Yesterday chat",
          last_active_at: "2026-04-23 20:00:00",
        }),
      ]);
      render(<TitleBarTabs workspace={makeWorkspace(chatPane("t-today"))} />);
      act(() => {
        const trigger = screen.getByTestId("titlebar-chat-tab-trigger");
        fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
        fireEvent.click(trigger);
      });

      await waitFor(() =>
        expect(screen.getByText("Today chat")).toBeInTheDocument(),
      );
      expect(screen.getByText("Today")).toBeInTheDocument();
      expect(screen.getByText("Yesterday")).toBeInTheDocument();
      expect(screen.getByText("Yesterday chat")).toBeInTheDocument();
      expect(
        screen.getByTestId("session-selector-new-chat"),
      ).toBeInTheDocument();
      expect(mocks.listSessions).toHaveBeenCalledWith("ws-1", "/p", 50);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("translates a vertical wheel delta into horizontal scroll once tabs overflow", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    const scroller = screen.getByTestId("titlebar-tabs-scroll");
    // jsdom reports 0 for both by default; stub an overflowing layout so
    // the guard (`scrollWidth <= clientWidth`) doesn't short-circuit —
    // mirrors the WebKit-webview quirk noted in the handler: a plain
    // vertical wheel doesn't natively scroll an `overflow-x: auto` box.
    Object.defineProperty(scroller, "scrollWidth", { value: 800, configurable: true });
    Object.defineProperty(scroller, "clientWidth", { value: 400, configurable: true });
    Object.defineProperty(scroller, "scrollLeft", { value: 0, writable: true, configurable: true });
    fireEvent.wheel(scroller, { deltaY: 120 });
    expect(scroller.scrollLeft).toBe(120);
  });

  it("scales a line-mode wheel delta and consumes the gesture", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    const scroller = screen.getByTestId("titlebar-tabs-scroll");
    Object.defineProperty(scroller, "scrollWidth", { value: 800, configurable: true });
    Object.defineProperty(scroller, "clientWidth", { value: 400, configurable: true });
    Object.defineProperty(scroller, "scrollLeft", { value: 0, writable: true, configurable: true });

    // A notched mouse reports deltas in LINES, not pixels — applied raw, one
    // notch would move the bar 3px. The event is also consumed so the
    // gesture doesn't scroll an ancestor at the same time.
    const lineWheel = new WheelEvent("wheel", {
      deltaY: 3,
      deltaMode: 1 /* DOM_DELTA_LINE */,
      bubbles: true,
      cancelable: true,
    });
    scroller.dispatchEvent(lineWheel);
    expect(scroller.scrollLeft).toBe(48);
    expect(lineWheel.defaultPrevented).toBe(true);
  });

  it("leaves the wheel gesture to the ancestor once the strip is at its end", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    const scroller = screen.getByTestId("titlebar-tabs-scroll");
    Object.defineProperty(scroller, "scrollWidth", { value: 800, configurable: true });
    Object.defineProperty(scroller, "clientWidth", { value: 400, configurable: true });
    Object.defineProperty(scroller, "scrollLeft", { value: 400, writable: true, configurable: true });

    const wheel = new WheelEvent("wheel", {
      deltaY: 120,
      bubbles: true,
      cancelable: true,
    });
    scroller.dispatchEvent(wheel);
    expect(scroller.scrollLeft).toBe(400);
    expect(wheel.defaultPrevented).toBe(false);
  });

  it("activates an inactive tab on click", () => {
    mocks.listSessions.mockResolvedValue([]);
    const ws = makeWorkspace(chatPane("thread-1"), {
      tabs: [
        {
          tab_id: "tab-chat",
          kind: "terminal",
          title: "Agent Chat",
          surface_id: "surface-1",
          browser_id: null,
          icon: null,
        },
        {
          tab_id: "tab-term",
          kind: "terminal",
          title: "zsh",
          surface_id: "surface-2",
          browser_id: null,
          icon: null,
        },
      ],
      surfaces: [
        {
          surface_id: "surface-1",
          title: "s",
          active_pane_id: "pane-chat",
          root: chatPane("thread-1"),
        },
        {
          surface_id: "surface-2",
          title: "t",
          active_pane_id: "pane-term",
          root: {
            kind: "terminal",
            pane_id: "pane-term",
            session_id: "sess-2",
            title: "zsh",
          },
        },
      ],
    });
    render(<TitleBarTabs workspace={ws} />);
    act(() => {
      fireEvent.click(screen.getByText("zsh"));
    });
    expect(mocks.activateTab).toHaveBeenCalledWith("ws-1", "tab-term");
  });
});

describe("TitleBarTabs tab actions", () => {
  const pillFor = (title: string) =>
    screen.getByText(title).closest("[data-tab-id]") as HTMLElement;

  it("closes a tab on middle-click", () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    const pill = pillFor("term-b");
    const down = new MouseEvent("mousedown", {
      button: 1,
      bubbles: true,
      cancelable: true,
    });
    pill.dispatchEvent(down);
    // Cancelled so the webview's middle-button autoscroll never engages.
    expect(down.defaultPrevented).toBe(true);
    fireEvent(pill, new MouseEvent("auxclick", { button: 1, bubbles: true }));
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "tab-b");
    expect(mocks.activateTab).not.toHaveBeenCalled();
  });

  it("offers the shared context menu and closes the other tabs from it", async () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.contextMenu(pillFor("term-b"));
    expect(await screen.findByText("Close tabs to the right")).toBeInTheDocument();
    expect(screen.getByText("Split right")).toBeInTheDocument();
    expect(screen.getByText("Rename tab")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Close other tabs"));
    await waitFor(() => expect(mocks.closeTab).toHaveBeenCalledTimes(2));
    expect(mocks.closeTab).toHaveBeenNthCalledWith(1, "ws-1", "tab-a");
    expect(mocks.closeTab).toHaveBeenNthCalledWith(2, "ws-1", "tab-c");
  });

  it("closes the tabs to the right, rightmost first", async () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.contextMenu(pillFor("term-a"));
    fireEvent.click(await screen.findByText("Close tabs to the right"));
    await waitFor(() => expect(mocks.closeTab).toHaveBeenCalledTimes(2));
    expect(mocks.closeTab).toHaveBeenNthCalledWith(1, "ws-1", "tab-c");
    expect(mocks.closeTab).toHaveBeenNthCalledWith(2, "ws-1", "tab-b");
  });

  it("renames inline from the context menu and commits on Enter", async () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.contextMenu(pillFor("term-b"));
    fireEvent.click(await screen.findByText("Rename tab"));

    const input = await screen.findByLabelText("Tab name");
    expect(input).toHaveValue("term-b");
    await waitFor(() => expect(input).toHaveFocus());
    fireEvent.change(input, { target: { value: "  server  " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(mocks.renameTab).toHaveBeenCalledTimes(1);
    expect(mocks.renameTab).toHaveBeenCalledWith("ws-1", "tab-b", "server");
    expect(screen.queryByLabelText("Tab name")).not.toBeInTheDocument();
  });

  it("renames on double-click and cancels on Escape", () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.doubleClick(screen.getByText("term-a"));

    const input = screen.getByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "discarded" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(mocks.renameTab).not.toHaveBeenCalled();
    expect(screen.getByText("term-a")).toBeInTheDocument();
  });

  it("marks an editor tab with unsaved changes", () => {
    const ws = makeThreeTabWorkspace();
    ws.tabs[1] = { ...ws.tabs[1], kind: "editor", title: "main.rs" };
    render(<TitleBarTabs workspace={ws} />);
    expect(screen.queryByLabelText("Unsaved changes")).not.toBeInTheDocument();

    act(() => useEditorStore.getState().setDirty("tab-b", true));
    expect(screen.getByLabelText("Unsaved changes")).toBeInTheDocument();
    act(() => useEditorStore.getState().setDirty("tab-b", false));
  });

  it("gives the active chat tab the same context menu", async () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    fireEvent.contextMenu(pillFor("Agent Chat"));
    fireEvent.click(await screen.findByText("Rename tab"));
    const input = await screen.findByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "Review bot" } });
    fireEvent.blur(input);
    expect(mocks.renameTab).toHaveBeenCalledWith("ws-1", "tab-chat", "Review bot");
  });

  it("returns focus to the tab's label after a keyboard rename", () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.doubleClick(screen.getByText("term-b"));
    fireEvent.keyDown(screen.getByLabelText("Tab name"), { key: "Escape" });
    expect(screen.getByTitle("term-b")).toHaveFocus();
  });

  it("leaves Enter and Escape to an IME composition", () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.doubleClick(screen.getByText("term-a"));
    const input = screen.getByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "にほ" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });
    expect(mocks.renameTab).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Tab name")).toBeInTheDocument();
  });

  it("ignores a whitespace-only rename", () => {
    render(<TitleBarTabs workspace={makeThreeTabWorkspace()} />);
    fireEvent.doubleClick(screen.getByText("term-a"));
    const input = screen.getByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(mocks.renameTab).not.toHaveBeenCalled();
    expect(screen.getByText("term-a")).toBeInTheDocument();
  });

  it("splits the active pane and disables the close items for a lone tab", async () => {
    const root: PaneNodeSnapshot = {
      kind: "terminal",
      pane_id: "pane-term",
      session_id: "sess-1",
      title: "zsh",
    };
    render(<TitleBarTabs workspace={makeWorkspace(root)} />);
    const pill = pillFor("Agent Chat");

    fireEvent.contextMenu(pill);
    expect(
      (await screen.findByText("Close other tabs")).closest("[role=menuitem]"),
    ).toHaveAttribute("data-disabled");
    expect(
      screen.getByText("Close tabs to the right").closest("[role=menuitem]"),
    ).toHaveAttribute("data-disabled");
    fireEvent.click(screen.getByText("Split right"));
    expect(mocks.splitPane).toHaveBeenLastCalledWith("pane-term", "horizontal");

    fireEvent.contextMenu(pill);
    fireEvent.click(await screen.findByText("Split down"));
    expect(mocks.splitPane).toHaveBeenLastCalledWith("pane-term", "vertical");
  });
});
