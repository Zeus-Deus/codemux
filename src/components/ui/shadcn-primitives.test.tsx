/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { Checkbox } from "./checkbox";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader } from "./code-block";
import { KbdKeys, splitKeyCombo } from "./kbd";
import { NativeSelect } from "./native-select";

afterEach(() => cleanup());

describe("splitKeyCombo / KbdKeys", () => {
  it("splits a combo into one cap per key", () => {
    expect(splitKeyCombo("Ctrl+Shift+P")).toEqual(["Ctrl", "Shift", "P"]);
    expect(splitKeyCombo("Escape")).toEqual(["Escape"]);
    expect(splitKeyCombo("+")).toEqual(["+"]);
  });

  it("keeps a plus key at the end of a combo", () => {
    expect(splitKeyCombo("Ctrl++")).toEqual(["Ctrl", "+"]);
  });

  it("draws separate caps but keeps the combo readable as text", () => {
    const { container } = render(<KbdKeys keys="Ctrl+Shift+P" />);
    expect(container.querySelectorAll("[data-slot=kbd]")).toHaveLength(3);
    expect(container).toHaveTextContent("Ctrl+Shift+P");
  });
});

describe("Checkbox", () => {
  it("reports its checked state through onCheckedChange and its label", () => {
    const onCheckedChange = vi.fn();
    render(
      <label>
        <Checkbox onCheckedChange={onCheckedChange} />
        Keep data
      </label>,
    );
    const box = screen.getByRole("checkbox", { name: "Keep data" });
    expect(box).not.toBeChecked();
    fireEvent.click(box);
    expect(onCheckedChange).toHaveBeenCalledWith(true);
  });
});

describe("NativeSelect", () => {
  it("puts every prop but className on the select itself", () => {
    const onChange = vi.fn();
    render(
      <NativeSelect aria-label="Host" data-testid="host" className="w-auto" value="a" onChange={onChange}>
        <option value="a">A</option>
        <option value="b">B</option>
      </NativeSelect>,
    );
    const select = screen.getByRole("combobox", { name: "Host" });
    expect(select).toBe(screen.getByTestId("host"));
    expect(select.parentElement).toHaveClass("w-auto");
    fireEvent.change(select, { target: { value: "b" } });
    expect(onChange).toHaveBeenCalled();
  });
});

describe("CodeBlock", () => {
  it("copies its value and announces it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(
      <CodeBlock>
        <CodeBlockHeader>
          <CodeBlockCopyButton value="npm test" />
        </CodeBlockHeader>
        <CodeBlockContent>npm test</CodeBlockContent>
      </CodeBlock>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("npm test");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });
});
