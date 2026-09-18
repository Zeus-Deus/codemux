import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { AddonNode } from "@/lib/addons/types";
import { AddonRenderer } from "./addon-renderer";
afterEach(cleanup);
const node = (
  id: string,
  element: string,
  properties: Record<string, unknown> = {},
  children: AddonNode[] = [],
): AddonNode => ({
  id,
  type: 1,
  element,
  properties,
  children,
  attributes: {},
  eventListeners: {},
});
const text = (value: string): AddonNode => ({
  ...node("text", ""),
  type: 3,
  data: value,
});
describe("trusted add-on rendering", () => {
  it("renders only allowlisted adapters and does not spread child attributes", () => {
    const button = node("button", "cmx-button", {
      label: "Run",
      style: "position:fixed",
      dangerouslySetInnerHTML: { __html: "<script>bad</script>" },
    });
    const { container } = render(
      <AddonRenderer
        nodes={[button, node("script", "script", {}, [text("bad")])]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    expect(container.querySelector("script")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Run" }).getAttribute("style"),
    ).toBeNull();
  });
  it("keeps focused inputs mounted across normalized-tree updates", () => {
    const event = vi.fn();
    const input = node("input", "cmx-text-field", {
      label: "Search",
      value: "a",
    });
    const view = render(
      <AddonRenderer nodes={[input]} event={event} link={vi.fn()} />,
    );
    const field = screen.getByRole("textbox");
    field.focus();
    view.rerender(
      <AddonRenderer
        nodes={[{ ...input, properties: { label: "Search", value: "ab" } }]}
        event={event}
        link={vi.fn()}
      />,
    );
    expect(screen.getByRole("textbox")).toBe(field);
    expect(document.activeElement).toBe(field);
    fireEvent.change(field, { target: { value: "abc" } });
    expect(event).toHaveBeenCalledWith(
      expect.objectContaining({ id: "input" }),
      "change",
      "abc",
    );
  });
  it("suppresses Markdown HTML and images and routes HTTPS links through the broker", () => {
    const link = vi.fn();
    const markdown = node("markdown", "cmx-markdown", {}, [
      text(
        '<img src="https://attacker.test">\n\n![image](https://attacker.test/a)\n[Open](https://example.com)',
      ),
    ]);
    const { container } = render(
      <AddonRenderer nodes={[markdown]} event={vi.fn()} link={link} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(link).toHaveBeenCalledWith(markdown, "https://example.com");
  });
  it("moves tab focus with arrow keys and emits only the selected value", () => {
    const event = vi.fn();
    render(
      <AddonRenderer
        nodes={[
          node("tabs", "cmx-tabs", {
            label: "Sections",
            value: "one",
            options: [
              { label: "One", value: "one" },
              { label: "Two", value: "two" },
            ],
          }),
        ]}
        event={event}
        link={vi.fn()}
      />,
    );
    const first = screen.getByRole("tab", { name: "One" });
    const second = screen.getByRole("tab", { name: "Two" });
    expect(first.tabIndex).toBe(0);
    expect(second.tabIndex).toBe(-1);
    first.focus();
    fireEvent.keyDown(first, { key: "ArrowRight" });
    expect(document.activeElement).toBe(second);
    expect(event).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: "tabs" }),
      "change",
      "two",
    );
    fireEvent.keyDown(second, { key: "Home" });
    expect(document.activeElement).toBe(first);
  });
  it("bounds mounted list rows even for the maximum 500-row input", () => {
    render(
      <AddonRenderer
        nodes={[
          node("list", "cmx-list", {
            items: Array.from({ length: 500 }, (_, i) => String(i)),
          }),
        ]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    expect(screen.getAllByRole("listitem").length).toBeLessThanOrEqual(14);
  });
});
