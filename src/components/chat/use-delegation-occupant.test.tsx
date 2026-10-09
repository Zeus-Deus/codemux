/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

import type { ChatViewItem, SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";

import { ComposerStrip } from "./ComposerStrip";
import {
  useDelegationOccupant,
  useSubagentOccupant,
} from "./use-composer-strip-occupants";

vi.mock("@/assets/preset-icons/claude.svg", () => ({ default: "/mock/claude.svg" }));
vi.mock("@/assets/preset-icons/codex.svg", () => ({ default: "/mock/codex.svg" }));

afterEach(() => {
  cleanup();
});

function delegated(thread: string, overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: `delegate:${thread}`,
    name: "Codex",
    agentType: "codex",
    description: "Add slugify helper",
    status: "running",
    activity: "Working in its tab",
    items: [],
    toneIndex: 0,
    startedAt: Date.now() - 134_000,
    ...overrides,
  };
}

const claude = (overrides: Partial<SubagentView> = {}) =>
  delegated("child-2", {
    name: "Claude",
    agentType: "claude",
    description: "Review the parser",
    ...overrides,
  });

function card(id: string, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id, seq: 0, turn_id: "t1", subagents };
}

function Harness({
  messages,
  streaming = false,
  threadId = "t1",
  onOpen = () => {},
  onStop = () => {},
}: {
  messages: ChatViewItem[];
  streaming?: boolean;
  threadId?: string;
  onOpen?: (thread: string) => void;
  onStop?: (provider: string, thread: string) => void;
}) {
  const delegation = useDelegationOccupant({ messages, threadId, streaming, onOpen, onStop });
  const subagents = useSubagentOccupant({ messages, threadId, streaming, onJump: () => {} });
  return <ComposerStrip occupants={[delegation, subagents]} />;
}

const rows = () => screen.queryAllByTestId("composer-strip-row");

describe("delegation occupant", () => {
  it("reads 'Continues when Codex finishes' while the parent is idle", () => {
    render(<Harness messages={[card("c1", [delegated("child-1")])]} />);
    const [row] = rows();
    expect(row).toHaveAttribute("data-kind", "delegation");
    expect(row).toHaveTextContent("Continues when Codex finishes");
    expect(row).toHaveTextContent("Add slugify helper · Working in its tab");
    expect(row).toHaveTextContent("2m 14s");
    expect(within(row).getByTestId("composer-strip-delegation-open")).toHaveTextContent("Open");
    expect(within(row).getByTestId("composer-strip-delegation-stop")).toHaveTextContent("Stop");
  });

  it("is calm: no sweep, and never the subagent bar", () => {
    render(<Harness messages={[card("c1", [delegated("child-1")])]} />);
    expect(screen.queryByTestId("composer-strip-sweep")).toBeNull();
    expect(screen.queryByText(/subagents? running/)).toBeNull();
    expect(rows()).toHaveLength(1);
  });

  it("only notes that reports post here while the parent streams", () => {
    render(<Harness streaming messages={[card("c1", [delegated("child-1"), claude()])]} />);
    const [row] = rows();
    expect(row).toHaveTextContent("2 delegated tasks running");
    expect(row).toHaveTextContent("reports post here");
  });

  it("opens the child chat and stops quietly, holding Stopping… until it settles", () => {
    const onOpen = vi.fn();
    const onStop = vi.fn();
    const messages = [card("c1", [delegated("child-1")])];
    const { rerender } = render(<Harness messages={messages} onOpen={onOpen} onStop={onStop} />);
    fireEvent.click(screen.getByTestId("composer-strip-delegation-open"));
    expect(onOpen).toHaveBeenCalledWith("child-1");
    fireEvent.click(screen.getByTestId("composer-strip-delegation-stop"));
    expect(onStop).toHaveBeenCalledWith("codex", "child-1");
    const stop = screen.getByTestId("composer-strip-delegation-stop");
    expect(stop).toHaveTextContent("Stopping…");
    expect(stop).toBeDisabled();
    fireEvent.click(stop);
    expect(onStop).toHaveBeenCalledTimes(1);
    rerender(
      <Harness
        messages={[card("c1", [delegated("child-1", { status: "stopped" })])]}
        onOpen={onOpen}
        onStop={onStop}
      />,
    );
    expect(rows()).toHaveLength(0);
  });

  it("drops a pending Stopping… on a thread switch", () => {
    const messages = [card("c1", [delegated("child-1")])];
    const { rerender } = render(<Harness messages={messages} />);
    fireEvent.click(screen.getByTestId("composer-strip-delegation-stop"));
    expect(screen.getByTestId("composer-strip-delegation-stop")).toHaveTextContent("Stopping…");
    rerender(<Harness messages={messages} threadId="t2" />);
    expect(screen.getByTestId("composer-strip-delegation-stop")).toHaveTextContent(/^Stop$/);
  });

  it("puts Stop all on the first row, which survives opening the strip", () => {
    const onStop = vi.fn();
    render(
      <Harness
        messages={[
          card("c1", [delegated("child-1")]),
          card("c2", [claude({ activity: "Waiting for your answer in its tab" })]),
        ]}
        onStop={onStop}
      />,
    );
    const [lead] = rows();
    expect(lead).toHaveTextContent("Continues when 2 agents finish");
    expect(lead).toHaveTextContent("1 working · 1 waiting for you");
    expect(within(lead).getByTestId("composer-strip-delegation-stop-all")).toHaveTextContent(
      "Stop all",
    );
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    const open = rows();
    expect(open.map((row) => row.getAttribute("data-row-id"))).toEqual([
      "delegation:all",
      "delegation:delegate:child-1",
      "delegation:delegate:child-2",
    ]);
    expect(within(open[0]).getByTestId("composer-strip-delegation-stop-all")).toBeInTheDocument();
    expect(open[2]).toHaveTextContent("Claude");
    expect(open[2]).toHaveTextContent("Review the parser · Waiting for your answer in its tab");

    fireEvent.click(within(open[0]).getByTestId("composer-strip-delegation-stop-all"));
    expect(onStop.mock.calls).toEqual([
      ["codex", "child-1"],
      ["claude", "child-2"],
    ]);
    expect(within(rows()[0]).getByTestId("composer-strip-delegation-stop-all")).toHaveTextContent(
      "Stopping…",
    );
    for (const row of rows().slice(1)) {
      expect(within(row).getByTestId("composer-strip-delegation-stop")).toHaveTextContent(
        "Stopping…",
      );
    }
  });

  it("never counts the Stop all header as a pending item", () => {
    const native: SubagentView = { id: "explore", status: "running", items: [], toneIndex: 0 };
    const { rerender } = render(
      <Harness messages={[card("c1", [delegated("child-1")]), card("c2", [claude()])]} />,
    );
    // Two tasks: the lead stands for one, so one more.
    expect(screen.getByTestId("composer-strip-toggle")).toHaveTextContent("+1");
    rerender(
      <Harness
        messages={[card("c1", [delegated("child-1")]), card("c2", [claude()]), card("c3", [native])]}
      />,
    );
    expect(screen.getByTestId("composer-strip-toggle")).toHaveTextContent("+2");
  });

  it("leaves native subagents to their own occupant", () => {
    const native: SubagentView = {
      id: "explore",
      name: "Explore",
      status: "running",
      items: [],
      toneIndex: 0,
    };
    render(<Harness streaming messages={[card("c1", [delegated("child-1")]), card("c2", [native])]} />);
    // Delegation leads; the subagent count names only the native one.
    expect(rows()[0]).toHaveAttribute("data-kind", "delegation");
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    const kinds = rows().map((row) => row.getAttribute("data-kind"));
    expect(kinds).toEqual(["delegation", "running"]);
    expect(rows()[1]).toHaveTextContent("Explore");
  });
});
