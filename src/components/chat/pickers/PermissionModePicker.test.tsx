/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { PermissionModeOption } from "@/tauri/types";
import { PermissionModePicker } from "./PermissionModePicker";

afterEach(() => cleanup());

const CLAUDE_MODES: PermissionModeOption[] = [
  {
    value: "default",
    label: "Supervised",
    description: "Ask before commands and file changes.",
    is_default: false,
  },
  {
    value: "acceptEdits",
    label: "Auto-accept edits",
    description: "Auto-approve edits, ask before other actions.",
    is_default: false,
  },
  {
    value: "bypassPermissions",
    label: "Full access",
    description: "Allow commands and edits without prompts.",
    is_default: true,
  },
];

const CODEX_MODES: PermissionModeOption[] = [
  {
    value: "read-only",
    label: "Read only",
    description: "Allow reads, block writes and commands.",
    is_default: false,
  },
  {
    value: "workspace-write",
    label: "Workspace write",
    description: "Allow edits within the workspace, block commands.",
    is_default: false,
  },
  {
    value: "danger-full-access",
    label: "Full access",
    description: "Allow commands and edits without prompts.",
    is_default: true,
  },
];

function renderPicker(
  overrides: Partial<Parameters<typeof PermissionModePicker>[0]> = {},
) {
  const onChange = vi.fn();
  const utils = render(
    <TooltipProvider>
      <PermissionModePicker
        modes={CLAUDE_MODES}
        value={null}
        onChange={onChange}
        {...overrides}
      />
    </TooltipProvider>,
  );
  const trigger = utils.container.querySelector("button") as HTMLElement | null;
  return { ...utils, trigger, onChange };
}

describe("PermissionModePicker — render", () => {
  it("hides when modes are null (capabilities unavailable)", () => {
    const { container } = renderPicker({ modes: null });
    expect(container.querySelector("button")).toBeNull();
  });

  it("hides when modes is an empty array (provider has no permission concept)", () => {
    const { container } = renderPicker({ modes: [] });
    expect(container.querySelector("button")).toBeNull();
  });

  it("Claude modes: trigger shows 'Full access' (the default)", () => {
    const { trigger } = renderPicker({ modes: CLAUDE_MODES });
    expect(trigger!.textContent).toContain("Full access");
  });

  it("Codex modes: trigger shows 'Full access' (Codex default)", () => {
    const { trigger } = renderPicker({ modes: CODEX_MODES });
    expect(trigger!.textContent).toContain("Full access");
  });

  it("honors a supported value", () => {
    const { trigger } = renderPicker({
      modes: CLAUDE_MODES,
      value: "acceptEdits",
    });
    expect(trigger!.textContent).toContain("Auto-accept edits");
  });

  it("falls back to the default when value is unknown", () => {
    const { trigger } = renderPicker({
      modes: CLAUDE_MODES,
      value: "not-a-mode",
    });
    expect(trigger!.textContent).toContain("Full access");
  });

  it("does NOT render a search input (CommandInput removed)", () => {
    const { trigger, container } = renderPicker({ modes: CLAUDE_MODES });
    expect(
      container.querySelector("input[placeholder*='Search']"),
    ).toBeNull();
    expect(trigger).not.toBeNull();
  });
});

describe("PermissionModePicker — interaction", () => {
  it("clicking a row calls onChange with the mode's value", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker({ modes: CLAUDE_MODES });
    await user.click(trigger!);
    await user.click(await screen.findByText("Supervised"));
    expect(onChange).toHaveBeenCalledWith("default");
  });

  it("lists every mode in the menu", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker({ modes: CLAUDE_MODES });
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    expect(options.length).toBe(CLAUDE_MODES.length);
    const labels = options.map((el) => el.textContent ?? "");
    for (const m of CLAUDE_MODES) {
      expect(labels.some((t) => t.includes(m.label))).toBe(true);
    }
  });

  it("Codex fixture: picking Read only returns the Codex value", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker({ modes: CODEX_MODES });
    await user.click(trigger!);
    await user.click(await screen.findByText("Read only"));
    expect(onChange).toHaveBeenCalledWith("read-only");
  });

  it("arrow keys navigate + Enter selects after popover open", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker({ modes: CLAUDE_MODES });
    await user.click(trigger!);
    await screen.findAllByRole("option");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledTimes(1);
    const picked = onChange.mock.calls[0][0];
    expect(
      ["default", "acceptEdits", "bypassPermissions"].includes(picked as string),
    ).toBe(true);
  });
});
