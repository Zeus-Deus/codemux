/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { Bug, ListTodo } from "lucide-react";

import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { SlashCommandPopup } from "./SlashCommandPopup";
import { buildProviderCommands } from "@/lib/agent-chat/slash-commands";

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
  it("shows the definition provider and skill type, even when used in another provider", () => {
    const item: SlashCommandItem = { id: "skill:review", label: "review", command: "$review", group: "SKILLS", onSelect: vi.fn(), identity: { provider: "claude", kind: "skill", label: "Claude skill · project · Portable in Codex" } };
    render(<SlashCommandPopup items={[item]} highlightedId={item.id} onHighlightChange={vi.fn()} onSelect={vi.fn()} open />);
    const source = screen.getByRole("img", { name: item.identity!.label });
    expect(source.querySelector('[data-provider="claude"]')).not.toBeNull();
    expect(source.querySelector('[data-provider="codex"]')).toBeNull();
    expect(source).toHaveAttribute("data-command-kind", "skill");
    expect(source.children).toHaveLength(1);
    expect(screen.getByTestId("slash-item-source")).toHaveTextContent("Portable in Codex");
  });

  it.each(["codex", "claude", "opencode", "cursor", "grok", "hermes"] as const)("identifies %s native commands with one provider mark and a textual command label", (provider) => {
    const items = buildProviderCommands({ provider, commands: [{ name: "native-action", description: "Provider action", argumentHint: "" }], reservedNames: new Set() });
    render(<SlashCommandPopup items={items} highlightedId={items[0].id} onHighlightChange={vi.fn()} onSelect={vi.fn()} open />);
    const source = screen.getByRole("img", { name: items[0].identity!.label });
    expect(source.querySelector(`[data-provider="${provider}"]`)).not.toBeNull();
    expect(source).toHaveAttribute("data-command-kind", "command");
    expect(source.children).toHaveLength(1);
  });
  it("does not rebuild a large skill catalogue for each highlight change", () => {
    const icon = vi.fn(() => <svg aria-hidden />);
    const items = Array.from({ length: 250 }, (_, index): SlashCommandItem => ({
      id: `skill:${index}`, label: `Skill ${index}`, command: `/skill-${index}`,
      group: "SKILLS", icon: icon as unknown as SlashCommandItem["icon"], onSelect: vi.fn(),
    }));
    const props = { items, onHighlightChange: vi.fn(), onSelect: vi.fn(), open: true };
    const { rerender } = render(<SlashCommandPopup {...props} highlightedId={items[0].id} />);
    expect(icon).toHaveBeenCalledTimes(250);
    icon.mockClear();
    for (let index = 1; index <= 10; index++) {
      rerender(<SlashCommandPopup {...props} onSelect={vi.fn()} highlightedId={items[index].id} />);
      expect(screen.getByTestId(`slash-item-${items[index].id}`)).toHaveAttribute("data-selected", "true");
    }
    expect(icon).not.toHaveBeenCalled();
  });

  it("keeps keyboard scrolling inside the menu viewport", () => {
    const props = { items: makeItems(), onHighlightChange: vi.fn(), onSelect: vi.fn(), open: true };
    const { container, rerender } = render(<SlashCommandPopup {...props} highlightedId="mode:plan" />);
    const viewport = container.querySelector<HTMLElement>("[data-slot=scroll-area-viewport]")!;
    const row = screen.getByTestId("slash-item-mode:debug");
    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({ top: 100, bottom: 200 } as DOMRect);
    const geometry = vi.spyOn(row, "getBoundingClientRect").mockReturnValue({ top: 190, bottom: 220 } as DOMRect);
    const ancestorScroll = vi.spyOn(row, "scrollIntoView");
    rerender(<SlashCommandPopup {...props} highlightedId="mode:debug" />);
    expect(viewport.scrollTop).toBe(20);
    expect(ancestorScroll).not.toHaveBeenCalled();
    vi.spyOn(screen.getByTestId("slash-item-mode:plan"), "getBoundingClientRect").mockReturnValue({ top: 90, bottom: 120 } as DOMRect);
    geometry.mockReturnValue({ top: 110, bottom: 140 } as DOMRect);
    rerender(<SlashCommandPopup {...props} highlightedId="mode:plan" />);
    expect(viewport.scrollTop).toBe(10);
    rerender(<SlashCommandPopup {...props} highlightedId="mode:debug" />);
    expect(viewport.scrollTop).toBe(10);
  });

  it("does not scroll on hover or highlight rows moving beneath a stationary pointer", () => {
    const onHighlightChange = vi.fn();
    const props = { items: makeItems(), onHighlightChange, onSelect: vi.fn(), open: true };
    const { container, rerender } = render(<SlashCommandPopup {...props} highlightedId="mode:plan" />);
    const viewport = container.querySelector<HTMLElement>("[data-slot=scroll-area-viewport]")!;
    viewport.scrollTop = 80;
    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({ top: 100, bottom: 200 } as DOMRect);
    const row = screen.getByTestId("slash-item-mode:debug");
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue({ top: 190, bottom: 220 } as DOMRect);
    fireEvent.pointerMove(row, { clientX: 100, clientY: 190 });
    expect(onHighlightChange).toHaveBeenLastCalledWith("mode:debug", "pointer");
    rerender(<SlashCommandPopup {...props} highlightedId="mode:debug" />);
    expect(viewport.scrollTop).toBe(80);
    onHighlightChange.mockClear();
    fireEvent.wheel(viewport, { deltaY: 100 });
    fireEvent.pointerMove(screen.getByTestId("slash-item-skill:codemux-ui"), { clientX: 100, clientY: 190 });
    expect(onHighlightChange).not.toHaveBeenCalled();
    expect(viewport.scrollTop).toBe(80);
    fireEvent.pointerMove(screen.getByTestId("slash-item-skill:codemux-ui"), { clientX: 101, clientY: 190 });
    expect(onHighlightChange).toHaveBeenLastCalledWith("skill:codemux-ui", "pointer");
  });

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

  it("shows a provider argument hint after the label without repeating it", () => {
    render(
      <SlashCommandPopup
        items={[
          {
            id: "provider-command:review",
            label: "review",
            description: "Review a pull request",
            command: "/review",
            argumentHint: "<pr-url>",
            group: "COMMANDS",
            onSelect: vi.fn(),
          },
          {
            id: "provider-command:fix",
            label: "fix",
            description: "/fix <issue>",
            command: "/fix",
            argumentHint: "<issue>",
            group: "COMMANDS",
            onSelect: vi.fn(),
          },
        ]}
        highlightedId={null}
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    expect(screen.getByText("<pr-url>")).toBeInTheDocument();
    // The description already carries the hint as its fallback.
    expect(screen.queryByText("<issue>")).toBeNull();
    expect(screen.getByText("/fix <issue>")).toBeInTheDocument();
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

  it("overrides Radix's shrink-to-fit viewport wrapper so rows truncate", () => {
    // Radix renders the ScrollArea viewport's children inside a
    // `display: table; min-width: 100%` div. A table box sizes to its
    // content, so a row wider than the popup (long chat title + its
    // provider/timestamp adornment) stretched the wrapper and got
    // clipped by `overflow-hidden` instead of truncating. These
    // overrides are the fix — losing them silently brings the clipping
    // back, which is easy to miss in a screenshot of short rows.
    const { container } = render(
      <SlashCommandPopup
        items={makeItems()}
        highlightedId="mode:plan"
        onHighlightChange={vi.fn()}
        onSelect={vi.fn()}
        open
      />,
    );
    const scrollArea = container.querySelector("[data-slot=scroll-area]");
    expect(scrollArea?.className).toContain(
      "[&>[data-slot=scroll-area-viewport]>div]:!block",
    );
    expect(scrollArea?.className).toContain(
      "[&>[data-slot=scroll-area-viewport]>div]:!w-full",
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
