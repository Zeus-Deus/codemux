/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, within, act } from "@testing-library/react";
import { IssuePickerPanel } from "./issue-picker";
import type { GitHubIssue } from "@/tauri/types";

const mockListGithubIssues = vi.fn();
vi.mock("@/tauri/commands", () => ({
  listGithubIssues: (...args: unknown[]) => mockListGithubIssues(...args),
  listGithubIssuesByPath: (...args: unknown[]) => mockListGithubIssues(...args),
}));

const flush = () => act(() => new Promise((r) => setTimeout(r, 10)));

const SAMPLE_ISSUES: GitHubIssue[] = [
  { number: 92, title: "Backend endpoints voor prospectielijst", state: "Open", labels: ["enhancement"], assignees: ["zeus"], url: "https://github.com/u/r/issues/92", body: null },
  { number: 85, title: "Fix login page redirect", state: "Open", labels: ["bug"], assignees: [], url: "https://github.com/u/r/issues/85", body: null },
  { number: 70, title: "Add dark mode toggle", state: "Closed", labels: [], assignees: [], url: "https://github.com/u/r/issues/70", body: null },
];

function renderPanel(overrides: Partial<React.ComponentProps<typeof IssuePickerPanel>> = {}) {
  const props = {
    workspaceId: "ws-1",
    open: true,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
  const result = render(<IssuePickerPanel {...props} />);
  const view = within(result.container);
  return { ...result, props, view };
}

describe("IssuePickerPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListGithubIssues.mockResolvedValue(SAMPLE_ISSUES);
  });

  it("shows skeleton loading state initially", () => {
    mockListGithubIssues.mockReturnValue(new Promise(() => {}));
    const { container } = renderPanel();
    // Skeleton rows have animate-pulse class
    const skeletons = container.querySelectorAll(".animate-pulse");
    expect(skeletons.length).toBeGreaterThan(0);
  });

  it("shows header and search input immediately (no freeze)", () => {
    mockListGithubIssues.mockReturnValue(new Promise(() => {}));
    const { view } = renderPanel();
    expect(view.getByText("Open Issues")).toBeInTheDocument();
    expect(view.getByPlaceholderText("Search issues...")).toBeInTheDocument();
  });

  it("renders issue list after data loads", async () => {
    const { view } = renderPanel();
    await flush();
    expect(view.getByText("Backend endpoints voor prospectielijst")).toBeInTheDocument();
    expect(view.getByText("Fix login page redirect")).toBeInTheDocument();
    expect(view.getByText("Add dark mode toggle")).toBeInTheDocument();
  });

  it("filters issues client-side on search input", async () => {
    const { view } = renderPanel();
    await flush();

    fireEvent.change(view.getByPlaceholderText("Search issues..."), {
      target: { value: "login" },
    });

    expect(view.getByText("Fix login page redirect")).toBeInTheDocument();
    expect(view.queryByText("Backend endpoints voor prospectielijst")).not.toBeInTheDocument();
  });

  it("shows empty state when no issues match search", async () => {
    const { view } = renderPanel();
    await flush();

    fireEvent.change(view.getByPlaceholderText("Search issues..."), {
      target: { value: "zzz-no-match-zzz" },
    });

    expect(view.getByText("No issues found")).toBeInTheDocument();
  });

  it("shows empty state for no open issues", async () => {
    mockListGithubIssues.mockResolvedValue([]);
    const { view } = renderPanel();
    await flush();
    expect(view.getByText("No open issues")).toBeInTheDocument();
  });

  it("calls server search after debounce when no local matches", async () => {
    const { view } = renderPanel();
    await flush();

    mockListGithubIssues.mockResolvedValueOnce([
      { number: 104, title: "Server result", state: "Closed", labels: [], assignees: [], url: "", body: null },
    ]);

    fireEvent.change(view.getByPlaceholderText("Search issues..."), {
      target: { value: "#104" },
    });

    await act(() => new Promise((r) => setTimeout(r, 350)));

    expect(mockListGithubIssues).toHaveBeenCalledWith("ws-1", "104");
  });

  it("calls onSelect and onClose when issue row clicked", async () => {
    const { props, view } = renderPanel();
    await flush();

    const options = view.getAllByRole("option");
    fireEvent.click(options[0]);

    expect(props.onSelect).toHaveBeenCalledWith(SAMPLE_ISSUES[0]);
    expect(props.onClose).toHaveBeenCalled();
  });

  it("keyboard: arrow down moves focus, Enter selects", async () => {
    const { props, view } = renderPanel();
    await flush();

    const panel = view.getByTestId("issue-picker-panel");
    fireEvent.keyDown(panel, { key: "ArrowDown" });
    fireEvent.keyDown(panel, { key: "Enter" });

    expect(props.onSelect).toHaveBeenCalledWith(SAMPLE_ISSUES[1]);
  });

  it("keyboard: Escape calls onClose", async () => {
    const { props, view } = renderPanel();
    await flush();

    const panel = view.getByTestId("issue-picker-panel");
    fireEvent.keyDown(panel, { key: "Escape" });

    expect(props.onClose).toHaveBeenCalled();
  });

  it("shows error state when gh auth fails", async () => {
    mockListGithubIssues.mockRejectedValue("gh CLI is not authenticated. Run: gh auth login");
    const { view } = renderPanel();
    await flush();
    expect(view.getByText("Connect GitHub to link issues")).toBeInTheDocument();
  });

  it("search input has correct placeholder", () => {
    const { view } = renderPanel();
    expect(view.getByPlaceholderText("Search issues...")).toBeInTheDocument();
  });

  it("filters by issue number with # prefix", async () => {
    const { view } = renderPanel();
    await flush();

    fireEvent.change(view.getByPlaceholderText("Search issues..."), {
      target: { value: "#85" },
    });

    expect(view.getByText("Fix login page redirect")).toBeInTheDocument();
    expect(view.queryByText("Backend endpoints voor prospectielijst")).not.toBeInTheDocument();
  });
});
