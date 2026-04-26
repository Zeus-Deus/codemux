/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

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

describe("SkillRow", () => {
  it("renders the skill name and description", () => {
    render(
      <SkillRow skill={makeSkill()} onView={vi.fn()} onOpenFile={vi.fn()} />,
    );
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.getByText("Does demo things.")).toBeInTheDocument();
  });

  it("hides the description block when none is set", () => {
    render(
      <SkillRow
        skill={makeSkill({ description: null })}
        onView={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.queryByText("Does demo things.")).not.toBeInTheDocument();
  });

  it("does NOT render the compatibility badge when compatibility is 'compatible'", () => {
    render(
      <SkillRow skill={makeSkill()} onView={vi.fn()} onOpenFile={vi.fn()} />,
    );
    expect(screen.queryByTestId("compatibility-badge")).not.toBeInTheDocument();
  });

  it("renders soft-warn badge when compatibility is 'soft-warn'", () => {
    render(
      <SkillRow
        skill={makeSkill({ compatibility: "soft-warn" })}
        onView={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("compatibility-badge")).toHaveAttribute(
      "data-level",
      "soft-warn",
    );
  });

  it("renders hard-warn badge when compatibility is 'hard-warn'", () => {
    render(
      <SkillRow
        skill={makeSkill({ compatibility: "hard-warn" })}
        onView={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByTestId("compatibility-badge")).toHaveAttribute(
      "data-level",
      "hard-warn",
    );
  });

  it("View button calls onView", () => {
    const onView = vi.fn();
    render(
      <SkillRow skill={makeSkill()} onView={onView} onOpenFile={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /view/i }));
    expect(onView).toHaveBeenCalledTimes(1);
  });

  it("Open-in-editor button calls onOpenFile", () => {
    const onOpenFile = vi.fn();
    render(
      <SkillRow skill={makeSkill()} onView={vi.fn()} onOpenFile={onOpenFile} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /open demo-skill in editor/i }));
    expect(onOpenFile).toHaveBeenCalledTimes(1);
  });

  it("shows a 'symlinked' label when the skill came via a symlink", () => {
    render(
      <SkillRow
        skill={makeSkill({ symlinked: true })}
        onView={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByText("symlinked")).toBeInTheDocument();
  });
});
