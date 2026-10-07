/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot, PaneStatus } from "@/tauri/types";
import { NeedsYouAnnouncer } from "./needs-you-announcer";

function setStatuses(statuses: Record<string, PaneStatus>) {
  const workspaces = [
    { workspace_id: "ws-a", title: "Auth fix", surfaces: [{ root: { kind: "pane", pane_id: "pa" } }] },
    { workspace_id: "ws-b", title: "Docs", surfaces: [{ root: { kind: "pane", pane_id: "pb" } }] },
  ];
  act(() => {
    useAppStore.setState({
      appState: { workspaces, pane_statuses: statuses } as unknown as AppStateSnapshot,
    });
  });
}

afterEach(() => {
  cleanup();
  useAppStore.setState({ appState: null });
});

describe("NeedsYouAnnouncer", () => {
  it("stays quiet about workspaces already blocked when the app loads", () => {
    setStatuses({ pa: "permission" });
    render(<NeedsYouAnnouncer />);
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("announces a workspace once when its agent becomes blocked", () => {
    setStatuses({ pa: "working" });
    render(<NeedsYouAnnouncer />);

    setStatuses({ pa: "permission" });
    expect(screen.getByRole("status")).toHaveTextContent("Auth fix needs you");

    // Still blocked on the next snapshot: nothing new to say.
    const before = screen.getByRole("status").firstElementChild;
    setStatuses({ pa: "permission", pb: "working" });
    expect(screen.getByRole("status").firstElementChild).toBe(before);
  });

  it("announces again when the same workspace blocks a second time", () => {
    setStatuses({});
    render(<NeedsYouAnnouncer />);
    setStatuses({ pb: "permission" });
    const first = screen.getByRole("status").firstElementChild;

    setStatuses({ pb: "working" });
    setStatuses({ pb: "permission" });
    const second = screen.getByRole("status").firstElementChild;
    expect(second).toHaveTextContent("Docs needs you");
    expect(second).not.toBe(first);
  });
});
