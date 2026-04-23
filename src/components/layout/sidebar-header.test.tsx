/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";

// shadcn SidebarProvider uses useIsMobile → window.matchMedia, which
// jsdom doesn't implement. Stub a minimal shape so the effect runs.
if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

const setShowDialogMock = vi.fn();
let enableAgentChatFlag = false;
let enableLazyFlag = false;

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      showNewWorkspaceDialog: false,
      setShowNewWorkspaceDialog: setShowDialogMock,
    };
    return selector(state);
  }),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) => {
    const state = {
      enableAgentChat: enableAgentChatFlag,
      enableLazyWorkspaceCreation: enableLazyFlag,
      loaded: true,
    };
    return selector(state);
  }),
}));

import { SidebarHeader } from "./sidebar-header";
import { useChatDraftStore } from "@/stores/chat-draft-store";

function renderHeader() {
  const utils = render(
    <TooltipProvider>
      <SidebarProvider>
        <SidebarHeader />
      </SidebarProvider>
    </TooltipProvider>,
  );
  const plus = utils.container.querySelector(
    'button[aria-label="New workspace"]',
  ) as HTMLElement;
  return { ...utils, plus };
}

describe("SidebarHeader + button", () => {
  beforeEach(() => {
    setShowDialogMock.mockClear();
    enableAgentChatFlag = false;
    enableLazyFlag = false;
    useChatDraftStore.setState({
      draftsById: {},
      activeHomeDraftId: null,
      projectDraftIdByPath: {},
      activeDraftId: null,
    });
  });

  it("agent_chat OFF + plain click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
  });

  it("agent_chat OFF + Shift+click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
  });

  it("agent_chat ON + lazy OFF + plain click → opens NewWorkspaceDialog (Home singleton retired)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    // Draft path must not fire when lazy flag is off.
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("agent_chat ON + Shift+click → opens NewWorkspaceDialog (regardless of lazy flag)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  describe("lazy workspace creation", () => {
    it("lazy ON + plain click → creates home draft, sets it active, no dialog", () => {
      enableAgentChatFlag = true;
      enableLazyFlag = true;
      const { plus } = renderHeader();
      fireEvent.click(plus);

      const state = useChatDraftStore.getState();
      expect(state.activeHomeDraftId).not.toBeNull();
      expect(state.activeDraftId).toBe(state.activeHomeDraftId);
      const draft = state.draftsById[state.activeHomeDraftId!];
      expect(draft.target).toEqual({ kind: "home" });
      expect(setShowDialogMock).not.toHaveBeenCalled();
    });

    it("lazy ON + plain click reuses the existing home draft on a second click", () => {
      enableAgentChatFlag = true;
      enableLazyFlag = true;
      const { plus } = renderHeader();
      fireEvent.click(plus);
      const firstId = useChatDraftStore.getState().activeHomeDraftId;
      fireEvent.click(plus);
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(firstId);
      expect(
        Object.keys(useChatDraftStore.getState().draftsById),
      ).toHaveLength(1);
    });
  });
});
