/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";

import { ComposerFooter } from "./ComposerFooter";

type FooterProps = ComponentProps<typeof ComposerFooter>;

afterEach(() => cleanup());

function baseProps(): FooterProps {
  return {
    provider: "claude",
    model: null,
    permissionMode: "bypassPermissions",
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: [
      {
        value: "bypassPermissions",
        label: "Full access",
        description: "",
        is_default: true,
      },
      {
        value: "default",
        label: "Supervised",
        description: "",
        is_default: false,
      },
    ],
    ultrathinkInBodyText: false,
    streaming: false,
    canSubmit: true,
    showProviderPicker: false,
    mode: "default",
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
    onProviderChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    controlsDisabled: false,
  };
}

function renderFooter(props: Partial<FooterProps> = {}) {
  return render(
    <TooltipProvider>
      <ComposerFooter {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

describe("ComposerFooter — Stage 3 mode selector", () => {
  it("renders the '+ Mode' dropdown trigger when mode is default", () => {
    renderFooter({ mode: "default" });
    expect(
      screen.getByRole("button", { name: /Activate mode/i }),
    ).toBeInTheDocument();
    // PermissionModePicker renders in default mode — its label
    // "Full access" should be visible somewhere.
    expect(screen.getByText("Full access")).toBeInTheDocument();
  });

  it("renders the ModePill when mode is active, hides the '+ Mode' trigger", () => {
    const { queryByRole } = renderFooter({ mode: "plan" });
    // ModePill renders under role="status" with a Plan label.
    expect(screen.getByRole("status", { name: /Plan mode active/i })).toBeInTheDocument();
    // The trigger is gone.
    expect(queryByRole("button", { name: /Activate mode/i })).toBeNull();
  });

  it("keeps the PermissionModePicker visible but disabled while a mode pill is active", () => {
    // Prior behavior was to unmount the picker, which surprised
    // users ("my picker disappeared"). Current design: picker stays
    // on-screen for discoverability; clicks are disabled so it can't
    // contradict the pill that has commandeered the live permission
    // mode.
    renderFooter({ mode: "plan" });
    const button = screen.getByRole("button", { name: /Full access/i });
    expect(button).toBeInTheDocument();
    expect(button).toBeDisabled();
  });

  it("dropdown Plan item fires onModeActivate('plan')", async () => {
    const user = userEvent.setup();
    const onModeActivate = vi.fn();
    renderFooter({ mode: "default", onModeActivate });
    // Radix Dropdown uses pointer events — fireEvent.click doesn't
    // open the menu; userEvent.click dispatches the full pointer
    // sequence.
    await user.click(
      screen.getByRole("button", { name: /Activate mode/i }),
    );
    await user.click(await screen.findByRole("menuitem", { name: /Plan/ }));
    expect(onModeActivate).toHaveBeenCalledWith("plan");
  });

  it("dropdown Ask item fires onModeActivate('ask') (Stage 4)", async () => {
    const user = userEvent.setup();
    const onModeActivate = vi.fn();
    renderFooter({ mode: "default", onModeActivate });
    await user.click(
      screen.getByRole("button", { name: /Activate mode/i }),
    );
    await user.click(await screen.findByRole("menuitem", { name: /Ask/ }));
    expect(onModeActivate).toHaveBeenCalledWith("ask");
  });

  it("Debug dropdown item is disabled (Stage 6)", async () => {
    const user = userEvent.setup();
    renderFooter({ mode: "default" });
    await user.click(
      screen.getByRole("button", { name: /Activate mode/i }),
    );
    const debug = await screen.findByRole("menuitem", { name: /Debug/ });
    expect(debug.getAttribute("aria-disabled")).toBe("true");
  });

  it("clicking the pill's X calls onModeRemove", () => {
    const onModeRemove = vi.fn();
    renderFooter({ mode: "plan", onModeRemove });
    fireEvent.click(
      screen.getByRole("button", { name: /Remove Plan mode/i }),
    );
    expect(onModeRemove).toHaveBeenCalled();
  });
});
