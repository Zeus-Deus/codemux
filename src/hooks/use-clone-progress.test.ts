import { describe, it, expect } from "vitest";
import { matchesCloneTarget } from "./use-clone-progress";

// The backend expands `~` before echoing `targetDir` back in the
// `git-clone-progress` event, so the filter must match the frontend's
// tilde form against the expanded absolute path (the reported bug:
// `~/projects/hermes-agent` vs `/home/zeus/projects/hermes-agent`).
describe("matchesCloneTarget", () => {
  it("matches an identical absolute path", () => {
    expect(
      matchesCloneTarget("/home/zeus/projects/repo", "/home/zeus/projects/repo"),
    ).toBe(true);
  });

  it("matches a tilde target against the backend's expanded path", () => {
    expect(
      matchesCloneTarget(
        "/home/zeus/projects/hermes-agent",
        "~/projects/hermes-agent",
      ),
    ).toBe(true);
  });

  it("does NOT match a different clone target (no cross-talk)", () => {
    expect(
      matchesCloneTarget("/home/zeus/projects/other-repo", "~/projects/repo"),
    ).toBe(false);
    expect(
      matchesCloneTarget(
        "/home/zeus/projects/other-repo",
        "/home/zeus/projects/repo",
      ),
    ).toBe(false);
  });

  it("does not confuse sibling dirs sharing a name suffix", () => {
    // `/projects/repo` must not match `.../my-repo` — endsWith on the
    // slash-prefixed suffix keeps the path-segment boundary.
    expect(
      matchesCloneTarget("/home/zeus/projects/my-repo", "~/projects/repo"),
    ).toBe(false);
  });

  it("accepts everything when the target is empty (defensive default)", () => {
    expect(matchesCloneTarget("/home/zeus/projects/repo", "")).toBe(true);
  });
});
