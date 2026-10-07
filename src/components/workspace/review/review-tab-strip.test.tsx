/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";

import { ReviewTabStrip } from "./review-tab-strip";

const TABS = [
  { id: "summary", label: "Summary" },
  { id: "timeline", label: "Timeline" },
  { id: "code", label: "Code", count: 8 },
];

function Harness() {
  const [active, setActive] = useState("summary");
  return <ReviewTabStrip tabs={TABS} activeId={active} onSelect={setActive} />;
}

afterEach(cleanup);

describe("ReviewTabStrip from the keyboard", () => {
  it("is one Tab stop, and the arrows move between tabs", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.tab();
    expect(screen.getByTestId("review-tab-summary")).toHaveFocus();

    await user.keyboard("{ArrowRight}");
    expect(screen.getByTestId("review-tab-timeline")).toHaveFocus();
    expect(screen.getByTestId("review-tab-timeline")).toHaveAttribute("aria-selected", "true");

    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(screen.getByTestId("review-tab-code")).toHaveFocus();

    await user.keyboard("{Home}");
    expect(screen.getByTestId("review-tab-summary")).toHaveAttribute("aria-selected", "true");
  });
});
