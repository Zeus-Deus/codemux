/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChatHomeLanding } from "./ChatHomeLanding";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";
afterEach(cleanup);
beforeEach(() => {
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: false, localSessionImportOfferDismissed: false });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});
describe("ChatHomeLanding", () => {
  it("offers opt-in import only for first-run local enabled profiles", () => {
    const view = render(<ChatHomeLanding composer={<div />} />);
    fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
    expect(useUIStore.getState().showLocalSessionImport).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    useUIStore.setState({ localSessionImportOfferDismissed: false });
    useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("hides import for remote clients and disabled chat", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    const view = render(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    useFeatureFlags.setState({ enableAgentChat: false });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("renders the marquee headline", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div data-testid="composer-slot" />} />,
    );
    const heading = container.querySelector("h1");
    expect(heading?.textContent).toBe("What should we do today?");
  });

  it("asks what to work on when the thread has no project", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} projectName={null} />,
    );
    expect(container.querySelector("h1")?.textContent).toBe(
      "What should we work on?",
    );
    expect(
      screen.queryByRole("button", { name: "or start without a project" }),
    ).toBeNull();
  });

  it("offers the project picker as 'No project' under the no-project headline", () => {
    render(
      <ChatHomeLanding
        composer={<div />}
        projectName={null}
        projectPicker={(trigger) => <span data-testid="picker">{trigger}</span>}
      />,
    );
    const trigger = screen.getByRole("button", { name: "No project" });
    expect(screen.getByTestId("picker")).toContainElement(trigger);
    // Under the heading, not inside it.
    expect(trigger.closest("h1")).toBeNull();
  });

  it("reserves the line under a project-scoped headline so it doesn't jump", () => {
    const view = render(
      <ChatHomeLanding
        composer={<div />}
        projectName="codemux"
        projectPicker={(trigger) => trigger}
        onStartWithoutProject={() => {}}
      />,
    );
    const withProject = screen.getByTestId("landing-subline").className;
    view.rerender(
      <ChatHomeLanding
        composer={<div />}
        projectName={null}
        projectPicker={(trigger) => trigger}
        onStartWithoutProject={() => {}}
      />,
    );
    expect(screen.getByTestId("landing-subline").className).toBe(withProject);
    expect(withProject).toContain("h-6");
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByTestId("landing-subline")).toBeNull();
  });

  it("moves focus to the 'No project' picker after starting without a project", () => {
    function Harness() {
      const [projectName, setProjectName] = useState<string | null>("codemux");
      return (
        <ChatHomeLanding
          composer={<div />}
          projectName={projectName}
          projectPicker={(trigger) => trigger}
          onStartWithoutProject={() => setProjectName(null)}
        />
      );
    }
    render(<Harness />);
    fireEvent.click(
      screen.getByRole("button", { name: "or start without a project" }),
    );
    expect(screen.getByRole("button", { name: "No project" })).toHaveFocus();
  });

  it("names the project through the picker and offers to drop it", () => {
    const onStartWithoutProject = vi.fn();
    const { container } = render(
      <ChatHomeLanding
        composer={<div />}
        projectName="codemux"
        projectPicker={(trigger) => <span data-testid="picker">{trigger}</span>}
        onStartWithoutProject={onStartWithoutProject}
      />,
    );
    expect(container.querySelector("h1")?.textContent).toBe(
      "What should we build in codemux?",
    );
    const name = screen.getByRole("button", { name: "codemux" });
    expect(screen.getByTestId("picker")).toContainElement(name);
    fireEvent.click(
      screen.getByRole("button", { name: "or start without a project" }),
    );
    expect(onStartWithoutProject).toHaveBeenCalledTimes(1);
  });

  it("renders the composer passed via props", () => {
    const { container } = render(
      <ChatHomeLanding
        composer={<div data-testid="composer-slot-a">slotA</div>}
      />,
    );
    expect(
      container.querySelector('[data-testid="composer-slot-a"]'),
    ).not.toBeNull();
  });

  it.each(["codemux", null])(
    "uses only neutral foreground/muted color tokens — no accents (project: %s)",
    (projectName) => {
      const { container } = render(
        <ChatHomeLanding
          composer={<div />}
          projectName={projectName}
          projectPicker={(trigger) => trigger}
          onStartWithoutProject={() => {}}
        />,
      );
      const html = container.innerHTML;
      expect(html).not.toMatch(/\btext-primary\b/);
      expect(html).not.toMatch(/\bbg-primary\b/);
      expect(html).not.toMatch(/\btext-success\b/);
      expect(html).not.toMatch(/\btext-warning\b/);
      expect(html).not.toMatch(/\btext-danger\b/);
    },
  );

  it("centers content vertically and horizontally", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("items-center");
    expect(root.className).toContain("justify-center");
  });
});
