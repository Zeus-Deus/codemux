/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useUIStore } from "@/stores/ui-store";

afterEach(() => cleanup());

/** A dialog opened from state, like the command palette: no DialogTrigger. */
function StoreDrivenDialog({ onCloseAutoFocus }: { onCloseAutoFocus?: (event: Event) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <textarea aria-label="Message" onKeyDown={(e) => e.key === "k" && e.ctrlKey && setOpen(true)} />
      <button type="button">Elsewhere</button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent onCloseAutoFocus={onCloseAutoFocus}>
          <DialogTitle>Palette</DialogTitle>
          <input aria-label="Search" autoFocus />
        </DialogContent>
      </Dialog>
    </>
  );
}

describe("DialogContent focus return", () => {
  it("gives focus back to what had it before a trigger-less dialog opened", async () => {
    const user = userEvent.setup();
    render(<StoreDrivenDialog />);
    const message = screen.getByRole("textbox", { name: "Message" });
    message.focus();
    await user.keyboard("{Control>}k{/Control}");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Search" })).toHaveFocus());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(message).toHaveFocus());
  });

  it("leaves focus alone when the caller handles it", async () => {
    const user = userEvent.setup();
    render(
      <StoreDrivenDialog
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          screen.getByRole("button", { name: "Elsewhere" }).focus();
        }}
      />,
    );
    screen.getByRole("textbox", { name: "Message" }).focus();
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(screen.getByRole("button", { name: "Elsewhere" })).toHaveFocus());
  });
});

describe("TooltipTrigger", () => {
  function PopoverWithTooltip() {
    return (
      <TooltipProvider delayDuration={0}>
        <button type="button">Before</button>
        <Popover>
          <Tooltip>
            <TooltipTrigger asChild>
              <PopoverTrigger asChild>
                <button type="button">Details</button>
              </PopoverTrigger>
            </TooltipTrigger>
            <TooltipContent>Workspace details</TooltipContent>
          </Tooltip>
          <PopoverContent>
            <button type="button">Inside</button>
          </PopoverContent>
        </Popover>
      </TooltipProvider>
    );
  }

  it("shows its label on Tab", async () => {
    const user = userEvent.setup();
    render(<PopoverWithTooltip />);
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Details" })).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Workspace details");
  });

  it("stays quiet when a closing popover hands focus back", async () => {
    const user = userEvent.setup();
    render(<PopoverWithTooltip />);
    await user.tab();
    await user.tab();
    await screen.findByRole("tooltip");
    await user.keyboard("{Enter}");
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    const trigger = screen.getByRole("button", { name: "Details" });
    await waitFor(() => expect(trigger).toHaveFocus());
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});

describe("Settings focus", () => {
  const nextFrames = () => act(() => new Promise((resolve) => setTimeout(resolve, 80)));

  it("lands in the settings search on open", async () => {
    const view = render(<textarea aria-label="Message" />);
    const message = screen.getByRole("textbox", { name: "Message" });
    message.focus();

    act(() => useUIStore.getState().setShowSettings(true));
    // Settings replaces the shell.
    view.rerender(<input aria-label="Search settings" />);
    await nextFrames();
    expect(screen.getByRole("textbox", { name: "Search settings" })).toHaveFocus();
    act(() => useUIStore.getState().setShowSettings(false));
  });

  it("returns to the element that had focus when it is still there", async () => {
    render(<textarea aria-label="Message" />);
    const message = screen.getByRole("textbox", { name: "Message" });
    message.focus();
    act(() => useUIStore.getState().setShowSettings(true));
    act(() => message.blur());
    act(() => useUIStore.getState().setShowSettings(false));
    await nextFrames();
    expect(message).toHaveFocus();
  });
});
