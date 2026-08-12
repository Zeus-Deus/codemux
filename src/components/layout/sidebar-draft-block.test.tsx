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
  useFrozenActiveDraftRow,
  useVisibleSidebarDraftCount,
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

/** Mirrors how the sidebar inbox wires the block: one frozen snapshot feeds
 *  both the rendered rows and the count behind the "Nothing active"
 *  placeholder, so every test also asserts they cannot disagree. */
function DraftBlockHarness(props: { filterPath: string | null }) {
  const frozenActive = useFrozenActiveDraftRow(catalog, props.filterPath);
  const visibleCount = useVisibleSidebarDraftCount(
    catalog,
    props.filterPath,
    frozenActive,
  );
  return (
    <>
      <div data-testid="visible-draft-count">{visibleCount}</div>
      <SidebarDraftBlock
        catalog={catalog}
        filterPath={props.filterPath}
        frozenActive={frozenActive}
      />
    </>
  );
}

function expectCountMatchesRows() {
  const rows = screen.queryAllByTestId("sidebar-draft-row").length;
  expect(screen.getByTestId("visible-draft-count").textContent).toBe(
    String(rows),
  );
  return rows;
}

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
    render(<DraftBlockHarness filterPath={null} />);

    act(() => {
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "Investigate the flaky build");
    });
    expect(screen.queryByTestId("sidebar-draft-row")).not.toBeInTheDocument();
    // The placeholder count must agree: no row means nothing to count.
    expect(expectCountMatchesRows()).toBe(0);

    act(() => {
      useChatDraftStore.getState().setActiveDraft(null);
    });
    const row = screen.getByTestId("sidebar-draft-row");
    expectCountMatchesRows();
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
    render(<DraftBlockHarness filterPath={null} />);

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
    render(<DraftBlockHarness filterPath={null} />);

    expect(screen.getAllByTestId("sidebar-draft-row")).toHaveLength(2);
    expectCountMatchesRows();
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

    const { rerender } = render(<DraftBlockHarness filterPath="/repo" />);
    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "1 attachment",
    );
    expectCountMatchesRows();

    rerender(<DraftBlockHarness filterPath="/other" />);
    expect(screen.queryByTestId("sidebar-draft-row")).not.toBeInTheDocument();
    expect(expectCountMatchesRows()).toBe(0);
  });

  it("keeps the count silent for a draft activated while empty", () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);
    render(<DraftBlockHarness filterPath={null} />);

    act(() => {
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "Typing into a fresh draft");
    });

    // Frozen-null: the block deliberately renders nothing, so the count must
    // stay 0 or the inbox hides its "Nothing active" placeholder for a row
    // that does not exist.
    expect(screen.queryByTestId("sidebar-draft-block")).not.toBeInTheDocument();
    expect(expectCountMatchesRows()).toBe(0);
  });

  it("recaptures the active row when the project filter stops hiding it", () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/repo");
    useChatDraftStore
      .getState()
      .updateDraftInput(draft.draftId, "Invested before activation");
    const { rerender } = render(<DraftBlockHarness filterPath="/other" />);

    act(() => {
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
    });
    expect(expectCountMatchesRows()).toBe(0);

    rerender(<DraftBlockHarness filterPath={null} />);
    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "Invested before activation",
    );
    expect(expectCountMatchesRows()).toBe(1);

    // Still frozen: edits after the recapture do not repaint the preview.
    act(() => {
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "Edited after recapture");
    });
    expect(screen.getByTestId("sidebar-draft-row")).toHaveTextContent(
      "Invested before activation",
    );
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
