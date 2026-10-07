/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { PendingWorkspaceRow } from "./pending-workspace-row";
import { useUIStore } from "@/stores/ui-store";
import type { NewWorkspaceDraft, PendingWorkspace } from "@/tauri/types";

const draft: NewWorkspaceDraft = {
  projectDir: "/projects/app",
  workspaceName: "",
  branchName: "fix/login",
  branchAutoFilled: false,
  prompt: "Fix the login race",
  attachments: ["/tmp/shot.png"],
  linkedIssue: null,
  selectedAgentId: "builtin-claude",
  modelSelection: { model: null, reasoning: null, context: null },
  baseBranch: "main",
  branchMode: "create_new",
  openExistingBranch: null,
  hostId: null,
};

function renderRow(pending: PendingWorkspace) {
  useUIStore.setState({ pendingWorkspaces: [pending] });
  return render(<PendingWorkspaceRow pending={pending} />);
}

afterEach(cleanup);

beforeEach(() => {
  useUIStore.setState({
    pendingWorkspaces: [],
    showNewWorkspaceDialog: false,
    newWorkspaceProjectDir: null,
    newWorkspaceDraft: null,
  });
});

describe("PendingWorkspaceRow", () => {
  it("shows the name while creating, with no actions", () => {
    renderRow({
      id: "p1",
      name: "Fix the login race",
      projectPath: "/projects/app",
      status: "creating",
      draft,
    });
    expect(screen.getByText("Fix the login race")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("reopens the dialog with the draft when a failed row is clicked", () => {
    renderRow({
      id: "p1",
      name: "Fix the login race",
      projectPath: "/projects/app",
      status: "failed",
      errorMessage: "fatal: already exists",
      draft,
    });

    fireEvent.click(screen.getByRole("button", { name: /Reopen/ }));

    const state = useUIStore.getState();
    expect(state.showNewWorkspaceDialog).toBe(true);
    expect(state.newWorkspaceProjectDir).toBe("/projects/app");
    expect(state.newWorkspaceDraft).toBe(draft);
    expect(state.pendingWorkspaces).toEqual([]);
  });

  it("dismisses a failed row without reopening", () => {
    renderRow({
      id: "p1",
      name: "x",
      projectPath: "/projects/app",
      status: "failed",
      errorMessage: "boom",
      draft,
    });

    fireEvent.click(
      screen.getByRole("button", { name: "Dismiss failed workspace" }),
    );

    const state = useUIStore.getState();
    expect(state.pendingWorkspaces).toEqual([]);
    expect(state.showNewWorkspaceDialog).toBe(false);
  });

  it("keeps a draftless failure as a plain status row", () => {
    renderRow({
      id: "p1",
      name: "x",
      projectPath: "/projects/app",
      status: "failed",
      errorMessage: "boom",
    });
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("a normal open clears any leftover draft", () => {
    useUIStore.setState({ newWorkspaceDraft: draft });
    useUIStore.getState().setShowNewWorkspaceDialog(true, "/projects/other");
    expect(useUIStore.getState().newWorkspaceDraft).toBeNull();
  });
});
