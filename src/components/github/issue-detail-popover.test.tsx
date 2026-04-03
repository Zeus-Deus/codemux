/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, within, act } from "@testing-library/react";
import { IssueDetailPopover } from "./issue-detail-popover";
import type { LinkedIssue, GitHubIssue } from "@/tauri/types";

const mockGetGithubIssue = vi.fn();
vi.mock("@/tauri/commands", () => ({
  getGithubIssue: (...args: unknown[]) => mockGetGithubIssue(...args),
}));

const flush = () => act(() => new Promise((r) => setTimeout(r, 10)));

const LINKED: LinkedIssue = {
  number: 104,
  title: "Add dark mode",
  state: "Open",
  labels: ["enhancement"],
};

const FULL_ISSUE: GitHubIssue = {
  number: 104,
  title: "Add dark mode",
  state: "Open",
  labels: ["enhancement", "ui"],
  assignees: ["zeus", "alice"],
  url: "https://github.com/org/repo/issues/104",
  body: "We need dark mode support.\n\nAcceptance criteria:\n- Toggle in settings\n- Persists across sessions",
};

function renderPopover() {
  const result = render(
    <IssueDetailPopover workspaceId="ws-1" issue={LINKED} />,
  );
  return { ...result, view: within(result.container) };
}

function clickTrigger(view: ReturnType<typeof within>) {
  const trigger = view.getByText(`#${LINKED.number}`).closest("button")!;
  fireEvent.click(trigger);
}

function getPopover() {
  return document.querySelector("[data-testid='issue-detail-content']") as HTMLElement | null;
}

describe("IssueDetailPopover", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetGithubIssue.mockResolvedValue(FULL_ISSUE);
  });

  it("shows loading skeleton when popover opens", async () => {
    mockGetGithubIssue.mockReturnValue(new Promise(() => {}));
    const { view } = renderPopover();
    clickTrigger(view);
    await act(() => new Promise((r) => setTimeout(r, 0)));

    const popover = getPopover();
    expect(popover).toBeTruthy();
    const skeletons = popover!.querySelectorAll("[data-slot='skeleton']");
    expect(skeletons.length).toBeGreaterThan(0);
  });

  it("renders full issue content after fetch", async () => {
    const { view } = renderPopover();
    clickTrigger(view);
    await flush();

    const popover = getPopover()!;
    expect(popover.textContent).toContain("Add dark mode");
    expect(popover.textContent).toContain("#104");
    expect(popover.textContent).toContain("enhancement");
    expect(popover.textContent).toContain("ui");
    expect(popover.textContent).toContain("zeus, alice");
    expect(popover.textContent).toContain("We need dark mode support.");
  });

  it("renders labels as badges", async () => {
    const { view } = renderPopover();
    clickTrigger(view);
    await flush();

    const popover = getPopover()!;
    const badges = popover.querySelectorAll("[data-slot='badge']");
    const badgeTexts = Array.from(badges).map((b) => b.textContent);
    expect(badgeTexts).toContain("enhancement");
    expect(badgeTexts).toContain("ui");
  });

  it("shows 'No description provided.' when body is null", async () => {
    mockGetGithubIssue.mockResolvedValue({ ...FULL_ISSUE, body: null });
    const { view } = renderPopover();
    clickTrigger(view);
    await flush();

    const popover = getPopover()!;
    expect(popover.textContent).toContain("No description provided.");
  });

  it("shows error state with retry button when fetch fails", async () => {
    mockGetGithubIssue.mockRejectedValueOnce("Network error");
    const { view } = renderPopover();
    clickTrigger(view);
    await flush();

    const popover = getPopover()!;
    expect(popover.textContent).toContain("Failed to load issue details");

    // Retry
    mockGetGithubIssue.mockResolvedValue(FULL_ISSUE);
    const retryBtn = within(popover).getByText("Retry");
    fireEvent.click(retryBtn);
    await flush();

    expect(popover.textContent).toContain("Add dark mode");
  });

  it("opens GitHub URL via window.open", async () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const { view } = renderPopover();
    clickTrigger(view);
    await flush();

    const popover = getPopover()!;
    const openBtn = within(popover).getByText("Open on GitHub");
    fireEvent.click(openBtn);

    expect(openSpy).toHaveBeenCalledWith(
      "https://github.com/org/repo/issues/104",
      "_blank",
    );
    openSpy.mockRestore();
  });

  it("click on chip does NOT propagate to parent", async () => {
    const parentClick = vi.fn();
    const { container } = render(
      <div onClick={parentClick}>
        <IssueDetailPopover workspaceId="ws-1" issue={LINKED} />
      </div>,
    );
    const trigger = within(container).getByText("#104").closest("button")!;
    fireEvent.click(trigger);

    expect(parentClick).not.toHaveBeenCalled();
  });
});
