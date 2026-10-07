/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { PermissionModeOption } from "@/tauri/types";
import {
  FullAccessNotice,
  isFullAccessMode,
  useFullAccessNoticeStore,
} from "./FullAccessNotice";

const MODES: PermissionModeOption[] = [
  {
    value: "bypassPermissions",
    label: "Full access",
    description: "Run every tool without asking",
    is_default: true,
  },
  { value: "default", label: "Ask first", description: "Prompt", is_default: false },
];

afterEach(cleanup);
beforeEach(() => useFullAccessNoticeStore.setState({ dismissed: false }));

describe("FullAccessNotice", () => {
  it("explains Full access until it is acknowledged", () => {
    render(<FullAccessNotice permissionMode="bypassPermissions" permissionModes={MODES} />);
    expect(screen.getByRole("note")).toHaveTextContent(
      "New chats start with Full access.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Got it" }));
    expect(screen.queryByRole("note")).toBeNull();
    expect(useFullAccessNoticeStore.getState().dismissed).toBe(true);
  });

  it("points at the access menu only once the composer shows it", () => {
    const view = render(
      <FullAccessNotice permissionMode="bypassPermissions" permissionModes={MODES} />,
    );
    expect(screen.getByRole("note")).toHaveTextContent("from the Full access menu");
    // Capabilities not harvested yet: the picker is hidden, so the
    // explanation stands alone.
    view.rerender(
      <FullAccessNotice permissionMode="danger-full-access" permissionModes={null} />,
    );
    expect(screen.getByRole("note")).toHaveTextContent("New chats start with Full access.");
    expect(screen.getByRole("note")).not.toHaveTextContent("menu");
  });

  it("stays hidden for supervised modes and providers without permissions", () => {
    const view = render(
      <FullAccessNotice permissionMode="default" permissionModes={MODES} />,
    );
    expect(screen.queryByRole("note")).toBeNull();
    view.rerender(
      <FullAccessNotice permissionMode="bypassPermissions" permissionModes={[]} />,
    );
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("recognises each provider's full-access value", () => {
    expect(isFullAccessMode("bypassPermissions")).toBe(true);
    expect(isFullAccessMode("danger-full-access")).toBe(true);
    expect(isFullAccessMode("default")).toBe(false);
    expect(isFullAccessMode(null)).toBe(false);
  });
});
