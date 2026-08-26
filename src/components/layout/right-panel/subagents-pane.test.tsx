/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";

import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import { SubagentsPane } from "./subagents-pane";

function subagent(overrides: Partial<SubagentView>): SubagentView {
  return {
    id: "sub-1",
    name: "Explore",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function prompt(id: string, seq: number, text: string): ChatViewItem {
  return { kind: "user_message", id, seq, text };
}

function run(id: string, seq: number, subagents: SubagentView[]): ChatViewItem {
  return { kind: "subagent_run", id, seq, turn_id: `turn-${id}`, subagents };
}

/** One wave: the shape every pre-wave test used. */
function messages(subagents: SubagentView[]): ChatViewItem[] {
  return [
    prompt("u-1", 0, "Implement clipboard-paste fallback"),
    run("run-1", 1, subagents),
  ];
}

/** Two waves — the design fixture: an older implement wave, then a newer
 *  analysis wave with one failure. */
function twoWaves(): ChatViewItem[] {
  return [
    prompt("u-1", 0, "Implement + verify\nsecond line is not the title"),
    run("run-1", 1, [
      subagent({
        id: "impl",
        name: "Subagent",
        model: "haiku",
        status: "completed",
        resultText: "Patched session.ts, all 14 tests green",
        durationMs: 371_000,
        totalTokens: 400_000,
      }),
      subagent({
        id: "verify",
        name: "Subagent",
        model: "haiku",
        status: "stopped",
        resultText: "Stopped — superseded by wave 2 findings",
        durationMs: 67_000,
        totalTokens: 240_000,
      }),
    ]),
    prompt("u-2", 2, "Issue analysis"),
    run("run-2", 3, [
      subagent({
        id: "explore-1",
        model: "opus",
        status: "completed",
        resultText:
          "Root cause: applyRuntimeInfo never re-registers the tile delegate\nmore detail",
        durationMs: 207_000,
      }),
      subagent({
        id: "explore-2",
        model: "opus",
        status: "failed",
        resultText: "Failed — sandbox denied network access during fetch",
        durationMs: 185_000,
      }),
    ]),
  ];
}

beforeEach(() => useUIStore.setState({ subagentEnterRequest: null }));
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("SubagentsPane — grouped waves with result-first rows", () => {
  it("shows aggregate progress and titles the wave with its prompt", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "live",
            name: "Pricing audit",
            model: "anthropic/claude-opus-4-8",
            activity: "reading pricing.ts…",
          }),
          subagent({
            id: "done",
            name: "Verification pass",
            model: "openai/gpt-5.4",
            status: "completed",
            resultText: "All checks pass\nDetails follow",
            durationMs: 16_000,
          }),
        ])}
      />,
    );

    expect(screen.getByText("1 / 2")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "1",
    );
    const wave = screen.getByRole("region", {
      name: "Implement clipboard-paste fallback",
    });
    expect(wave).toHaveAttribute("data-wave-status", "running");
    expect(screen.getByText("2 agents · 1 running")).toBeInTheDocument();
    // Both rows: name, model capsule, and the excerpt line.
    expect(screen.getByText("Pricing audit")).toBeInTheDocument();
    expect(screen.getByText("Verification pass")).toBeInTheDocument();
    expect(
      screen.getByTitle("Model: anthropic/claude-opus-4-8"),
    ).toHaveTextContent("anthropic/claude-opus-4-8");
    expect(screen.getByTitle("Model: openai/gpt-5.4")).toHaveTextContent(
      "openai/gpt-5.4",
    );
    expect(screen.getByText("reading pricing.ts…")).toBeInTheDocument();
    expect(screen.getByText("All checks pass")).toBeInTheDocument();
  });

  it("falls back to 'Ran N subagents' when no prompt precedes the wave", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={[
          run("run-1", 0, [
            subagent({ id: "a", status: "completed" }),
            subagent({ id: "b", name: "Verify", status: "completed" }),
          ]),
        ]}
      />,
    );
    expect(
      screen.getByRole("region", { name: "Ran 2 subagents" }),
    ).toBeInTheDocument();
  });

  it("keeps the latest wave open, folds older ones, and toggles on click", () => {
    render(<SubagentsPane threadId="thread-1" messages={twoWaves()} />);

    const older = screen.getByRole("button", { name: /^Implement \+ verify/ });
    const newer = screen.getByRole("button", { name: /^Issue analysis/ });
    expect(older).toHaveAttribute("aria-expanded", "false");
    expect(newer).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.queryByText("Patched session.ts, all 14 tests green"),
    ).toBeNull();
    expect(
      screen.getByText(
        "Root cause: applyRuntimeInfo never re-registers the tile delegate",
      ),
    ).toBeInTheDocument();

    fireEvent.click(older);
    expect(older).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getByText("Patched session.ts, all 14 tests green"),
    ).toBeInTheDocument();

    fireEvent.click(newer);
    expect(newer).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText(
        "Root cause: applyRuntimeInfo never re-registers the tile delegate",
      ),
    ).toBeNull();
  });

  it("surfaces a failure on the wave header and numbers repeated names", () => {
    render(<SubagentsPane threadId="thread-1" messages={twoWaves()} />);

    const failedWave = screen.getByRole("region", { name: "Issue analysis" });
    expect(failedWave).toHaveAttribute("data-wave-status", "failed");
    expect(screen.getByText("2 agents · 1 failed")).toBeInTheDocument();
    // Folded wave still reports its rollup, including tokens and a halt.
    const olderWave = screen.getByRole("region", {
      name: "Implement + verify",
    });
    expect(olderWave).toHaveAttribute("data-wave-status", "stopped");
    expect(
      screen.getByText("2 agents · 1 stopped · Σ 640K tok"),
    ).toBeInTheDocument();
    // Two "Explore" rows → "Explore 1" / "Explore 2".
    expect(
      screen.getByRole("button", { name: "Open Explore 1 thread" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open Explore 2 thread" }),
    ).toBeInTheDocument();
  });

  it("omits the model capsule when the provider has not reported one", () => {
    const { container } = render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ model: undefined })])}
      />,
    );

    expect(container.querySelector("[data-subagent-model]")).toBeNull();
  });

  it("keeps the header timer live while a sibling has failed", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(100_000));
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({ id: "a", status: "failed", durationMs: 5_000 }),
          subagent({ id: "b", status: "running", startedAt: 90_000 }),
        ])}
      />,
    );

    const wave = screen.getByRole("region", {
      name: "Implement clipboard-paste fallback",
    });
    // The failure still wins the header glyph...
    expect(wave).toHaveAttribute("data-wave-status", "failed");
    // ...but the elapsed label follows the agent that is still running.
    const header = within(wave).getByRole("button", { expanded: true });
    const elapsed = within(header).getByText("0m 10s");
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(elapsed).toHaveTextContent("0m 13s");
  });

  it("opens a subagent thread from its row", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ id: "sub-42" })])}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Open Explore thread" }),
    );
    expect(useUIStore.getState().subagentEnterRequest).toMatchObject({
      threadId: "thread-1",
      subagentId: "sub-42",
    });
  });
});
