/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { Skill } from "@/tauri/commands";

import { SkillViewModal } from "./skill-view-modal";

afterEach(() => cleanup());

function makeSkill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: "id",
    name: "demo-skill",
    description: "Demo description",
    provider: "claude",
    scope: "user",
    skillDir: "/skills/demo",
    filePath: "/home/user/.claude/skills/demo/SKILL.md",
    body: "# Heading\n\nBody text with **bold**.",
    rawFrontmatter: { name: "demo-skill", description: "Demo description" },
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  };
}

describe("SkillViewModal", () => {
  it("renders nothing when skill is null", () => {
    const { container } = render(
      <SkillViewModal skill={null} onClose={vi.fn()} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders the skill name and provider · scope metadata", () => {
    render(<SkillViewModal skill={makeSkill()} onClose={vi.fn()} />);
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.getByText(/claude · user/)).toBeInTheDocument();
  });

  it("renders the file path in monospace", () => {
    render(<SkillViewModal skill={makeSkill()} onClose={vi.fn()} />);
    const fp = screen.getByTestId("skill-modal-filepath");
    expect(fp).toHaveTextContent(
      "/home/user/.claude/skills/demo/SKILL.md",
    );
    expect(fp.className).toContain("font-mono");
  });

  it("appends the plugin slug to the metadata when present", () => {
    render(
      <SkillViewModal
        skill={makeSkill({
          scope: "plugin",
          pluginSlug: "frontend-design",
        })}
        onClose={vi.fn()}
      />,
    );
    expect(
      screen.getByText(/claude · plugin · frontend-design/),
    ).toBeInTheDocument();
  });

  it("renders the markdown body via ChatMarkdown", () => {
    render(<SkillViewModal skill={makeSkill()} onClose={vi.fn()} />);
    const body = screen.getByTestId("skill-modal-body");
    expect(body).toHaveTextContent("Heading");
    expect(body).toHaveTextContent("Body text with bold.");
  });

  it("shows an italic placeholder when the skill has no body", () => {
    render(
      <SkillViewModal
        skill={makeSkill({ body: "" })}
        onClose={vi.fn()}
      />,
    );
    expect(
      screen.getByText(/this skill has no body content/i),
    ).toBeInTheDocument();
  });

  it("hides the compatibility warning when compatibility is 'compatible'", () => {
    render(<SkillViewModal skill={makeSkill()} onClose={vi.fn()} />);
    expect(
      screen.queryByTestId("skill-modal-compat-warning"),
    ).not.toBeInTheDocument();
  });

  it("renders the compat warning + signals list for soft-warn", () => {
    render(
      <SkillViewModal
        skill={makeSkill({
          compatibility: "soft-warn",
          compatibilitySignals: ["mentions CLI tool: gh", "contains bash code blocks"],
        })}
        onClose={vi.fn()}
      />,
    );
    const warning = screen.getByTestId("skill-modal-compat-warning");
    expect(warning).toHaveTextContent(/may reference external tools/i);
    expect(warning).toHaveTextContent("mentions CLI tool: gh");
    expect(warning).toHaveTextContent("contains bash code blocks");
  });

  it("renders the destructive variant of the compat warning for hard-warn", () => {
    render(
      <SkillViewModal
        skill={makeSkill({
          compatibility: "hard-warn",
          compatibilitySignals: ["allowed-tools frontmatter"],
        })}
        onClose={vi.fn()}
      />,
    );
    const warning = screen.getByTestId("skill-modal-compat-warning");
    expect(warning).toHaveTextContent(/may not work in current session/i);
    expect(warning.className).toContain("border-destructive");
  });

  it("renders the frontmatter disclosure when fields exist", () => {
    render(
      <SkillViewModal
        skill={makeSkill({
          rawFrontmatter: { name: "demo", "allowed-tools": ["Bash"] },
        })}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText("Advanced metadata")).toBeInTheDocument();
    const pre = screen.getByTestId("skill-modal-frontmatter");
    expect(pre.textContent).toContain("allowed-tools");
    expect(pre.textContent).toContain("Bash");
  });

  it("hides the frontmatter section when frontmatter is empty", () => {
    render(
      <SkillViewModal
        skill={makeSkill({ rawFrontmatter: {} })}
        onClose={vi.fn()}
      />,
    );
    expect(screen.queryByText("Advanced metadata")).not.toBeInTheDocument();
  });

  it("explicit Close button calls onClose", () => {
    // Radix Dialog also renders an icon-X close button with the same
    // accessible name. Pick the footer button (variant=outline) so we
    // exercise our own onClick path, not Radix's built-in.
    const onClose = vi.fn();
    render(<SkillViewModal skill={makeSkill()} onClose={onClose} />);
    const closeButtons = screen.getAllByRole("button", { name: "Close" });
    const footerClose = closeButtons.find(
      (btn) => btn.getAttribute("data-variant") === "outline",
    );
    expect(footerClose).toBeDefined();
    fireEvent.click(footerClose!);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
