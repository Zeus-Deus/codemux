/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  type RenderResult,
} from "@testing-library/react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { Skill } from "@/tauri/commands";

import { SkillRow } from "./skill-row";

afterEach(() => cleanup());

function makeSkill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: "id",
    name: "demo-skill",
    description: "Does demo things.",
    provider: "claude",
    scope: "user",
    skillDir: "/skills/demo",
    filePath: "/skills/demo/SKILL.md",
    body: "",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  };
}

interface RenderOpts {
  skill?: Skill;
  enabled?: boolean;
  onToggleEnabled?: () => void;
  onView?: () => void;
  onOpenFile?: () => void;
}

function renderRow(opts: RenderOpts = {}): RenderResult {
  return render(
    <TooltipProvider>
      <SkillRow
        skill={opts.skill ?? makeSkill()}
        enabled={opts.enabled ?? true}
        onToggleEnabled={opts.onToggleEnabled ?? vi.fn()}
        onView={opts.onView ?? vi.fn()}
        onOpenFile={opts.onOpenFile ?? vi.fn()}
      />
    </TooltipProvider>,
  );
}

describe("SkillRow", () => {
  it("renders the skill name and description", () => {
    renderRow();
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.getByText("Does demo things.")).toBeInTheDocument();
  });

  it("hides the description block when none is set", () => {
    renderRow({ skill: makeSkill({ description: null }) });
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.queryByText("Does demo things.")).not.toBeInTheDocument();
  });

  it("does NOT render the compatibility badge when compatibility is 'compatible'", () => {
    renderRow();
    expect(screen.queryByTestId("compatibility-badge")).not.toBeInTheDocument();
  });

  it("renders soft-warn badge when compatibility is 'soft-warn'", () => {
    renderRow({ skill: makeSkill({ compatibility: "soft-warn" }) });
    expect(screen.getByTestId("compatibility-badge")).toHaveAttribute(
      "data-level",
      "soft-warn",
    );
  });

  it("renders hard-warn badge when compatibility is 'hard-warn'", () => {
    renderRow({ skill: makeSkill({ compatibility: "hard-warn" }) });
    expect(screen.getByTestId("compatibility-badge")).toHaveAttribute(
      "data-level",
      "hard-warn",
    );
  });

  it("View button calls onView", () => {
    const onView = vi.fn();
    renderRow({ onView });
    fireEvent.click(screen.getByRole("button", { name: /view/i }));
    expect(onView).toHaveBeenCalledTimes(1);
  });

  it("Open-in-editor button calls onOpenFile", () => {
    const onOpenFile = vi.fn();
    renderRow({ onOpenFile });
    fireEvent.click(
      screen.getByRole("button", { name: /open demo-skill in editor/i }),
    );
    expect(onOpenFile).toHaveBeenCalledTimes(1);
  });

  it("shows a Link2 icon (with tooltip) when the skill came via a symlink", () => {
    renderRow({ skill: makeSkill({ symlinked: true }) });
    const icon = screen.getByTestId("skill-row-symlink-icon");
    expect(icon).toBeInTheDocument();
    expect(icon).toHaveAttribute("aria-label", "Symlinked skill");
  });

  it("does NOT show the symlink icon for non-symlinked skills", () => {
    renderRow();
    expect(
      screen.queryByTestId("skill-row-symlink-icon"),
    ).not.toBeInTheDocument();
  });

  // ── Stage 5: disable toggle ──────────────────────────────────────

  it("renders enabled by default with the switch checked", () => {
    renderRow({ skill: makeSkill({ id: "demo" }) });
    const sw = screen.getByTestId("skill-row-switch-demo");
    expect(sw).toHaveAttribute("aria-checked", "true");
    expect(
      screen.queryByTestId("skill-row-disabled-badge"),
    ).not.toBeInTheDocument();
  });

  it("renders disabled state: line-through name + badge + greyed row", () => {
    renderRow({ skill: makeSkill({ id: "demo" }), enabled: false });
    const row = screen.getByTestId("skill-row-demo");
    expect(row).toHaveAttribute("data-enabled", "false");
    expect(row.className).toContain("opacity-50");
    expect(screen.getByTestId("skill-row-disabled-badge")).toHaveTextContent(
      /disabled/i,
    );
    const sw = screen.getByTestId("skill-row-switch-demo");
    expect(sw).toHaveAttribute("aria-checked", "false");
  });

  it("clicking the switch fires onToggleEnabled", () => {
    const onToggleEnabled = vi.fn();
    renderRow({
      skill: makeSkill({ id: "demo" }),
      enabled: true,
      onToggleEnabled,
    });
    fireEvent.click(screen.getByTestId("skill-row-switch-demo"));
    expect(onToggleEnabled).toHaveBeenCalledTimes(1);
  });

  it("surfaces invalid legacy names and prevents enabling them", () => {
    const onToggleEnabled = vi.fn();
    renderRow({
      skill: makeSkill({
        id: "legacy",
        name: "Legacy_Skill",
        validationError: "skill name may contain only lowercase letters",
      }),
      onToggleEnabled,
    });
    expect(screen.getByText(/Invalid: skill name may contain/)).toBeInTheDocument();
    const sw = screen.getByTestId("skill-row-switch-legacy");
    expect(sw).toBeDisabled();
    fireEvent.click(sw);
    expect(onToggleEnabled).not.toHaveBeenCalled();
  });
});
