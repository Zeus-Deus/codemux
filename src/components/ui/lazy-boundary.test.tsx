/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { LazyBoundary } from "./lazy-boundary";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("LazyBoundary", () => {
  it("shows an accessible loading fallback while a child suspends", () => {
    const pending = new Promise<never>(() => {});
    function SuspendedChild(): never {
      throw pending;
    }

    render(
      <LazyBoundary label="settings">
        <SuspendedChild />
      </LazyBoundary>,
    );

    expect(
      screen.getByRole("status", { name: "Loading settings" }),
    ).toHaveTextContent("Loading settings…");
  });

  it("keeps the shell visible behind a compact overlay loading state", () => {
    const pending = new Promise<never>(() => {});
    function SuspendedChild(): never {
      throw pending;
    }

    render(
      <LazyBoundary label="command palette" presentation="overlay">
        <SuspendedChild />
      </LazyBoundary>,
    );

    const status = screen.getByRole("status", {
      name: "Loading command palette",
    });
    expect(status).toHaveClass("bg-background/20");
    expect(status).not.toHaveClass("bg-background");
    expect(status.firstElementChild).toHaveClass("bg-popover/95", "shadow-xl");
  });

  it("contains chunk render failures and offers a reload recovery", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    function BrokenChild(): never {
      throw new Error("chunk unavailable");
    }

    render(
      <LazyBoundary label="workspace overview">
        <BrokenChild />
      </LazyBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Couldn’t load workspace overview.",
    );
    expect(screen.getByRole("button", { name: "Reload Codemux" })).toBeEnabled();
  });

  it("clears a failed chunk when the boundary is reused for another surface", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    function BrokenChild(): never {
      throw new Error("settings chunk unavailable");
    }

    const view = render(
      <LazyBoundary label="settings">
        <BrokenChild />
      </LazyBoundary>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Couldn’t load settings.",
    );

    view.rerender(
      <LazyBoundary label="automations">
        <div>Automations loaded</div>
      </LazyBoundary>,
    );

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("Automations loaded")).toBeVisible();
  });
});
