import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AddonNode } from "@/lib/addons/types";
import { ADDON_ICON_NAMES, AddonRenderer } from "./addon-renderer";
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
  it("does not submit a containing form when a plugin Markdown link is activated", () => {
    const submit = vi.fn();
    const link = vi.fn();
    render(
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <AddonRenderer
          nodes={[
            node("markdown", "cmx-markdown", {}, [
              text("[Open](https://example.com)"),
            ]),
          ]}
          event={vi.fn()}
          link={link}
        />
      </form>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(link).toHaveBeenCalledOnce();
    expect(submit).not.toHaveBeenCalled();
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
    expect(screen.getAllByRole("listitem")[0]).toHaveAttribute(
      "aria-setsize",
      "500",
    );
    expect(screen.getAllByRole("listitem")[0]).toHaveAttribute(
      "aria-posinset",
      "1",
    );
    fireEvent.scroll(screen.getByRole("list", { name: "Add-on list" }), {
      target: { scrollTop: 3600 },
    });
    expect(screen.getAllByRole("listitem")[0]).toHaveAttribute(
      "aria-posinset",
      "99",
    );
    expect(screen.getAllByRole("listitem")[0]).toHaveTextContent("98");
  });
  it("exposes complete table dimensions and row positions across a virtual scroll", () => {
    render(
      <AddonRenderer
        nodes={[
          node("table", "cmx-table", {
            headers: ["Path"],
            rows: Array.from({ length: 500 }, (_, i) => [String(i)]),
          }),
        ]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    const table = screen.getByRole("table", { name: "Add-on table" });
    expect(table).toHaveAttribute("aria-rowcount", "501");
    expect(screen.getAllByRole("row")[0]).toHaveAttribute("aria-rowindex", "1");
    expect(screen.getAllByRole("row")[1]).toHaveAttribute("aria-rowindex", "2");
    fireEvent.scroll(table, { target: { scrollTop: 3600 } });
    expect(screen.getAllByRole("row")[1]).toHaveAttribute(
      "aria-rowindex",
      "100",
    );
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("98");
  });
  it("gives a labelled list or table its own accessible name", () => {
    render(
      <AddonRenderer
        nodes={[
          node("list", "cmx-list", { label: "Changed paths", items: ["a"] }),
          node("table", "cmx-table", {
            label: "Open issues",
            headers: ["Title"],
            rows: [["Crash"]],
          }),
        ]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    expect(screen.getByRole("list", { name: "Changed paths" })).toBeTruthy();
    expect(screen.getByRole("table", { name: "Open issues" })).toBeTruthy();
  });
});

describe("trusted adapters use CodeMux controls", () => {
  it("renders Switch as the app's switch, labelled and keyboard operable", async () => {
    const event = vi.fn();
    render(
      <AddonRenderer
        nodes={[
          node("switch", "cmx-switch", { label: "Include paths", checked: false }),
          node("checkbox", "cmx-checkbox", { label: "Remember", checked: true }),
        ]}
        event={event}
        link={vi.fn()}
      />,
    );
    const toggle = screen.getByRole("switch", { name: "Include paths" });
    expect(toggle).toHaveAttribute("data-slot", "switch");
    expect(toggle).toHaveAttribute("aria-checked", "false");
    // A checkbox stays a checkbox; only Switch changed.
    expect(screen.getByRole("checkbox", { name: "Remember" })).toBeChecked();
    toggle.focus();
    await userEvent.keyboard(" ");
    expect(event).toHaveBeenCalledWith(
      expect.objectContaining({ id: "switch" }),
      "change",
      true,
    );
  });
  it("maps Button, Badge and Divider to the shared components", () => {
    const { container } = render(
      <AddonRenderer
        nodes={[
          node("button", "cmx-button", { label: "Refresh" }),
          node("danger", "cmx-button", { label: "Delete", color: "danger" }),
          node("badge", "cmx-badge", {}, [text("3 open")]),
          node("divider", "cmx-divider"),
        ]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    const refresh = screen.getByRole("button", { name: "Refresh" });
    expect(refresh).toHaveAttribute("data-slot", "button");
    expect(refresh).toHaveAttribute("data-variant", "outline");
    expect(screen.getByRole("button", { name: "Delete" })).toHaveAttribute(
      "data-variant",
      "destructive",
    );
    expect(screen.getByText("3 open")).toHaveAttribute("data-slot", "badge");
    expect(container.querySelector('[data-slot="separator"]')).not.toBeNull();
  });
  it("renders every allowlisted icon name with its own glyph", () => {
    // The generated manifest schema carries the validators' icon list
    // (manifest::ICONS); the renderer must know every name on it.
    const schema = JSON.parse(
      readFileSync(
        resolve(process.cwd(), "packages/plugin-sdk/schema/manifest.json"),
        "utf8",
      ),
    ) as {
      definitions: { View: { properties: { icon: { enum: string[] } } } };
    };
    const allowed = schema.definitions.View.properties.icon.enum;
    expect([...ADDON_ICON_NAMES].sort()).toEqual([...allowed].sort());
    const { container } = render(
      <AddonRenderer
        nodes={allowed.map((name) => node(name, "cmx-icon", { name }))}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    const glyphs = [...container.querySelectorAll("svg")].map(
      (svg) => svg.getAttribute("class") ?? "",
    );
    expect(glyphs).toHaveLength(allowed.length);
    allowed.forEach((name, index) =>
      expect(glyphs[index]).toContain(`lucide-${name}`),
    );
  });
  it("shows Markdown links as text when the add-on may not open links", () => {
    render(
      <AddonRenderer
        nodes={[
          node("markdown", "cmx-markdown", {}, [
            text("[Docs](https://example.com)"),
          ]),
        ]}
        event={vi.fn()}
        link={vi.fn()}
        linksAllowed={false}
      />,
    );
    expect(screen.queryByRole("button", { name: "Docs" })).toBeNull();
    expect(screen.getByText("Docs")).toBeTruthy();
  });
  it("opens a Markdown link whatever the case of its https scheme", () => {
    const link = vi.fn();
    render(
      <AddonRenderer
        nodes={[
          node("markdown", "cmx-markdown", {}, [
            text("[Docs](HTTPS://example.com/docs) and [Plain](http://example.com)"),
          ]),
        ]}
        event={vi.fn()}
        link={link}
      />,
    );
    expect(screen.getByRole("button", { name: "Plain" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Docs" }));
    expect(link).toHaveBeenCalledWith(
      expect.objectContaining({ id: "markdown" }),
      "HTTPS://example.com/docs",
    );
  });
});

describe("text fields keep what the user types", () => {
  const field = (properties: Record<string, unknown>) =>
    node("field", "cmx-text-field", { label: "Query", ...properties });
  function setup(properties: Record<string, unknown>, element = "cmx-text-field") {
    const event = vi.fn();
    const view = render(
      <AddonRenderer
        nodes={[{ ...field(properties), element }]}
        event={event}
        link={vi.fn()}
      />,
    );
    const echo = (value: string) =>
      view.rerender(
        <AddonRenderer
          nodes={[{ ...field({ ...properties, value }), element }]}
          event={event}
          link={vi.fn()}
        />,
      );
    return {
      event,
      echo,
      input: screen.getByRole("textbox", { name: "Query" }) as
        | HTMLInputElement
        | HTMLTextAreaElement,
    };
  }
  const sent = (event: ReturnType<typeof vi.fn>) =>
    event.mock.calls.map(([, , value]) => value);
  it("holds typed text before the add-on echoes it", async () => {
    const { input, event } = setup({ value: "" });
    await userEvent.type(input, "abc");
    expect(input.value).toBe("abc");
    expect(sent(event)).toEqual(["a", "ab", "abc"]);
  });
  it("ignores late echoes of earlier keystrokes during a fast burst", async () => {
    const { input, event, echo } = setup({ value: "" });
    await userEvent.type(input, "abcd");
    // The add-on's round trip lags behind: its echoes arrive one by one.
    echo("a");
    expect(input.value).toBe("abcd");
    echo("abc");
    expect(input.value).toBe("abcd");
    await userEvent.type(input, "e");
    echo("abcd");
    expect(input.value).toBe("abcde");
    expect(sent(event)).toEqual(["a", "ab", "abc", "abcd", "abcde"]);
  });
  it("keeps the caret for a mid-string edit and its echo", async () => {
    const { input, echo } = setup({ value: "helo world" });
    input.focus();
    input.setSelectionRange(3, 3);
    await userEvent.keyboard("l");
    expect(input.value).toBe("hello world");
    expect(input.selectionStart).toBe(4);
    echo("hello world");
    expect(input.value).toBe("hello world");
    expect(input.selectionStart).toBe(4);
  });
  it("takes a value the add-on sets itself and keeps the caret there", async () => {
    const { input, echo } = setup({ value: "hello world" });
    input.focus();
    input.setSelectionRange(5, 5);
    echo("HELLO world");
    expect(input.value).toBe("HELLO world");
    expect(input.selectionStart).toBe(5);
    // Clearing after a submit is a value the field never reported.
    await userEvent.type(input, "!");
    echo("");
    expect(input.value).toBe("");
  });
  it("works uncontrolled when the add-on omits value", async () => {
    const { input, event } = setup({}, "cmx-text-area");
    expect(input.tagName).toBe("TEXTAREA");
    await userEvent.type(input, "notes");
    expect(input.value).toBe("notes");
    expect(event).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: "field" }),
      "change",
      "notes",
    );
  });
  it("does not send text over the broker's 32 KiB event limit", () => {
    const { input, event } = setup({ value: "" });
    fireEvent.change(input, { target: { value: "é".repeat(16385) } });
    expect(event).not.toHaveBeenCalled();
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(
      screen.getByText(/too long to send to the add-on/),
    ).toBeTruthy();
    // The message describes the field; it is not part of its name.
    expect(input).toHaveAccessibleName("Query");
    expect(input).toHaveAccessibleDescription(
      "This text is too long to send to the add-on (32 KiB at most).",
    );
    fireEvent.change(input, { target: { value: "short" } });
    expect(sent(event)).toEqual(["short"]);
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(input).not.toHaveAttribute("aria-describedby");
  });
  it("never rolls back to a late echo after a long burst", () => {
    // An add-on busy with other work answers only after dozens of edits.
    const { input, event, echo } = setup({ value: "" });
    const typed = "the quick brown fox jumps over the lazy dog, twice over";
    for (let i = 1; i <= typed.length; i++)
      fireEvent.change(input, { target: { value: typed.slice(0, i) } });
    expect(sent(event)).toHaveLength(typed.length);
    echo("t");
    echo("the quick");
    expect(input.value).toBe(typed);
    echo(typed);
    expect(input.value).toBe(typed);
    // Afterwards a value the add-on sets itself still wins.
    echo("");
    expect(input.value).toBe("");
  });
});
describe("add-on buttons", () => {
  it("honors a full-height button and keeps a long label on one line", () => {
    const label = "Refresh the project brief from the working tree";
    render(
      <AddonRenderer
        nodes={[node("button", "cmx-button", { label, height: "full" })]}
        event={vi.fn()}
        link={vi.fn()}
      />,
    );
    const button = screen.getByRole("button", { name: label });
    expect(button).toHaveClass("h-full", "max-w-full");
    expect(button).toHaveAttribute("title", label);
    expect(button.querySelector(".truncate")).toHaveTextContent(label);
  });
});
