/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";

import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
} from "@/lib/agent-chat/types";

import {
  ComposerStrip,
  STRIP_ROW_HEIGHT,
  type StripGoal,
} from "./ComposerStrip";
import {
  queuedMessages,
  queuedOccupant,
  sessionErrorOccupant,
  useMonitoringOccupant,
  useSubagentOccupant,
} from "./use-composer-strip-occupants";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function subagent(overrides: Partial<SubagentView>): SubagentView {
  return {
    id: "s1",
    name: "Subagent",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function card(id: string, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id, seq: 0, turn_id: "t1", subagents };
}

function queuedMsg(queuedId: string, text: string): ChatViewItem {
  return {
    kind: "user_message",
    id: `um-${queuedId}`,
    seq: 1,
    text,
    queued: { queuedId },
  };
}

function errorNotice(message: string): ChatViewItem {
  return {
    kind: "runtime_notice",
    id: "notice-1",
    seq: 2,
    message,
    severity: "error",
  };
}

interface HarnessProps {
  messages?: ChatViewItem[];
  threadId?: string | null;
  streaming?: boolean;
  onJump?: (cardId: string) => void;
  monitoring?: boolean;
  reason?: string | null;
  onStop?: () => void | Promise<void>;
  onEdit?: (queuedId: string, text: string) => void;
}

/** Mirrors AgentChatPane's wiring: every occupant source feeding one strip. */
function Harness({
  messages = [],
  threadId = "t1",
  streaming = true,
  onJump = () => {},
  monitoring = false,
  reason = null,
  onStop = () => {},
  onEdit,
}: HarnessProps) {
  const sub = useSubagentOccupant({ messages, threadId, streaming, onJump });
  const mon = useMonitoringOccupant({ monitoring, reason, threadId, onStop });
  return (
    <ComposerStrip
      occupants={[
        sessionErrorOccupant(messages, streaming),
        mon,
        sub,
        queuedOccupant(queuedMessages(messages), onEdit),
      ]}
    />
  );
}

const rows = () => screen.queryAllByTestId("composer-strip-row");
const orbStates = () =>
  [...document.querySelectorAll("canvas[data-orb-state]")].map((c) =>
    c.getAttribute("data-orb-state"),
  );

describe("ComposerStrip — shell", () => {
  it("renders nothing with no occupants", () => {
    const { container } = render(<Harness />);
    expect(container.firstChild).toBeNull();
  });

  it("mirrors the scope-strip chin, flipped above the pill", () => {
    render(<Harness monitoring />);
    const strip = screen.getByTestId("composer-strip");
    // 20px inset each side: 720px inside the 760px column.
    expect(strip.className).toContain("px-5");
    const shell = strip.firstElementChild as HTMLElement;
    expect(shell.className).toContain("rounded-t-[14px]");
    expect(shell.className).toContain("border-b-0");
    // 18px seam tucked under the pill.
    expect(shell.className).toContain("-mb-[18px]");
    expect(shell.className).toContain("pb-[18px]");
    // Same neutral surface as the scope strip; never tinted per occupant.
    expect(shell.className).toContain("bg-muted/20");
    expect(shell.className).not.toContain("rounded-t-[19px]");
    const row = rows()[0];
    expect(row.className).toContain(STRIP_ROW_HEIGHT);
  });

  it("colours only the mark: an amber, still monitoring dot", () => {
    render(<Harness monitoring />);
    const dot = screen.getByTestId("composer-strip-monitoring-dot");
    expect(dot.className).toContain("bg-warning");
    expect(dot.className).not.toContain("animate");
  });

  it("draws the ember sweep for live work only", () => {
    const { rerender } = render(<Harness monitoring />);
    expect(screen.queryByTestId("composer-strip-sweep")).toBeNull();
    rerender(
      <Harness
        messages={[card("run-1", [subagent({ id: "a" })])]}
      />,
    );
    const sweep = screen.getByTestId("composer-strip-sweep");
    expect(sweep.className).toContain("h-px");
    expect(sweep.className).toContain("via-accent-ember");
  });
});

describe("ComposerStrip — list", () => {
  const busy: ChatViewItem[] = [
    card("run-1", [
      subagent({ id: "a", name: "Explore" }),
      subagent({ id: "b", name: "Implement" }),
    ]),
    queuedMsg("q-1", "and then run the tests"),
  ];

  it("collapsed shows exactly one row — the highest-priority occupant — plus a +n chip", () => {
    render(<Harness messages={busy} monitoring reason="CI on PR #482" />);
    expect(rows()).toHaveLength(1);
    expect(rows()[0]).toHaveAttribute("data-kind", "monitoring");
    expect(rows()[0]).toHaveTextContent("CI on PR #482");
    // 2 subagents + 1 queued message behind the lead.
    expect(screen.getByTestId("composer-strip-toggle")).toHaveTextContent("+3");
  });

  it("orders error > monitoring > running > queued", () => {
    const { rerender } = render(
      <Harness
        messages={[...busy, errorNotice("Session error: sidecar exited")]}
        streaming={false}
        monitoring
      />,
    );
    expect(rows()[0]).toHaveAttribute("data-kind", "error");
    expect(rows()[0]).toHaveTextContent("sidecar exited");
    expect(rows()[0]).not.toHaveTextContent("Session error: sidecar");

    rerender(<Harness messages={busy} />);
    expect(rows()[0]).toHaveAttribute("data-kind", "running");
    expect(rows()[0]).toHaveTextContent("2 subagents running");
  });

  it("opens in place with one row per pending item, each with its own action", () => {
    const onJump = vi.fn();
    const onEdit = vi.fn();
    render(<Harness messages={busy} onJump={onJump} onEdit={onEdit} />);
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));

    expect(screen.getByTestId("composer-strip")).toHaveAttribute("data-open");
    expect(rows().map((r) => r.getAttribute("data-kind"))).toEqual([
      "running",
      "running",
      "queued",
    ]);
    const list = screen.getByRole("list", { name: "Pending activity" });
    expect(list.className).toContain("max-h-[145px]");
    expect(list.className).toContain("overflow-y-auto");
    expect(list.className).toContain("gap-[3px]");

    fireEvent.click(within(rows()[1]).getByRole("button", { name: "Jump" }));
    expect(onJump).toHaveBeenCalledWith("run-1");
    fireEvent.click(within(rows()[2]).getByRole("button", { name: "Edit" }));
    expect(onEdit).toHaveBeenCalledWith("q-1", "and then run the tests");
    // Actions never close the list — only the user's toggle does.
    expect(screen.getByTestId("composer-strip")).toHaveAttribute("data-open");
  });

  it("only the chip or Escape closes it; arrivals and completions leave it open", () => {
    const { rerender } = render(<Harness messages={busy} />);
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));

    rerender(<Harness messages={busy} monitoring />);
    expect(screen.getByTestId("composer-strip")).toHaveAttribute("data-open");
    rerender(<Harness messages={[queuedMsg("q-1", "later")]} />);
    expect(screen.getByTestId("composer-strip")).toHaveAttribute("data-open");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByTestId("composer-strip")).not.toHaveAttribute("data-open");

    rerender(<Harness messages={busy} />);
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    expect(screen.getByTestId("composer-strip")).not.toHaveAttribute("data-open");
  });

  it("closes when the list empties, and stays closed when work returns", () => {
    const { rerender } = render(<Harness messages={busy} />);
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    rerender(<Harness messages={[]} streaming={false} />);
    // The finished flash still occupies the strip for a moment.
    rerender(<Harness messages={[]} threadId="t2" />);
    expect(screen.queryByTestId("composer-strip")).toBeNull();
    rerender(<Harness messages={busy} threadId="t2" />);
    expect(screen.getByTestId("composer-strip")).not.toHaveAttribute("data-open");
  });
});

describe("ComposerStrip — subagents", () => {
  it("stays hidden while the matching transcript work-log row is visible", async () => {
    class VisibleIntersectionObserver {
      constructor(private readonly callback: IntersectionObserverCallback) {}
      observe(target: Element) {
        this.callback(
          [{ isIntersecting: true, target } as IntersectionObserverEntry],
          this as unknown as IntersectionObserver,
        );
      }
      disconnect() {}
      unobserve() {}
      takeRecords() {
        return [];
      }
      readonly root = null;
      readonly rootMargin = "0px";
      readonly thresholds = [0];
    }
    vi.stubGlobal("IntersectionObserver", VisibleIntersectionObserver);
    render(
      <>
        <div data-subagent-card="run-1">
          <span data-subagent-run-id="run-1" />
        </div>
        <Harness messages={[card("run-1", [subagent({ id: "a" })])]} />
      </>,
    );
    await waitFor(() => {
      expect(screen.queryByTestId("composer-strip")).toBeNull();
    });
  });

  it("single running: summary row with the activity kind and a Jump chip", () => {
    const onJump = vi.fn();
    render(
      <Harness
        messages={[card("run-1", [subagent({ id: "a", name: "Verify" })])]}
        onJump={onJump}
      />,
    );
    expect(screen.getByText("1 subagent running")).toBeInTheDocument();
    expect(screen.getByText("working")).toBeInTheDocument();
    expect(screen.queryByTestId("composer-strip-toggle")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Jump" }));
    expect(onJump).toHaveBeenCalledWith("run-1");
  });

  it("flattens running subagents with from-labels when several cards run", () => {
    render(
      <Harness
        messages={[
          card("run-1", [subagent({ id: "a", name: "Explore" })]),
          card("run-2", [subagent({ id: "b", name: "Implement" })]),
        ]}
      />,
    );
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    expect(rows()[0]).toHaveTextContent("Explore");
    expect(rows()[0]).toHaveTextContent("from task 1");
    expect(rows()[1]).toHaveTextContent("Implement");
    expect(rows()[1]).toHaveTextContent("from task 2");
  });

  it("flashes finished for 2.5s after an observed transition, with a Jump", () => {
    vi.useFakeTimers();
    const onJump = vi.fn();
    const running = [card("run-1", [subagent({ id: "a" })])];
    const finished = [card("run-1", [subagent({ id: "a", status: "completed" })])];
    const { rerender } = render(<Harness messages={running} onJump={onJump} />);
    rerender(<Harness messages={finished} onJump={onJump} />);

    expect(rows()[0]).toHaveAttribute("data-kind", "finished");
    expect(screen.getByText("Subagents finished")).toBeInTheDocument();
    expect(orbStates()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Jump" }));
    expect(onJump).toHaveBeenCalledWith("run-1");

    act(() => {
      vi.advanceTimersByTime(2500);
    });
    expect(screen.queryByTestId("composer-strip")).toBeNull();
  });

  it("does not flash on mount or on a switch to an idle thread", () => {
    const idle = [card("run-1", [subagent({ id: "a", status: "completed" })])];
    const { rerender } = render(<Harness messages={idle} />);
    expect(screen.queryByTestId("composer-strip")).toBeNull();

    rerender(
      <Harness messages={[card("run-2", [subagent({ id: "b" })])]} threadId="a" />,
    );
    rerender(<Harness messages={idle} threadId="b" />);
    expect(screen.queryByTestId("composer-strip")).toBeNull();
  });

  it("ignores a background task once the run is over", () => {
    const messages = [
      card("run-1", [subagent({ id: "bg", backgroundTask: true })]),
    ];
    const { rerender } = render(<Harness messages={messages} />);
    expect(screen.getByText("1 subagent running")).toBeInTheDocument();
    rerender(<Harness messages={messages} streaming={false} />);
    expect(screen.queryByText("1 subagent running")).toBeNull();
  });

  it("keeps the summary orb neutral and gives each opened row its own", () => {
    function tool(name: string, input: unknown = {}) {
      return {
        kind: "tool_call" as const,
        id: `tc-${name}`,
        seq: 1,
        tool_use_id: `tu-${name}`,
        tool_name: name,
        input,
        status: "running" as const,
        result_content: null,
        approval_request_id: null,
      };
    }
    render(
      <Harness
        messages={[
          card("run-1", [
            subagent({ id: "a", items: [tool("Grep")] }),
            subagent({
              id: "b",
              items: [tool("Bash", { command: "git push origin HEAD" })],
            }),
          ]),
        ]}
      />,
    );
    expect(orbStates()).toEqual(["working"]);
    fireEvent.click(screen.getByTestId("composer-strip-toggle"));
    expect(orbStates()).toEqual(["searching", "connecting"]);
  });
});

describe("ComposerStrip — monitoring", () => {
  it("stops, parks on Stopping…, and releases when the status leaves monitoring", () => {
    const onStop = vi.fn(() => new Promise<void>(() => {}));
    const { rerender } = render(<Harness monitoring onStop={onStop} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
    const pending = screen.getByRole("button", { name: "Stopping…" });
    expect(pending).toBeDisabled();
    fireEvent.click(pending);
    expect(onStop).toHaveBeenCalledTimes(1);

    rerender(<Harness monitoring={false} onStop={onStop} />);
    expect(screen.queryByTestId("composer-strip")).toBeNull();
    rerender(<Harness monitoring onStop={onStop} />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  it("resets the pending state across a thread switch", () => {
    const onStop = vi.fn(() => new Promise<void>(() => {}));
    const { rerender } = render(<Harness monitoring onStop={onStop} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    rerender(<Harness monitoring threadId="t2" onStop={onStop} />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  it("still stops on a pane with no bound thread", () => {
    const onStop = vi.fn(() => Promise.resolve());
    render(<Harness monitoring threadId={null} onStop={onStop} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("recovers the button when the stop command fails", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const onStop = vi.fn(() => Promise.reject(new Error("no session")));
    render(<Harness monitoring onStop={onStop} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Stop" })).toBeEnabled();
    error.mockRestore();
  });
});

describe("ComposerStrip — session error", () => {
  it("is withheld while streaming and cleared once anything follows the notice", () => {
    const notice = errorNotice("Session error: server unreachable");
    const { rerender } = render(<Harness messages={[notice]} streaming />);
    expect(screen.queryByTestId("composer-strip")).toBeNull();

    rerender(<Harness messages={[notice]} streaming={false} />);
    expect(rows()[0]).toHaveAttribute("data-kind", "error");

    rerender(
      <Harness
        messages={[
          notice,
          { kind: "user_message", id: "um-2", seq: 3, text: "retry" },
        ]}
        streaming={false}
      />,
    );
    expect(screen.queryByTestId("composer-strip")).toBeNull();
  });
});

describe("ComposerStrip — goal", () => {
  function stripGoal(
    overrides: Partial<StripGoal> = {},
    status: "standing" | "interrupted" = "standing",
  ): StripGoal {
    return {
      goal: {
        text: "Migrate the importer to the streaming parser and keep the old path behind a flag",
        setAt: Date.now() - 4 * 60_000,
        sourceMessageId: "user-1",
        status,
      },
      resumePhrase: "/goal resume",
      onResume: vi.fn(),
      onEditResume: vi.fn(),
      onClear: vi.fn(),
      onCopy: vi.fn(),
      onJump: vi.fn(),
      stopped: null,
      ...overrides,
    };
  }

  const goalRow = () => screen.getByTestId("composer-strip-goal");
  const twoQueued = () =>
    queuedOccupant(queuedMessages([queuedMsg("q-1", "a"), queuedMsg("q-2", "b")]));

  it("standing: label, truncated text, age and an up chevron; no Resume", () => {
    render(<ComposerStrip occupants={[]} goal={stripGoal()} />);
    const row = goalRow();
    expect(row).toHaveAttribute("data-status", "standing");
    expect(within(row).getByText("Goal")).toBeInTheDocument();
    expect(within(row).getByText(/Migrate the importer/).className).toContain("truncate");
    expect(screen.getByTestId("composer-strip-goal-age")).toHaveTextContent("4m");
    expect(screen.getByRole("button", { name: "Show goal" })).toBeInTheDocument();
    expect(screen.queryByTestId("composer-strip-goal-resume")).toBeNull();
    expect(screen.queryByTestId("composer-strip-goal-edge")).toBeNull();
    // Resting height is one strip row.
    expect(row.firstElementChild?.className).toContain(STRIP_ROW_HEIGHT);
  });

  it("contention: the goal keeps the row, the rest count into +n, and the drill-in lists them", () => {
    render(<ComposerStrip goal={stripGoal()} occupants={[twoQueued()]} />);
    expect(screen.getByTestId("composer-strip")).toHaveAttribute("data-lead", "goal");
    expect(rows()).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Show goal" })).toBeNull();
    fireEvent.click(screen.getByTestId("composer-strip-goal-more"));
    expect(screen.getByTestId("composer-strip-goal-details")).toBeInTheDocument();
    expect(rows()).toHaveLength(0);
    const others = screen.getByTestId("composer-strip-goal-others");
    expect(others).toHaveTextContent("2 messages queued");
    fireEvent.click(others);
    expect(rows()).toHaveLength(2);
  });

  it("interrupted: amber edge, Resume and the overflow; never folded under another occupant", () => {
    const onResume = vi.fn();
    render(
      <ComposerStrip
        goal={stripGoal({ onResume }, "interrupted")}
        occupants={[sessionErrorOccupant([errorNotice("Session error: gone")], false)]}
      />,
    );
    expect(within(goalRow()).getByText("Goal interrupted")).toBeInTheDocument();
    expect(screen.getByTestId("composer-strip-goal-tint")).toBeInTheDocument();
    expect(screen.getByTestId("composer-strip-goal-edge")).toBeInTheDocument();
    expect(screen.queryByTestId("composer-strip-sweep")).toBeNull();
    expect(screen.queryByTestId("composer-strip-goal-age")).toBeNull();
    expect(screen.getByTestId("composer-strip-goal-menu")).toBeInTheDocument();
    expect(screen.getByTestId("composer-strip-goal-more")).toHaveTextContent("+1");
    expect(rows()).toHaveLength(0);
    fireEvent.click(screen.getByTestId("composer-strip-goal-resume"));
    expect(onResume).toHaveBeenCalledTimes(1);
  });

  it("standing opened: set-time meta, Copy / Clear / Hide, then the text and Jump", () => {
    const goal = stripGoal();
    render(<ComposerStrip occupants={[]} goal={goal} />);
    fireEvent.click(screen.getByTestId("composer-strip-goal-toggle"));
    expect(screen.getByTestId("composer-strip-goal-meta").textContent).toMatch(
      /^set \d{2}:\d{2} · 4m ago$/,
    );
    const details = screen.getByTestId("composer-strip-goal-details");
    expect(details.className).toContain("pl-9");
    expect(details).toHaveTextContent(goal.goal.text);
    expect(screen.queryByTestId("composer-strip-goal-sends")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Jump to message" }));
    expect(goal.onJump).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(goal.onCopy).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(goal.onClear).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByTestId("composer-strip-goal-details")).toBeNull();
  });

  it("interrupted opened: where it stopped and the literal text Resume will send", () => {
    const onJumpToLast = vi.fn();
    const goal = stripGoal(
      {
        stopped: {
          at: Date.now() - 3 * 3_600_000,
          after: "Pinned the renderer scale factor",
          onJump: onJumpToLast,
        },
      },
      "interrupted",
    );
    render(<ComposerStrip occupants={[]} goal={goal} />);
    fireEvent.click(screen.getByTestId("composer-strip-goal-toggle"));
    expect(screen.getByTestId("composer-strip-goal-meta").textContent).toMatch(
      /^stopped \d{2}:\d{2} · idle 3h$/,
    );
    expect(screen.queryByRole("button", { name: "Copy" })).toBeNull();
    const details = screen.getByTestId("composer-strip-goal-details");
    expect(details).toHaveTextContent('stopped after "Pinned the renderer scale factor"');

    const sends = screen.getByTestId("composer-strip-goal-sends");
    expect(sends).toHaveTextContent("/goal resume");
    fireEvent.click(within(sends).getByRole("button", { name: "edit before sending" }));
    expect(goal.onEditResume).toHaveBeenCalledTimes(1);
    expect(goal.onResume).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Jump to last activity" }));
    expect(onJumpToLast).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("composer-strip-goal-details")).toBeNull();
  });

  it("the overflow holds Copy goal text, Jump to message and Clear goal", async () => {
    const goal = stripGoal({}, "interrupted");
    render(<ComposerStrip occupants={[]} goal={goal} />);
    const trigger = screen.getByTestId("composer-strip-goal-menu");
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([
      "Copy goal text",
      "Jump to message",
      "Clear goal",
    ]);
    fireEvent.click(items[2]!);
    expect(goal.onClear).toHaveBeenCalledTimes(1);
  });

  it("never shows a turn count, cap, percentage or verdict", () => {
    for (const status of ["standing", "interrupted"] as const) {
      const { unmount } = render(
        <ComposerStrip occupants={[twoQueued()]} goal={stripGoal({}, status)} />,
      );
      fireEvent.click(screen.getByTestId("composer-strip-goal-toggle"));
      const text = screen.getByTestId("composer-strip").textContent ?? "";
      expect(text).not.toMatch(/\d+\s*\/\s*\d+|%|turn \d|pass|fail|complete/i);
      unmount();
    }
  });
});
