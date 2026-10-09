/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { DELEGATION_RESULTS_PREFIX } from "@/lib/agent-chat/delegation";

import { DelegationResultsDivider } from "./DelegationResultsDivider";

vi.mock("@/assets/preset-icons/claude.svg", () => ({ default: "/mock/claude.svg" }));
vi.mock("@/assets/preset-icons/codex.svg", () => ({ default: "/mock/codex.svg" }));

afterEach(() => {
  cleanup();
});

function wake(...blocks: string[]): string {
  return [
    DELEGATION_RESULTS_PREFIX,
    "Codemux posted this message, not the user.",
    "",
    ...blocks,
    "",
    "No delegated tasks are still running.",
  ].join("\n");
}

const CODEX_DONE =
  '<task provider="codex" model="gpt-6.1-codex" status="completed" title="Add slugify helper" duration="6m 12s" thread="c1">\n<report>\nAdded slugify.\n</report>\n</task>';
const CLAUDE_DONE =
  '<task provider="claude" status="completed" title="Review the parser" duration="1m 02s" thread="c2">\n<report>\nNo bugs.\n</report>\n</task>';
const CLAUDE_FAILED =
  '<task provider="claude" status="failed" title="Review the parser" duration="9s" thread="c2">\n<report>Claude is not signed in.</report>\n</task>';

describe("DelegationResultsDivider", () => {
  it("collapses to one line naming the providers and the outcome", () => {
    const { container } = render(
      <DelegationResultsDivider text={wake(CODEX_DONE, CLAUDE_DONE)} />,
    );
    const toggle = screen.getByRole("button");
    expect(toggle).toHaveTextContent("Delegated results · Codex, Claude · completed");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("delegation-results-text")).toBeNull();
    // Decorative marks: the label already names the providers.
    expect(
      [...container.querySelectorAll("img[data-provider]")].map((img) =>
        img.getAttribute("data-provider"),
      ),
    ).toEqual(["codex", "claude"]);
    expect(screen.getByTestId("delegation-results-outcome").className).not.toContain(
      "text-status-attention",
    );
  });

  it("marks the outcome red when any task failed", () => {
    render(<DelegationResultsDivider text={wake(CODEX_DONE, CLAUDE_FAILED)} />);
    const outcome = screen.getByTestId("delegation-results-outcome");
    expect(outcome).toHaveTextContent("1 completed · 1 failed");
    expect(outcome.className).toContain("text-status-attention");
  });

  it("expands to exactly the text the model received, in a bounded block", () => {
    const text = wake(CODEX_DONE);
    render(<DelegationResultsDivider text={text} />);
    fireEvent.click(screen.getByRole("button"));
    const block = screen.getByTestId("delegation-results-text");
    expect(block.textContent).toBe(text);
    expect(block.className).toContain("max-h-[320px]");
    expect(block.className).toContain("thin-scrollbar");
    expect(block.className).toContain("text-left");
    expect(screen.getByRole("button")).toHaveAttribute("aria-expanded", "true");
  });

  it("still reads as results when no task block parses", () => {
    render(<DelegationResultsDivider text={`${DELEGATION_RESULTS_PREFIX}\nsomething else`} />);
    expect(screen.getByRole("button")).toHaveTextContent(/^Delegated results$/);
  });
});
