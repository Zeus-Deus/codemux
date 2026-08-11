import {
  act,
  cleanup,
  render,
  screen,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  SidebarDraftBlock,
  SidebarRailDrafts,
  type SidebarDraftCatalog,
} from "./sidebar-draft-block";

const catalog: SidebarDraftCatalog = {
  homeDir: "/home/test",
  nameByProjectPath: new Map([
    ["/repo", "codemux"],
    ["/other", "other"],
  ]),
  projectByWorkspaceId: new Map(),
};

function resetStores() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  useAgentChatStore.setState({ threads: {} });
}

describe("SidebarDraftBlock", () => {
  beforeEach(() => {
    resetStores();
  });
  afterEach(cleanup);

  it("appears only after leaving a fresh invested draft and restores it", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);
    render(<SidebarDraftBlock catalog={catalog} filterPath={null} />);

    act(() => {
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "Investigate the flaky build");
    });
    expect(screen.queryByTestId("sidebar-draft-row")).not.toBeInTheDocument();

    act(() => {
      useChatDraftStore.getState().setActiveDraft(null);
    });
    const row = screen.getByTestId("sidebar-draft-row");
    expect(row).toHaveTextContent("codemux");
    expect(row).toHaveTextContent("Investigate the flaky build");

    await userEvent.click(row);
    expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
    expect(row).toHaveAttribute("data-active", "true");
  });

  it("freezes the active preview while preserving live composer edits", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore
      .getState()
      .updateDraftInput(draft.draftId, "Original preview");
    render(<SidebarDraftBlock catalog={catalog} filterPath={null} />);

    const row = screen.getByTestId("sidebar-draft-row");
    await userEvent.click(row);
    act(() => {
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "Edited without sidebar churn");
    });

    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "Original preview",
    );
    expect(screen.queryByText("Edited without sidebar churn")).not.toBeInTheDocument();
    expect(
      useChatDraftStore.getState().draftsById[draft.draftId].inputDraft,
    ).toBe("Edited without sidebar churn");
  });

  it("shows multiple drafts independently and discards only the selected one", async () => {
    const first = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore
      .getState()
      .updateDraftInput(first.draftId, "First unfinished task");
    const second = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore
      .getState()
      .updateDraftInput(second.draftId, "Second unfinished task");
    render(<SidebarDraftBlock catalog={catalog} filterPath={null} />);

    expect(screen.getAllByTestId("sidebar-draft-row")).toHaveLength(2);
    const secondRow = screen
      .getAllByTestId("sidebar-draft-row")
      .find((row) => row.textContent?.includes("Second unfinished task"));
    expect(secondRow).toBeDefined();
    await userEvent.click(
      within(secondRow!).getByRole("button", {
        name: "Discard draft: Second unfinished task",
      }),
    );

    expect(screen.getAllByTestId("sidebar-draft-row")).toHaveLength(1);
    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "First unfinished task",
    );
    expect(useChatDraftStore.getState().draftsById[second.draftId]).toBeUndefined();
    expect(useChatDraftStore.getState().draftsById[first.draftId]).toBeDefined();
  });

  it("surfaces attachment-only drafts and honors the project filter", () => {
    const repoDraft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    const chat = useAgentChatStore.getState();
    chat.ensureThread(repoDraft.threadId);
    chat.addStagedAttachment(repoDraft.threadId, {
      id: "att-1",
      kind: "file",
      ref: "/repo/README.md",
      metadata: { label: "README.md" },
    });

    const { rerender } = render(
      <SidebarDraftBlock catalog={catalog} filterPath="/repo" />,
    );
    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "1 attachment",
    );

    rerender(<SidebarDraftBlock catalog={catalog} filterPath="/other" />);
    expect(screen.queryByTestId("sidebar-draft-row")).not.toBeInTheDocument();
  });

  it("keeps invested drafts one click away in the collapsed rail", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore
      .getState()
      .updateDraftInput(draft.draftId, "Rail-accessible draft");
    render(
      <TooltipProvider>
        <SidebarRailDrafts catalog={catalog} />
      </TooltipProvider>,
    );

    const button = screen.getByRole("button", {
      name: "Open draft: Rail-accessible draft",
    });
    await userEvent.click(button);
    expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  });
});
