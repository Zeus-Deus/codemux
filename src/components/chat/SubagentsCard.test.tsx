/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import {
  groupRollupLabel,
  subagentPreview,
  SubagentWorkLogRow,
} from "./SubagentsCard";

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

function run(id: string, subagents: SubagentView[]): SubagentRunItem {
  return {
    kind: "subagent_run",
    id,
    seq: Number(id.replace(/\D/g, "")) || 0,
    turn_id: `turn-${id}`,
    subagents,
  };
}

beforeEach(() => useUIStore.setState({ rightPanelTabs: {} }));
afterEach(cleanup);

describe("SubagentWorkLogRow — one line per stretch", () => {
  it("merges canonical runs into one live row with a concise preview", () => {
    const { container } = render(
      <SubagentWorkLogRow
        workspaceId="ws-1"
        runs={[
          run("run-1", [
            subagent({ id: "a", name: "verification pass", status: "completed" }),
            subagent({ id: "b", name: "pricing audit", status: "running" }),
          ]),
          run("run-2", [
            subagent({ id: "c", name: "ccusage cross-check", status: "pending" }),
          ]),
        ]}
      />,
    );

    expect(screen.getByText("work log")).toBeInTheDocument();
    expect(screen.getByText("Ran 3 subagents")).toBeInTheDocument();
    expect(
      screen.getByText(
        "verification pass · pricing audit · ccusage cross-check",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("1 done · 2 active")).toBeInTheDocument();
    expect(container.querySelectorAll("[data-subagent-run-id]")).toHaveLength(2);
    expect(container.querySelector("[data-subagent-card='run-1']")).not.toBeNull();
    expect(container.querySelectorAll("canvas[data-orb-state]")).toHaveLength(1);
  });

  it("opens and marks the Subagents panel from View", () => {
    render(
      <SubagentWorkLogRow
        workspaceId="ws-1"
        runs={[run("run-1", [subagent({ id: "a" })])]}
      />,
    );
    const row = screen.getByRole("button", { name: "View 1 subagent" });
    expect(row).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(row);
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("subagents");
    expect(row).toHaveAttribute("aria-pressed", "true");
  });

  it("settles to one line with tools, elapsed, tokens, and a success mark", () => {
    const { container } = render(
      <SubagentWorkLogRow
        workspaceId="ws-1"
        runs={[
          run("run-1", [
            subagent({
              id: "a",
              status: "completed",
              durationMs: 74_000,
              toolUseCount: 6,
              totalTokens: 20_000,
            }),
            subagent({
              id: "b",
              status: "completed",
              durationMs: 41_000,
              toolUseCount: 3,
              totalTokens: 18_800,
            }),
          ]),
        ]}
      />,
    );
    expect(screen.getByText("work log · settled")).toBeInTheDocument();
    expect(screen.getByText("9 tools · 1m 14s")).toBeInTheDocument();
    expect(screen.getByText("Σ 38.8K")).toBeInTheDocument();
    expect(container.querySelector(".text-status-open")).not.toBeNull();
    expect(container.querySelector("canvas[data-orb-state]")).toBeNull();
  });

  it("uses attention for a failed stretch", () => {
    const { container } = render(
      <SubagentWorkLogRow
        runs={[run("run-1", [subagent({ status: "failed" })])]}
      />,
    );
    expect(container.querySelector(".text-status-attention")).not.toBeNull();
  });
});

describe("work-log helpers", () => {
  it("deduplicates preview labels and caps them at three", () => {
    expect(
      subagentPreview([
        subagent({ id: "a", name: "audit" }),
        subagent({ id: "b", name: "audit" }),
        subagent({ id: "c", name: "verify" }),
        subagent({ id: "d", name: "ship" }),
        subagent({ id: "e", name: "ignored" }),
      ]),
    ).toBe("audit · verify · ship");
  });

  it("keeps the complete real-data rollup available", () => {
    expect(
      groupRollupLabel(
        [
          subagent({
            status: "completed",
            durationMs: 74_000,
            toolUseCount: 9,
            totalTokens: 38_800,
          }),
        ],
        0,
      ),
    ).toBe("1m 14s · Σ 38.8K · 9 tools");
  });
});
