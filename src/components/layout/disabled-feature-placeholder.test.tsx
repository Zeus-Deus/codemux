/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useUIStore } from "@/stores/ui-store";

import { DisabledFeaturePlaceholder } from "./disabled-feature-placeholder";

afterEach(() => {
  cleanup();
  // Reset UI store side effects between tests so the previous test's
  // setShowSettings spy doesn't bleed in.
  useUIStore.setState({ showSettings: false, settingsSection: null });
});

describe("DisabledFeaturePlaceholder", () => {
  it("renders the feature name and default description", () => {
    render(<DisabledFeaturePlaceholder feature="Agent Chat" />);
    expect(screen.getByText(/Agent Chat is disabled/)).toBeInTheDocument();
    expect(
      screen.getByText(/Your data is preserved/i),
    ).toBeInTheDocument();
  });

  it("accepts a custom description override", () => {
    render(
      <DisabledFeaturePlaceholder
        feature="Skill Sync"
        description="Custom suspension reason."
      />,
    );
    expect(screen.getByText("Custom suspension reason.")).toBeInTheDocument();
  });

  it("CTA opens Settings → Interface", async () => {
    const user = userEvent.setup();
    const setShowSettings = vi.fn();
    useUIStore.setState({ setShowSettings } as unknown as Parameters<typeof useUIStore.setState>[0]);

    render(<DisabledFeaturePlaceholder feature="Agent Chat" />);

    await user.click(
      screen.getByRole("button", { name: /Open Settings/i }),
    );

    expect(setShowSettings).toHaveBeenCalledWith(true, "interface");
  });
});
