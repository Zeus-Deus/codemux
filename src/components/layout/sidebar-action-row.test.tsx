/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
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
const setShowNewProjectScreenMock = vi.fn();
const setShowCommandPaletteMock = vi.fn();
const openProjectMock = vi.fn();
let enableAgentChatFlag = false;
let enableLazyFlag = false;

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      showNewWorkspaceDialog: false,
      setShowNewWorkspaceDialog: setShowDialogMock,
      setShowNewProjectScreen: setShowNewProjectScreenMock,
      setShowCommandPalette: setShowCommandPaletteMock,
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

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({
    openProject: openProjectMock,
  }),
}));

import { SidebarActionRow } from "./sidebar-action-row";
import { useChatDraftStore } from "@/stores/chat-draft-store";

function renderRow() {
  const utils = render(
    <TooltipProvider>
      <SidebarProvider>
        <SidebarActionRow />
      </SidebarProvider>
    </TooltipProvider>,
  );
  const newAgent = utils.container.querySelector(
    'button[aria-label="New agent"]',
  ) as HTMLElement;
  const search = utils.container.querySelector(
    'button[aria-label="Search"]',
  ) as HTMLElement;
  return { ...utils, newAgent, search };
}

// SidebarProvider defaults to expanded; `defaultOpen={false}` renders the
// collapsed icon-rail variant of the header.
function renderCollapsedRow() {
  const utils = render(
    <TooltipProvider>
      <SidebarProvider defaultOpen={false}>
        <SidebarActionRow />
      </SidebarProvider>
    </TooltipProvider>,
  );
  const newAgent = utils.container.querySelector(
    'button[aria-label="New agent"]',
  ) as HTMLElement;
  const search = utils.container.querySelector(
    'button[aria-label="Search"]',
  ) as HTMLElement;
  return { ...utils, newAgent, search };
}

describe("SidebarActionRow — New agent button", () => {
  beforeEach(() => {
    setShowDialogMock.mockClear();
    setShowNewProjectScreenMock.mockClear();
    openProjectMock.mockClear();
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
    const { newAgent } = renderRow();
    fireEvent.click(newAgent);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
  });

  it("agent_chat OFF + Shift+click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { newAgent } = renderRow();
    fireEvent.click(newAgent, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
  });

  it("agent_chat ON + lazy OFF + plain click → opens NewWorkspaceDialog (Home singleton retired)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = false;
    const { newAgent } = renderRow();
    fireEvent.click(newAgent);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("agent_chat ON + Shift+click → opens NewWorkspaceDialog (regardless of lazy flag)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    const { newAgent } = renderRow();
    fireEvent.click(newAgent, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  describe("lazy workspace creation", () => {
    it("lazy ON + plain click → creates home draft, sets it active, no dialog", () => {
      enableAgentChatFlag = true;
      enableLazyFlag = true;
      const { newAgent } = renderRow();
      fireEvent.click(newAgent);

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
      const { newAgent } = renderRow();
      fireEvent.click(newAgent);
      const firstId = useChatDraftStore.getState().activeHomeDraftId;
      fireEvent.click(newAgent);
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(firstId);
      expect(
        Object.keys(useChatDraftStore.getState().draftsById),
      ).toHaveLength(1);
    });
  });
});

describe("SidebarActionRow — Search affordance", () => {
  beforeEach(() => {
    setShowCommandPaletteMock.mockClear();
  });

  it("renders a search button that opens the command palette", () => {
    const { search } = renderRow();
    expect(search).not.toBeNull();
    fireEvent.click(search);
    expect(setShowCommandPaletteMock).toHaveBeenCalledWith(true);
  });
});

describe("SidebarActionRow — collapsed icon rail", () => {
  beforeEach(() => {
    setShowDialogMock.mockClear();
    setShowCommandPaletteMock.mockClear();
    enableAgentChatFlag = false;
    enableLazyFlag = false;
    cleanup();
  });

  it("shows only New agent + Search, centered", () => {
    const { newAgent, search } = renderCollapsedRow();
    expect(newAgent).not.toBeNull();
    expect(search).not.toBeNull();
  });

  it("Search opens the command palette", () => {
    const { search } = renderCollapsedRow();
    fireEvent.click(search);
    expect(setShowCommandPaletteMock).toHaveBeenCalledWith(true);
  });

  it("New agent still creates a workspace (agent_chat OFF → dialog)", () => {
    const { newAgent } = renderCollapsedRow();
    fireEvent.click(newAgent);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
  });

  it("drops the retired Add repository / Automations / Workspaces buttons", () => {
    const { container } = renderCollapsedRow();
    expect(
      container.querySelector('button[aria-label="Add repository"]'),
    ).toBeNull();
    expect(
      container.querySelector('button[aria-label="Automations"]'),
    ).toBeNull();
    expect(
      container.querySelector('button[aria-label="Workspaces"]'),
    ).toBeNull();
  });
});

describe("SidebarActionRow — floating-titlebar clearance", () => {
  beforeEach(() => {
    cleanup();
    enableAgentChatFlag = false;
    enableLazyFlag = false;
    useChatDraftStore.setState({
      draftsById: {},
      activeHomeDraftId: null,
      projectDraftIdByPath: {},
      activeDraftId: null,
    });
  });

  // Regression: the clearance shipped ungated, so with the GUI flag off —
  // where `TitleBar` renders the in-flow `h-9` bar and nothing floats over
  // the sidebar — the search row sat under ~32px of dead padding.
  it("keeps the legacy inset while the title bar renders in normal flow", () => {
    const { getByTestId } = renderRow();
    const row = getByTestId("sidebar-action-row-expanded");
    expect(row).toHaveClass("pt-3");
    expect(row).not.toHaveClass("pt-11");
  });

  it("keeps the collapsed rail's legacy inset with legacy chrome", () => {
    const { getByTestId } = renderCollapsedRow();
    const row = getByTestId("sidebar-action-row-collapsed");
    expect(row).toHaveClass("pt-2");
    expect(row).not.toHaveClass("pt-11");
  });

  it("reserves the collision zone once the floating titlebar overlays the sidebar", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    const draft = useChatDraftStore
      .getState()
      .getOrCreateHomeDraft({ lockedToHome: true });
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const { getByTestId } = renderRow();
    expect(getByTestId("sidebar-action-row-expanded")).toHaveClass("pt-11");
  });

  it("reserves the collision zone on the collapsed rail too", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    const draft = useChatDraftStore
      .getState()
      .getOrCreateHomeDraft({ lockedToHome: true });
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const { getByTestId } = renderCollapsedRow();
    expect(getByTestId("sidebar-action-row-collapsed")).toHaveClass("pt-11");
  });
});
