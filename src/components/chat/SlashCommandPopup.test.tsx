/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { Bug, ListTodo } from "lucide-react";

import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { SlashCommandPopup } from "./SlashCommandPopup";

afterEach(() => cleanup());

function makeItems(): SlashCommandItem[] {
  return [
    {
      id: "mode:plan",
      label: "Plan",
      description: "Plan and design before coding",
      command: "/plan",
      group: "MODES",
      icon: ListTodo,
      onSelect: vi.fn(),
    },
    {
      id: "mode:debug",
      label: "Debug",
      description: "Add diagnostic logs",
      command: "/debug",
      group: "MODES",
      icon: Bug,
      onSelect: vi.fn(),
    },
    {
      id: "skill:codemux-ui",
      label: "Codemux UI",
      description: "Visual + UI work",
      command: "/skill codemux-ui",
      group: "SKILLS",
      onSelect: vi.fn(),
    },
  ];
}

describe("SlashCommandPopup", () => {
  it("renders nothing when open=false", () => {
    const { container } = render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId={null}
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open={false}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders items grouped by `group` field with section headings", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByText("MODES")).toBeInTheDocument();
    expect(screen.getByText("SKILLS")).toBeInTheDocument();
    expect(screen.getByText("Plan")).toBeInTheDocument();
    expect(screen.getByText("Debug")).toBeInTheDocument();
    expect(screen.getByText("Codemux UI")).toBeInTheDocument();
  });

  it("shows the command hint right-aligned (e.g. /plan)", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByText("/plan")).toBeInTheDocument();
    expect(screen.getByText("/debug")).toBeInTheDocument();
  });

  it("renders the empty state when items is empty", () => {
    render(
      <SlashCommandPopup
        items={[]}
        highlightedId={null}
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByText(/No commands match/i)).toBeInTheDocument();
  });

  it("calls onSelect when an item is clicked", () => {
    const onSelect = vi.fn();
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={onSelect}
        open
      />,
    );
    fireEvent.click(screen.getByTestId("slash-item-mode:plan"));
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ id: "mode:plan" }),
    );
  });

  it("reflects the controlled highlightedId via cmdk's data-selected attribute", () => {
    const { rerender } = render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByTestId("slash-item-mode:plan")).toHaveAttribute(
      "data-selected",
      "true",
    );
    expect(screen.getByTestId("slash-item-mode:debug")).not.toHaveAttribute(
      "data-selected",
      "true",
    );

    rerender(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:debug"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByTestId("slash-item-mode:debug")).toHaveAttribute(
      "data-selected",
      "true",
    );
  });

  it("renders footerNote in muted tone for loading state", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
        footerNote={{ tone: "muted", message: "Loading skills…" }}
      />,
    );
    const footer = screen.getByTestId("slash-popup-footer");
    expect(footer).toHaveAttribute("data-tone", "muted");
    expect(footer).toHaveTextContent("Loading skills…");
  });

  it("renders footerNote in error tone with destructive color class", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
        footerNote={{ tone: "error", message: "Skills: scan failed" }}
      />,
    );
    const footer = screen.getByTestId("slash-popup-footer");
    expect(footer).toHaveAttribute("data-tone", "error");
    expect(footer).toHaveTextContent("Skills: scan failed");
    expect(footer.className).toContain("text-destructive");
  });

  it("renders footerNote alongside the empty-state message when items are empty", () => {
    // Empty filter and footer are orthogonal: the empty state describes
    // the filter, the footer describes the skill-loading pipeline. Both
    // render so the user knows why nothing matched AND what's happening
    // with skills.
    render(
      <SlashCommandPopup
        items={[]}
        highlightedId={null}
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
        footerNote={{ tone: "muted", message: "Loading skills…" }}
      />,
    );
    expect(screen.getByText(/No commands match/i)).toBeInTheDocument();
    expect(screen.getByTestId("slash-popup-footer")).toHaveTextContent(
      "Loading skills…",
    );
  });

  it("omits footerNote node entirely when prop is null", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
        footerNote={null}
      />,
    );
    expect(screen.queryByTestId("slash-popup-footer")).not.toBeInTheDocument();
  });

  it("preserves group insertion order (MODES before SKILLS)", () => {
    render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    const popup = screen.getByTestId("slash-command-popup");
    const headings = Array.from(popup.querySelectorAll("[cmdk-group-heading]"))
      .map((el) => el.textContent);
    expect(headings).toEqual(["MODES", "SKILLS"]);
  });
});
