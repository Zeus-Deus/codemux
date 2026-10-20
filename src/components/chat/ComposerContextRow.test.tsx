/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

const state = vi.hoisted(() => ({
  workspace: null as null | {
    pr_state: string | null;
    pr_number: number | null;
    pr_url: string | null;
    provider_kind: string | null;
  },
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => state.workspace,
}));

import { ComposerContextRow } from "./ComposerContextRow";

function renderRow() {
  return render(
    <ComposerContextRow projectRoot="/repo/codemux" isHomeWorkspace={false} branch="fix-composer" />,
  );
}

afterEach(() => {
  cleanup();
  state.workspace = null;
});

describe("ComposerContextRow", () => {
  it("shows where the first message runs: project and branch", () => {
    renderRow();
    expect(screen.getByText("codemux")).toBeInTheDocument();
    expect(screen.getByText("fix-composer")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("adds the branch's pull request when there is one", () => {
    state.workspace = {
      pr_state: "OPEN",
      pr_number: 585,
      pr_url: "https://github.com/o/r/pull/585",
      provider_kind: null,
    };
    renderRow();
    expect(screen.getByRole("button", { name: /#585/ })).toBeEnabled();
  });
});
