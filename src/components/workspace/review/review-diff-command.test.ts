import { beforeEach, describe, expect, it, vi } from "vitest";
import { getPrReviewDiff } from "@/tauri/commands";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: vi.fn() }));
beforeEach(() => invoke.mockReset().mockResolvedValue("patch"));

describe("head-bound review diff IPC", () => {
  it("passes the expected SHA to the native command without changing it", async () => {
    expect(await getPrReviewDiff("/fixture/repo", 42, "a".repeat(40))).toBe("patch");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_pr_review_diff", {
      path: "/fixture/repo", prNumber: 42, expectedHeadSha: "a".repeat(40),
    });
  });

  it("keeps callers without a head SHA compatible", async () => {
    expect(await getPrReviewDiff("/fixture/repo", 42)).toBe("patch");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_pr_review_diff", {
      path: "/fixture/repo", prNumber: 42,
    });
  });
});
