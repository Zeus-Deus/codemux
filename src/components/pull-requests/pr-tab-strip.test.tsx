/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { rowKey, type PrRow } from "@/lib/pr-overview";
import { PrTabStrip } from "./pr-tab-strip";

function row(number: number): PrRow {
  return {
    number,
    title: `pull request ${number}`,
    author: "juliusm",
    head_branch: `branch/${number}`,
    is_draft: false,
    additions: 1,
    deletions: 1,
    review_decision: null,
    checks: "passing",
    review_requested_from: [],
    updated_at: new Date().toISOString(),
    url: `https://github.com/example/codemux/pull/${number}`,
    projectRoot: "/repo",
    repo: "example/codemux",
    providerKind: "github",
  } as PrRow;
}

function renderStrip() {
  const tabs = [row(412), row(418)];
  const onSelect = vi.fn();
  const onClose = vi.fn();
  render(
    <PrTabStrip
      tabs={tabs}
      activeKey={rowKey(tabs[0])}
      candidates={tabs}
      onSelect={onSelect}
      onClose={onClose}
      onOpenInBrowser={vi.fn()}
    />,
  );
  return { onSelect, onClose };
}

afterEach(cleanup);

describe("PrTabStrip from the keyboard", () => {
  it("names each tab by its title, not just its number", () => {
    renderStrip();
    const tab = screen.getByRole("tab", { name: "#418 pull request 418" });
    expect(tab).toHaveAttribute("title", "pull request 418");
  });

  it("walks the tabs with the arrows and switches on Enter", async () => {
    const user = userEvent.setup();
    const { onSelect } = renderStrip();

    // Tab lands on the active tab only (roving focus).
    await user.tab();
    expect(screen.getByTestId("pr-tab-412")).toHaveFocus();

    await user.keyboard("{ArrowRight}");
    expect(screen.getByTestId("pr-tab-418")).toHaveFocus();
    expect(onSelect).not.toHaveBeenCalled();

    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ number: 418 }));
  });

  it("closes the focused tab on Delete", async () => {
    const user = userEvent.setup();
    const { onClose } = renderStrip();

    await user.tab();
    await user.keyboard("{Delete}");
    expect(onClose).toHaveBeenCalledWith(rowKey(row(412)));
  });
});
