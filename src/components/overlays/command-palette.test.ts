import { describe, expect, it } from "vitest";
import { workspaceCommandValue } from "./command-palette";

describe("workspaceCommandValue", () => {
  it("indexes workspace title, branch, and project location", () => {
    expect(
      workspaceCommandValue({
        title: "Fix missing PR icon",
        git_branch: "fix/missing-pr-icon",
        project_root: "/home/zeus/projects/codemux",
        cwd: "/home/zeus/projects/codemux/.worktrees/fix-missing-pr-icon",
      }),
    ).toBe(
      "Fix missing PR icon fix/missing-pr-icon /home/zeus/projects/codemux /home/zeus/projects/codemux/.worktrees/fix-missing-pr-icon",
    );
  });

  it("omits missing optional metadata without polluting search text", () => {
    expect(
      workspaceCommandValue({
        title: "Scratch",
        git_branch: null,
        project_root: null,
        cwd: "/tmp/scratch",
      }),
    ).toBe("Scratch /tmp/scratch");
  });
});
