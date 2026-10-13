import { afterEach, describe, it, expect } from "vitest";
import { useState } from "react";
import { cleanup, render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { useConfirm } from "./confirm-dialog";

afterEach(cleanup);

function Harness() {
  const { confirm, dialog } = useConfirm();
  const [answer, setAnswer] = useState<string>("none");
  return (
    <>
      <button
        type="button"
        onClick={async () => {
          const ok = await confirm({
            title: "Clear all browser data?",
            description: "Cookies and cache are deleted.",
            confirmLabel: "Clear all data",
            destructive: true,
          });
          setAnswer(String(ok));
        }}
      >
        Ask
      </button>
      <output>{answer}</output>
      {dialog}
    </>
  );
}

describe("useConfirm", () => {
  it("resolves true when the named action is chosen", async () => {
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Ask" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Clear all browser data?" });
    expect(dialog).toHaveAccessibleDescription("Cookies and cache are deleted.");
    const action = within(dialog).getByRole("button", { name: "Clear all data" });
    expect(action).toHaveAttribute("data-variant", "destructive");
    fireEvent.click(action);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("true"));
  });

  it("resolves false on cancel", async () => {
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Ask" }));
    fireEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }),
    );
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("false"));
  });

  it("resolves false on Escape", async () => {
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Ask" }));
    fireEvent.keyDown(await screen.findByRole("alertdialog"), { key: "Escape" });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("false"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
