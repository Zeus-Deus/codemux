/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

vi.mock("@/tauri/commands", () => ({
  listToolPermissions: vi.fn(),
  removeToolPermission: vi.fn(),
}));

import { listToolPermissions, removeToolPermission } from "@/tauri/commands";
import type { PermissionRule } from "@/tauri/commands";
import { toast } from "@/lib/toast";

import { PermissionsSection } from "./permissions-section";

afterEach(() => cleanup());

const RULES: PermissionRule[] = [
  {
    tool_name: "Bash",
    rule_content: null,
    behavior: "allow",
    scope: "user",
    source_path: "/home/u/.claude/settings.json",
  },
  {
    tool_name: "Read",
    rule_content: "src/**",
    behavior: "allow",
    scope: "user",
    source_path: "/home/u/.claude/settings.json",
  },
  {
    tool_name: "WebFetch",
    rule_content: null,
    behavior: "allow",
    scope: "local",
    source_path: "/work/proj/.claude/settings.local.json",
  },
];

describe("PermissionsSection", () => {
  beforeEach(() => {
    vi.mocked(listToolPermissions).mockReset();
    vi.mocked(removeToolPermission).mockReset();
    vi.mocked(toast.success).mockReset();
    vi.mocked(toast.error).mockReset();
  });

  it("renders the CLI-sharing banner", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue([]);
    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() =>
      expect(
        screen.getByText(/also used by the Claude CLI/i),
      ).toBeInTheDocument(),
    );
  });

  it("groups rules by scope and shows tool name + rule content", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    render(<PermissionsSection projectRoot="/work/proj" />);

    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    expect(screen.getByText("User-wide")).toBeInTheDocument();
    expect(screen.getByText("This project (gitignored)")).toBeInTheDocument();
    expect(screen.getByText("This project (shared)")).toBeInTheDocument();
    // The shared scope has no rules — it should explicitly say so.
    expect(
      screen.getAllByText(/no rules in this scope/i).length,
    ).toBeGreaterThan(0);

    expect(screen.getByText("Bash")).toBeInTheDocument();
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.getByText("(src/**)")).toBeInTheDocument();
    expect(screen.getByText("WebFetch")).toBeInTheDocument();
  });

  it("hides project-scoped groups when projectRoot is null", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue([RULES[0]!]);
    render(<PermissionsSection projectRoot={null} />);

    await waitFor(() =>
      expect(screen.getByText("User-wide")).toBeInTheDocument(),
    );
    expect(screen.queryByText("This project (gitignored)")).not.toBeInTheDocument();
    expect(screen.queryByText("This project (shared)")).not.toBeInTheDocument();
  });

  it("removes a rule via the confirm dialog and refetches", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    vi.mocked(removeToolPermission).mockResolvedValue();
    const user = userEvent.setup();

    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    // Click the Remove button on the Bash row. There are three remove
    // buttons rendered (one per rule); the first one corresponds to
    // the first user-wide rule, which is Bash.
    const removeButtons = screen.getAllByLabelText("Remove rule");
    await user.click(removeButtons[0]!);

    expect(
      await screen.findByRole("alertdialog"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Remove this permission rule\?/i),
    ).toBeInTheDocument();

    vi.mocked(listToolPermissions).mockResolvedValueOnce(
      RULES.filter((r) => !(r.tool_name === "Bash" && r.scope === "user")),
    );

    await user.click(screen.getByRole("button", { name: /^Remove rule$/i }));

    await waitFor(() =>
      expect(removeToolPermission).toHaveBeenCalledWith(
        expect.objectContaining({ tool_name: "Bash", scope: "user" }),
        "/work/proj",
      ),
    );
    expect(toast.success).toHaveBeenCalled();
  });

  it("shows a toast and keeps the rule when removal fails", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    vi.mocked(removeToolPermission).mockRejectedValue("disk full");
    const user = userEvent.setup();

    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    const removeButtons = screen.getAllByLabelText("Remove rule");
    await user.click(removeButtons[0]!);

    await user.click(screen.getByRole("button", { name: /^Remove rule$/i }));

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        expect.stringMatching(/Failed to remove rule/i),
        expect.anything(),
      ),
    );
    expect(screen.getByText("Bash")).toBeInTheDocument();
  });

  // ----- Edge cases ----------------------------------------------------

  it("renders the error banner when listToolPermissions rejects (does not crash)", async () => {
    vi.mocked(listToolPermissions).mockRejectedValue("backend exploded");
    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() =>
      expect(
        screen.getByText(/Failed to load rules:/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/backend exploded/)).toBeInTheDocument();
    // The "Loading rules…" spinner is gone — we have an empty list.
    expect(screen.queryByText(/Loading rules/)).not.toBeInTheDocument();
    // Group headings still render (with "No rules in this scope").
    expect(screen.getByText("User-wide")).toBeInTheDocument();
  });

  it("ignores rules with an unknown scope value (filtered out, no crash)", async () => {
    const orphan = {
      tool_name: "Mystery",
      rule_content: null,
      behavior: "allow",
      // Intentionally invalid — must be cast through unknown so the
      // structural type-check accepts the broken shape.
      scope: "from-mars" as unknown as PermissionRule["scope"],
      source_path: "/oops",
    } as PermissionRule;
    vi.mocked(listToolPermissions).mockResolvedValue([orphan, RULES[0]!]);
    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() =>
      expect(screen.getByText("Bash")).toBeInTheDocument(),
    );
    // The rule with the bogus scope drops on the floor; the other
    // rule still renders.
    expect(screen.queryByText("Mystery")).not.toBeInTheDocument();
  });

  it("cancelling the confirm dialog does not call removeToolPermission", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    vi.mocked(removeToolPermission).mockResolvedValue();
    const user = userEvent.setup();

    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    const removeButtons = screen.getAllByLabelText("Remove rule");
    await user.click(removeButtons[0]!);
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /^Cancel$/i }));

    // Wait a tick; the dialog should close and no removal should fire.
    await waitFor(() =>
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument(),
    );
    expect(removeToolPermission).not.toHaveBeenCalled();
    // Rule is still listed.
    expect(screen.getByText("Bash")).toBeInTheDocument();
  });

  it("after a successful remove, refresh() refetches the rule list (mock invoked twice)", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    vi.mocked(removeToolPermission).mockResolvedValue();
    const user = userEvent.setup();

    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());
    // First call was the initial mount.
    expect(listToolPermissions).toHaveBeenCalledTimes(1);

    const removeButtons = screen.getAllByLabelText("Remove rule");
    await user.click(removeButtons[0]!);
    await user.click(screen.getByRole("button", { name: /^Remove rule$/i }));

    // Second call is the post-remove refresh, fired regardless of the
    // optimistic local update so we stay in sync with disk.
    await waitFor(() =>
      expect(listToolPermissions).toHaveBeenCalledTimes(2),
    );
    expect(listToolPermissions).toHaveBeenLastCalledWith("/work/proj");
  });

  it("trash button is reachable by keyboard (N8 fix — focus-visible reveals it)", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    const trash = screen.getAllByLabelText("Remove rule")[0]!;
    // The class set must include the focus-visible reveal so the
    // button isn't permanently hidden to keyboard users. (jsdom can't
    // evaluate the actual CSS, so we assert the Tailwind tokens that
    // drive the visibility.)
    expect(trash.className).toContain("focus-visible:opacity-100");
    expect(trash.className).toContain("group-focus-within:opacity-100");

    // Programmatic focus is the closest jsdom analogue of `userEvent.tab()`
    // for our purposes — it activates `:focus-within` on the parent
    // <li>, which the group-focus-within selector targets.
    trash.focus();
    expect(trash).toHaveFocus();
  });

  it("dialog closes (pendingRemoval cleared) when removal fails", async () => {
    // I2 regression: prior behavior left the dialog open with no
    // inline error feedback, and the user typically tried again and
    // hit a confusing "rule not found" on the second attempt.
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    vi.mocked(removeToolPermission).mockRejectedValue("disk full");
    const user = userEvent.setup();

    render(<PermissionsSection projectRoot="/work/proj" />);
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    await user.click(screen.getAllByLabelText("Remove rule")[0]!);
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Remove rule$/i }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    // Dialog must be gone — that's the I2 fix.
    await waitFor(() =>
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument(),
    );
  });

  it("switching projectRoot mid-dialog closes the dialog (I3 fix)", async () => {
    // I3 regression: a user who opened the confirm dialog and then
    // switched workspaces would otherwise commit the removal against
    // the new project's path with the old rule's `scope`, which
    // resolves to a different file.
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    const user = userEvent.setup();

    const { rerender } = render(
      <PermissionsSection projectRoot="/work/proj" />,
    );
    await waitFor(() => expect(screen.getByText("Bash")).toBeInTheDocument());

    await user.click(screen.getAllByLabelText("Remove rule")[0]!);
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();

    rerender(<PermissionsSection projectRoot="/other/proj" />);

    await waitFor(() =>
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument(),
    );
  });

  it("changing projectRoot triggers a re-fetch with the new root", async () => {
    vi.mocked(listToolPermissions).mockResolvedValue(RULES);
    const { rerender } = render(
      <PermissionsSection projectRoot="/work/proj" />,
    );
    await waitFor(() =>
      expect(listToolPermissions).toHaveBeenCalledWith("/work/proj"),
    );
    expect(listToolPermissions).toHaveBeenCalledTimes(1);

    rerender(<PermissionsSection projectRoot="/other/proj" />);
    await waitFor(() =>
      expect(listToolPermissions).toHaveBeenCalledWith("/other/proj"),
    );
    // Total calls should now be 2 — once for the original root, once
    // for the changed root.
    expect(listToolPermissions).toHaveBeenCalledTimes(2);
  });
});
