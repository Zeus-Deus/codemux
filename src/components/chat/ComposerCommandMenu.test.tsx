/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ComposerCommandMenu } from "./ComposerCommandMenu";
import { PermissionModePicker } from "./pickers/PermissionModePicker";

afterEach(cleanup);

it("lets the embedded access picker own Enter without selecting a composer command", async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  const onPermissionChange = vi.fn();
  const onEscape = vi.fn();
  render(
    <ComposerCommandMenu
      open
      submode="main"
      query=""
      onQueryChange={() => {}}
      onSelect={onSelect}
      onEscape={onEscape}
      items={[{ id: "mode:plan", label: "Plan", command: "/plan", group: "MODES", onSelect: () => {} }]}
      headerSlot={
        <PermissionModePicker
          value="ask"
          modes={[
            { value: "ask", label: "Ask first", description: "Ask before tools", is_default: true },
            { value: "full", label: "Full access", description: "Allow tools", is_default: false },
          ]}
          onChange={onPermissionChange}
        />
      }
    />,
  );

  await user.click(screen.getByRole("combobox"));
  await user.tab();
  expect(screen.getByRole("button", { name: "Ask first" })).toHaveFocus();
  await user.keyboard("{Enter}");
  expect(onSelect).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog")).toBeInTheDocument();

  await user.keyboard("{ArrowDown}{Enter}");
  expect(onPermissionChange).toHaveBeenCalledWith("full");
  expect(onSelect).not.toHaveBeenCalled();
  expect(onEscape).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).toBeNull();

  await user.click(screen.getByRole("button", { name: "Ask first" }));
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onEscape).not.toHaveBeenCalled();
  await user.keyboard("{Escape}");
  expect(onEscape).toHaveBeenCalledOnce();

  await user.click(screen.getByRole("combobox"));
  await user.keyboard("{Enter}");
  expect(onSelect).toHaveBeenCalledOnce();
});
