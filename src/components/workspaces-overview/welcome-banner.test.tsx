/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setShowSettings: vi.fn(),
      setShowWorkspacesOverview: vi.fn(),
    }),
  ),
}));

import {
  WELCOME_BANNER_DISMISS_KEY,
  WelcomeBanner,
} from "./welcome-banner";

afterEach(() => {
  cleanup();
  try {
    localStorage.clear();
  } catch {
    /* test env */
  }
});

describe("WelcomeBanner", () => {
  it("renders the 'fresh' variant when the user has 0 devices and 0 siblings", () => {
    render(
      <WelcomeBanner
        deviceCount={0}
        siblingWorkspaceCount={0}
        localWorkspaceCount={0}
      />,
    );
    expect(screen.getByText("Welcome to Workspaces")).toBeInTheDocument();
    expect(
      screen.getByText(/This is where your work lives across devices/i),
    ).toBeInTheDocument();
    // Fresh variant gets the Add-a-device CTA.
    expect(screen.getByText("Add a device")).toBeInTheDocument();
  });

  it("renders the 'device-no-siblings' variant when devices exist but no sibling workspaces", () => {
    render(
      <WelcomeBanner
        deviceCount={1}
        siblingWorkspaceCount={0}
        localWorkspaceCount={3}
      />,
    );
    expect(
      screen.getByText("Workspaces, now across all your devices"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/right-click any workspace/i),
    ).toBeInTheDocument();
    // No Add-a-device CTA on this variant.
    expect(screen.queryByText("Add a device")).toBeNull();
  });

  it("renders the 'has-siblings' variant with the correct count", () => {
    const { container } = render(
      <WelcomeBanner
        deviceCount={1}
        siblingWorkspaceCount={4}
        localWorkspaceCount={2}
      />,
    );
    expect(
      screen.getByText("Welcome to Codemux on this device"),
    ).toBeInTheDocument();
    // The body paragraph contains the plural count.
    expect(container.textContent ?? "").toContain("4 workspaces");
  });

  it("renders the singular form when exactly one sibling exists", () => {
    const { container } = render(
      <WelcomeBanner
        deviceCount={1}
        siblingWorkspaceCount={1}
        localWorkspaceCount={0}
      />,
    );
    const text = container.textContent ?? "";
    expect(text).toContain("1 workspace");
    expect(text).not.toContain("1 workspaces");
  });

  it("persists dismissal so subsequent renders return null", () => {
    const { rerender, queryByText } = render(
      <WelcomeBanner
        deviceCount={1}
        siblingWorkspaceCount={3}
        localWorkspaceCount={0}
      />,
    );
    expect(
      screen.getByText("Welcome to Codemux on this device"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Dismiss welcome message"));
    // Same instance hides immediately.
    expect(queryByText("Welcome to Codemux on this device")).toBeNull();

    // A fresh render also returns null because the dismissal
    // landed in localStorage.
    expect(localStorage.getItem(WELCOME_BANNER_DISMISS_KEY)).toBe("1");
    rerender(
      <WelcomeBanner
        deviceCount={1}
        siblingWorkspaceCount={3}
        localWorkspaceCount={0}
      />,
    );
    expect(queryByText("Welcome to Codemux on this device")).toBeNull();
  });

  it("never re-shows after dismissal even when the user's state changes", () => {
    localStorage.setItem(WELCOME_BANNER_DISMISS_KEY, "1");
    // Pretend the user just gained 5 sibling workspaces — banner
    // would normally pull them in with the 'has-siblings' variant.
    const { container } = render(
      <WelcomeBanner
        deviceCount={2}
        siblingWorkspaceCount={5}
        localWorkspaceCount={3}
      />,
    );
    expect(container.firstChild).toBeNull();
  });
});
