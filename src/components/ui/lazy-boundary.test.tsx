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

  it("hides a closed overlay's failure and retries it on the next open", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let broken = true;
    function Dialog({ open }: { open: boolean }) {
      if (broken && open) throw new Error("dialog crashed");
      return open ? <div>Dialog open</div> : null;
    }
    const at = (open: boolean) => (
      <LazyBoundary label="content search" presentation="overlay" open={open}>
        <Dialog open={open} />
      </LazyBoundary>
    );

    const view = render(at(true));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Couldn’t load content search.",
    );

    // Escape closes the overlay: the full-screen error must not stay up.
    view.rerender(at(false));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    broken = false;
    view.rerender(at(true));
    expect(screen.getByText("Dialog open")).toBeVisible();
  });

  it("does not show a closed overlay's loading state", () => {
    const pending = new Promise<never>(() => {});
    function SuspendedChild(): never {
      throw pending;
    }

    render(
      <LazyBoundary label="file search" presentation="overlay" open={false}>
        <SuspendedChild />
      </LazyBoundary>,
    );

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
