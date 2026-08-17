/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const mockGetCheckLogExcerpt = vi
  .fn()
  .mockResolvedValue("AssertionError: expected 2 calls, received 1");

vi.mock("@/tauri/commands", () => ({
  getCheckLogExcerpt: (...a: unknown[]) => mockGetCheckLogExcerpt(...a),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

const mockToastError = vi.fn();
vi.mock("@/lib/toast", () => ({
  toast: {
    error: (...a: unknown[]) => mockToastError(...a),
    success: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  },
}));

import { ReviewChecks } from "./review-checks";
import type { CheckInfo } from "@/tauri/types";

function check(name: string, conclusion: string): CheckInfo {
  return {
    name,
    status: "COMPLETED",
    conclusion,
    elapsed_time: "1m 04s",
    detail_url: `https://github.com/acme/app/actions/${name}`,
    started_at: null,
    completed_at: null,
  };
}

const FAILING = check("web-checks (windows-latest)", "fail");

function renderChecks(over: Partial<Parameters<typeof ReviewChecks>[0]> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchInterval: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ReviewChecks
        checks={[check("build", "pass"), FAILING]}
        cwd="/repo"
        prNumber={285}
        {...over}
      />
    </QueryClientProvider>,
  );
}

const flush = () => act(() => new Promise((r) => setTimeout(r, 0)));

beforeEach(() => {
  vi.clearAllMocks();
  mockGetCheckLogExcerpt.mockResolvedValue("AssertionError: expected 2 calls, received 1");
});
afterEach(cleanup);

describe("ReviewChecks — Fix with agent", () => {
  it("is not drawn when no handoff is wired", async () => {
    renderChecks();
    await flush();
    expect(screen.queryByText("Fix with agent")).not.toBeInTheDocument();
    // The rest of the failing card is unaffected.
    expect(screen.getByText("Full log")).toBeInTheDocument();
  });

  it("renders on the failing card, with a caption saying what it does", async () => {
    renderChecks({
      onFixWithAgent: vi.fn().mockResolvedValue(undefined),
      handoffCaption: "opens a thread in this workspace",
    });
    await flush();

    expect(screen.getByText("Fix with agent")).toBeInTheDocument();
    expect(screen.getByText("opens a thread in this workspace")).toBeInTheDocument();
  });

  it("is offered only for the checks that failed", async () => {
    renderChecks({
      checks: [check("build", "pass"), check("lint", "pass")],
      onFixWithAgent: vi.fn().mockResolvedValue(undefined),
    });
    await flush();
    expect(screen.queryByText("Fix with agent")).not.toBeInTheDocument();
  });

  it("hands over the check and the excerpt it already loaded", async () => {
    const onFixWithAgent = vi.fn().mockResolvedValue(undefined);
    renderChecks({ onFixWithAgent });
    // The failing card opens itself, so the excerpt query runs.
    await waitFor(() =>
      expect(screen.getByText(/AssertionError/)).toBeInTheDocument(),
    );

    await userEvent.click(screen.getByTestId(`fix-with-agent-${FAILING.name}`));
    await flush();

    expect(onFixWithAgent).toHaveBeenCalledTimes(1);
    const [passedCheck, excerpt] = onFixWithAgent.mock.calls[0];
    expect(passedCheck.name).toBe(FAILING.name);
    expect(excerpt).toBe("AssertionError: expected 2 calls, received 1");
  });

  it("keeps the button's box while the handoff is in flight", async () => {
    let release: () => void = () => {};
    const onFixWithAgent = vi.fn(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    renderChecks({ onFixWithAgent });
    await flush();

    const button = screen.getByTestId(`fix-with-agent-${FAILING.name}`);
    const before = button.className;

    await userEvent.click(button);

    // Binding rule 1: the label changes, the box does not.
    expect(button).toHaveTextContent("Starting agent");
    expect(button.className).toBe(before);
    expect(button).toBeDisabled();

    await act(async () => {
      release();
      await Promise.resolve();
    });
    await flush();
    expect(button).toHaveTextContent("Fix with agent");
    expect(button).not.toBeDisabled();
  });

  it("reports a failed handoff and restores the button", async () => {
    const onFixWithAgent = vi.fn().mockRejectedValue(new Error("claude is not installed"));
    renderChecks({ onFixWithAgent });
    await flush();

    await userEvent.click(screen.getByTestId(`fix-with-agent-${FAILING.name}`));
    await flush();

    expect(mockToastError).toHaveBeenCalledWith(
      expect.stringContaining("claude is not installed"),
    );
    expect(screen.getByTestId(`fix-with-agent-${FAILING.name}`)).not.toBeDisabled();
  });
});
