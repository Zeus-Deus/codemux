/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DropdownMenu, DropdownMenuContent } from "@/components/ui/dropdown-menu";
import type { AgentChatSessionRecord } from "@/tauri/commands";
import { SessionHistoryList } from "./session-history-menu";

afterEach(cleanup);

describe("SessionHistoryList", () => {
  it("marks the current session with a check instead of a fill", async () => {
    const sessions: AgentChatSessionRecord[] = ["current", "other"].map((id) => ({
      thread_id: id,
      sdk_session_id: id,
      workspace_id: "ws-1",
      cwd: "/project",
      provider: "claude",
      title: id,
      created_at: "2026-04-24 12:00:00",
      last_active_at: "2026-04-24 12:00:00",
      model: null,
      effort: null,
      context_window: null,
      permission_mode: null,
    }));
    const onSelect = vi.fn();
    render(
      <DropdownMenu open>
        <DropdownMenuContent>
          <SessionHistoryList
            sessions={sessions}
            loading={false}
            activeThreadId="current"
            onSelect={onSelect}
            onNewChat={vi.fn()}
            onDelete={vi.fn()}
            closeMenu={vi.fn()}
          />
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    const current = screen.getByTestId("session-row-current");
    const other = screen.getByTestId("session-row-other");
    expect(current).toHaveAttribute("aria-checked", "true");
    expect(current).toHaveClass("font-medium");
    expect(current).not.toHaveClass("bg-surface-3");
    expect(other).toHaveAttribute("aria-checked", "false");
    for (const row of [current, other]) {
      expect(row.lastElementChild).toHaveClass("size-4", "shrink-0");
    }
    expect(current.lastElementChild?.querySelector("svg")).not.toBeNull();
    expect(other.lastElementChild).toBeEmptyDOMElement();
    await userEvent.click(other);
    expect(onSelect).toHaveBeenCalledWith(sessions[1]);
  });
});
