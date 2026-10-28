/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const {
  cloneProjectFlowMock,
  createProjectFlowMock,
  openProjectFlowMock,
  pickFolderMock,
  pointHomeDraftAtMock,
} = vi.hoisted(() => ({
  cloneProjectFlowMock: vi.fn(),
  createProjectFlowMock: vi.fn(),
  openProjectFlowMock: vi.fn(),
  pickFolderMock: vi.fn(),
  pointHomeDraftAtMock: vi.fn(),
}));

vi.mock("@/hooks/use-project-actions", () => ({
  cloneProjectFlow: cloneProjectFlowMock,
  createProjectFlow: createProjectFlowMock,
  openProjectFlow: openProjectFlowMock,
  openDroppedFolder: vi.fn(),
  openProjectAtPath: vi.fn(),
  pointHomeDraftAt: pointHomeDraftAtMock,
}));
vi.mock("@/lib/file-dialog", () => ({ pickFolder: pickFolderMock }));
vi.mock("@tauri-apps/api/event", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/event")>()),
  listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dbGetRecentProjects: vi.fn().mockResolvedValue([]),
}));

import { AddProjectSheet } from "./add-project-sheet";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";

afterEach(cleanup);
beforeEach(() => {
  cloneProjectFlowMock.mockReset().mockResolvedValue({ path: "/cloned", name: "cloned" });
  createProjectFlowMock.mockReset().mockResolvedValue({ path: "/created", name: "created" });
  pointHomeDraftAtMock.mockReset();
  openProjectFlowMock.mockReset().mockResolvedValue({ success: false });
  pickFolderMock.mockReset().mockResolvedValue(null);
  useAppStore.setState({ homeDir: "/home/user" });
  useFeatureFlags.setState({ enableAgentChat: true, enableLazyWorkspaceCreation: true });
  useUIStore.setState({ addProjectSheet: null, projectsLocation: null });
});

function openSheet(tab: "open" | "clone" | "new") {
  render(<AddProjectSheet />);
  act(() => useUIStore.getState().setAddProjectSheet(tab));
}

const destination = () => screen.getByTestId("destination-path").textContent;

describe("AddProjectSheet", () => {
  it("opens on the requested tab", () => {
    openSheet("clone");
    expect(screen.getByRole("tab", { name: "Clone" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("Repository")).toHaveFocus();
  });

  describe("Clone", () => {
    it("clones owner/repo from GitHub into ~/Projects and closes", async () => {
      openSheet("clone");
      fireEvent.change(screen.getByLabelText("Repository"), {
        target: { value: "acme/field-notes" },
      });
      expect(screen.getByLabelText("Folder name")).toHaveValue("field-notes");
      expect(destination()).toBe("~/Projects/field-notes");

      fireEvent.click(screen.getByRole("button", { name: /Clone/ }));
      await vi.waitFor(() => expect(useUIStore.getState().addProjectSheet).toBeNull());
      expect(cloneProjectFlowMock).toHaveBeenCalledWith(
        "https://github.com/acme/field-notes.git",
        "/home/user/Projects/field-notes",
      );
      // The landing's draft follows the new project.
      expect(pointHomeDraftAtMock).toHaveBeenCalledWith("/cloned");
    });

    it("never clones into a relative path, even before the home dir is known", async () => {
      useAppStore.setState({ homeDir: null });
      openSheet("clone");
      fireEvent.change(screen.getByLabelText("Repository"), {
        target: { value: "git@github.com:acme/app.git" },
      });
      fireEvent.submit(screen.getByLabelText("Repository").closest("form")!);
      await vi.waitFor(() => expect(cloneProjectFlowMock).toHaveBeenCalled());
      expect(cloneProjectFlowMock).toHaveBeenCalledWith(
        "git@github.com:acme/app.git",
        "~/Projects/app",
      );
    });

    it("uses the remembered location and lets the user change it", async () => {
      useUIStore.setState({ projectsLocation: "/home/user/Code" });
      pickFolderMock.mockResolvedValue("/mnt/work");
      openSheet("clone");
      fireEvent.change(screen.getByLabelText("Repository"), {
        target: { value: "acme/app" },
      });
      expect(destination()).toBe("~/Code/app");

      fireEvent.click(screen.getByRole("button", { name: "Change" }));
      await vi.waitFor(() => expect(useUIStore.getState().projectsLocation).toBe("/mnt/work"));
      expect(destination()).toBe("/mnt/work/app");
    });

    it("shows a failed clone inline and stays open", async () => {
      cloneProjectFlowMock.mockRejectedValue("fatal: repository not found");
      openSheet("clone");
      fireEvent.change(screen.getByLabelText("Repository"), {
        target: { value: "acme/missing" },
      });
      fireEvent.click(screen.getByRole("button", { name: /Clone/ }));
      expect(await screen.findByRole("alert")).toHaveTextContent("fatal: repository not found");
      expect(useUIStore.getState().addProjectSheet).toBe("clone");
    });

    it("hands SSH host aliases to git unchanged", async () => {
      openSheet("clone");
      fireEvent.change(screen.getByLabelText("Repository"), {
        target: { value: "work-git:team/repo.git" },
      });
      expect(destination()).toBe("~/Projects/repo");
      fireEvent.click(screen.getByRole("button", { name: /Clone/ }));
      await vi.waitFor(() => expect(cloneProjectFlowMock).toHaveBeenCalled());
      expect(cloneProjectFlowMock).toHaveBeenCalledWith(
        "work-git:team/repo.git",
        "/home/user/Projects/repo",
      );
    });
  });

  describe("New", () => {
    it("creates an empty repository in ~/Projects", async () => {
      openSheet("new");
      const name = screen.getByLabelText("Name");
      expect(name).toHaveFocus();
      fireEvent.change(name, { target: { value: "habits" } });
      expect(destination()).toBe("~/Projects/habits");
      fireEvent.click(screen.getByRole("button", { name: /Create/ }));
      await vi.waitFor(() => expect(useUIStore.getState().addProjectSheet).toBeNull());
      expect(createProjectFlowMock).toHaveBeenCalledWith(
        "/home/user/Projects",
        "habits",
        undefined,
      );
    });

    it("hands the description to the new project's first chat on Ctrl+Enter", async () => {
      openSheet("new");
      fireEvent.change(screen.getByLabelText("Name"), { target: { value: "habits" } });
      fireEvent.click(screen.getByRole("radio", { name: /Describe it/ }));
      const description = screen.getByLabelText("What should the agent build?");
      expect(description).toHaveFocus();
      fireEvent.change(description, { target: { value: "  A habit tracker  " } });
      fireEvent.keyDown(description, { key: "Enter", ctrlKey: true });
      await vi.waitFor(() => expect(useUIStore.getState().addProjectSheet).toBeNull());
      expect(createProjectFlowMock).toHaveBeenCalledWith(
        "/home/user/Projects",
        "habits",
        "A habit tracker",
      );
      // That flow brings its own draft; nothing else is retargeted.
      expect(pointHomeDraftAtMock).not.toHaveBeenCalled();
    });

    it("asks for a description before an agent can build it", () => {
      openSheet("new");
      fireEvent.change(screen.getByLabelText("Name"), { target: { value: "habits" } });
      fireEvent.click(screen.getByRole("radio", { name: /Describe it/ }));
      fireEvent.click(screen.getByRole("button", { name: /Create/ }));
      expect(screen.getByRole("alert")).toHaveTextContent("Describe what the agent should build.");
      expect(createProjectFlowMock).not.toHaveBeenCalled();
    });

    it("refuses a name that would be a path", () => {
      openSheet("new");
      fireEvent.change(screen.getByLabelText("Name"), { target: { value: "../escape" } });
      fireEvent.click(screen.getByRole("button", { name: /Create/ }));
      expect(screen.getByRole("alert")).toHaveTextContent("single folder name");
      expect(createProjectFlowMock).not.toHaveBeenCalled();
    });

    it("offers only the empty repository without the agent chat interface", () => {
      useFeatureFlags.setState({ enableAgentChat: false, enableLazyWorkspaceCreation: false });
      openSheet("new");
      expect(screen.queryByRole("radio", { name: /Describe it/ })).toBeNull();
    });
  });

  describe("Open", () => {
    it("closes once a chosen folder is open", async () => {
      openProjectFlowMock.mockResolvedValue({ success: true, path: "/work/api", name: "api" });
      openSheet("open");
      fireEvent.click(screen.getByRole("button", { name: /Choose folder/ }));
      await vi.waitFor(() => expect(useUIStore.getState().addProjectSheet).toBeNull());
    });

    it("stays open when the picker is cancelled", async () => {
      openSheet("open");
      fireEvent.click(screen.getByRole("button", { name: /Choose folder/ }));
      await vi.waitFor(() => expect(openProjectFlowMock).toHaveBeenCalled());
      expect(useUIStore.getState().addProjectSheet).toBe("open");
    });
  });
});
