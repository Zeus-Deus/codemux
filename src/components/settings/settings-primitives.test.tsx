import { afterEach, describe, it, expect, vi } from "vitest";
import { useState } from "react";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHeader, SegmentedControl, SettingRow } from "./settings-primitives";

afterEach(cleanup);

describe("SettingRow", () => {
  it("names a switch with its label and describes it with its description", () => {
    render(
      <SettingRow label="Notification sounds" description="Play a sound when an agent finishes.">
        <Switch />
      </SettingRow>,
    );
    const toggle = screen.getByRole("switch", { name: "Notification sounds" });
    expect(toggle).toHaveAccessibleDescription("Play a sound when an agent finishes.");
  });

  it("toggles the switch when the label is clicked", () => {
    const onCheckedChange = vi.fn();
    render(
      <SettingRow label="Wrap code">
        <Switch onCheckedChange={onCheckedChange} />
      </SettingRow>,
    );
    fireEvent.click(screen.getByText("Wrap code"));
    expect(onCheckedChange).toHaveBeenCalledWith(true);
  });

  it("names an input and a select trigger", () => {
    render(
      <>
        <SettingRow label="Default base branch">
          <Input defaultValue="main" />
        </SettingRow>
        <SettingRow label="Cursor style">
          <Select value="bar">
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="bar">Bar</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
      </>,
    );
    expect(screen.getByRole("textbox", { name: "Default base branch" })).toHaveValue("main");
    expect(screen.getByRole("combobox", { name: "Cursor style" })).toBeInTheDocument();
  });

  it("keeps a control's own id and name, and labels the row as a group instead", () => {
    render(
      <SettingRow label="Font" description="Lives in Appearance.">
        <button type="button">Open Appearance</button>
      </SettingRow>,
    );
    expect(screen.getByRole("button", { name: "Open Appearance" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Font" })).toHaveAccessibleDescription(
      "Lives in Appearance.",
    );
  });

  it("marks a device-only setting for sight and for assistive tech", () => {
    render(
      <SettingRow label="Density" description="Spacing." scope="device">
        <Switch />
      </SettingRow>,
    );
    expect(screen.getByRole("switch", { name: "Density" })).toHaveAccessibleDescription(
      "Spacing. Only on this device",
    );
  });
});

describe("SectionHeader", () => {
  it("renders the page title on the shared title token with an action slot", () => {
    render(
      <SectionHeader
        title="Keyboard Shortcuts"
        description="Click a shortcut to rebind it."
        action={<button type="button">Reset all</button>}
      />,
    );
    const title = screen.getByRole("heading", { level: 2, name: "Keyboard Shortcuts" });
    expect(title).toHaveClass("text-title");
    expect(screen.getByRole("button", { name: "Reset all" })).toBeInTheDocument();
  });
});

describe("SegmentedControl", () => {
  function Density() {
    const [value, setValue] = useState<"a" | "b" | "c">("b");
    return (
      <SegmentedControl
        ariaLabel="Density"
        value={value}
        onChange={setValue}
        options={[
          { value: "a", label: "Alpha" },
          { value: "b", label: "Beta" },
          { value: "c", label: "Gamma" },
        ]}
      />
    );
  }

  it("is one tab stop: only the checked option is tabbable", () => {
    render(<Density />);
    const tabbable = screen
      .getAllByRole("radio")
      .filter((radio) => radio.getAttribute("tabindex") === "0");
    expect(tabbable).toHaveLength(1);
    expect(tabbable[0]).toHaveAccessibleName("Beta");
  });

  it("moves the selection and focus with the arrow keys, wrapping at the ends", () => {
    render(<Density />);
    const beta = screen.getByRole("radio", { name: "Beta" });

    fireEvent.keyDown(beta, { key: "ArrowRight" });
    const gamma = screen.getByRole("radio", { name: "Gamma" });
    expect(gamma).toHaveAttribute("aria-checked", "true");
    expect(gamma).toHaveFocus();

    fireEvent.keyDown(gamma, { key: "ArrowRight" });
    expect(screen.getByRole("radio", { name: "Alpha" })).toHaveAttribute("aria-checked", "true");

    fireEvent.keyDown(screen.getByRole("radio", { name: "Alpha" }), { key: "ArrowLeft" });
    expect(gamma).toHaveAttribute("aria-checked", "true");
    expect(gamma).toHaveAttribute("tabindex", "0");
  });
});
