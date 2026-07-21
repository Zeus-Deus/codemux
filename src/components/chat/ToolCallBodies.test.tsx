/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { ToolCallItem } from "@/lib/agent-chat/types";

import { ToolCallBody } from "./ToolCallBodies";

afterEach(() => cleanup());

function makeTool(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tool-1",
    seq: 0,
    tool_use_id: "tu-1",
    tool_name: "Bash",
    input: {},
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

describe("BashToolBody", () => {
  it("shows the command and tail of stdout", () => {
    const lines = Array.from({ length: 15 }, (_, i) => `line ${i + 1}`).join(
      "\n",
    );
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Bash",
          input: { command: "ls -la" },
          result_content: lines,
        })}
      />,
    );
    expect(screen.getByText(/ls -la/)).toBeInTheDocument();
    expect(screen.getByText(/line 15/)).toBeInTheDocument();
    expect(screen.getByText(/\+ 5 earlier lines hidden/)).toBeInTheDocument();
    // Lines outside the tail should not be rendered.
    expect(screen.queryByText("line 1")).not.toBeInTheDocument();
  });

  it("shows a non-zero exit code badge", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Bash",
          input: { command: "false" },
          result_content: "command failed\nexit code: 7",
          status: "error",
        })}
      />,
    );
    expect(screen.getByText(/exit 7/)).toBeInTheDocument();
  });

  it("does not show an exit badge for clean success", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Bash",
          input: { command: "true" },
          result_content: "ok",
        })}
      />,
    );
    expect(screen.queryByText(/exit /)).not.toBeInTheDocument();
  });
});

describe("ReadToolBody", () => {
  it("shows file path with line range and a preview of the result", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Read",
          input: { file_path: "src/foo.ts", offset: 1, limit: 50 },
          result_content: Array.from({ length: 50 }, (_, i) => `row${i + 1}`).join(
            "\n",
          ),
        })}
      />,
    );
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    expect(screen.getByText(/L1-50/)).toBeInTheDocument();
    expect(screen.getByText(/Read 50 lines/)).toBeInTheDocument();
    // Only the first 5 lines should be visible.
    expect(screen.getByText(/row1/)).toBeInTheDocument();
    expect(screen.queryByText(/row50/)).not.toBeInTheDocument();
  });

  it("renders only the path when there is no result yet", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Read",
          input: { file_path: "src/foo.ts" },
          status: "running",
          result_content: null,
        })}
      />,
    );
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    expect(screen.queryByText(/Read \d+/)).not.toBeInTheDocument();
  });
});

describe("GrepToolBody", () => {
  it("shows match count, locations, and truncates after 5", () => {
    const result = [
      "src/a.ts:10:foo()",
      "src/a.ts:20:bar()",
      "src/b.ts:5:foo()",
      "src/b.ts:15:foo()",
      "src/c.ts:1:foo()",
      "src/c.ts:2:foo()",
      "src/d.ts:3:foo()",
    ].join("\n");
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Grep",
          input: { pattern: "foo", path: "src" },
          result_content: result,
        })}
      />,
    );
    expect(screen.getByText("foo")).toBeInTheDocument();
    expect(screen.getByText(/in src/)).toBeInTheDocument();
    expect(screen.getByText(/7 matches/)).toBeInTheDocument();
    expect(screen.getByText(/\+ 2 more/)).toBeInTheDocument();
    // 5th match should be visible, 6th should not.
    expect(screen.getByText("src/c.ts:1")).toBeInTheDocument();
    expect(screen.queryByText("src/d.ts:3")).not.toBeInTheDocument();
  });
});

describe("WebFetchToolBody", () => {
  it("renders the URL as a link and shows the page title (first line)", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "WebFetch",
          input: { url: "https://example.com/x" },
          result_content: "Example Domain — title line\nrest of body...",
        })}
      />,
    );
    const link = screen.getByRole("link") as HTMLAnchorElement;
    expect(link.href).toBe("https://example.com/x");
    expect(screen.getByText(/Example Domain/)).toBeInTheDocument();
  });
});

describe("EditToolBody", () => {
  it("renders a diff card computed from the tool input (old_string/new_string)", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Edit",
          input: {
            file_path: "src/foo.ts",
            old_string: "old line",
            new_string: "new line a\nnew line b",
          },
        })}
      />,
    );
    // Filename header.
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    // +N/−N counts come from the computed diff, not the model's prose:
    // one removed line, two added lines.
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
    // The removed and added line text render in the body.
    expect(screen.getByText("old line")).toBeInTheDocument();
    expect(screen.getByText("new line a")).toBeInTheDocument();
    expect(screen.getByText("new line b")).toBeInTheDocument();
  });

  it("renders an all-added diff for a Write (no old text)", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Write",
          input: { file_path: "src/new.ts", content: "export const a = 1;" },
        })}
      />,
    );
    expect(screen.getByText("src/new.ts")).toBeInTheDocument();
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("−0")).toBeInTheDocument();
    expect(screen.getByText("export const a = 1;")).toBeInTheDocument();
  });
});

describe("ToolCallBody fallback", () => {
  it("renders a generic block for unknown tools", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "MysteryTool",
          input: { foo: "bar" },
          result_content: "raw output",
        })}
      />,
    );
    expect(screen.getByText("raw output")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

describe("BashToolBody edge cases", () => {
  it("with empty result_content (still running) shows only the command, no exit badge", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Bash",
          input: { command: "sleep 1" },
          status: "running",
          result_content: null,
        })}
      />,
    );
    expect(screen.getByText(/sleep 1/)).toBeInTheDocument();
    expect(screen.queryByText(/exit/)).not.toBeInTheDocument();
    expect(screen.queryByText(/earlier line/)).not.toBeInTheDocument();
  });

  it("with very large output (1000+ lines) renders only the last 10 + the hidden-count footer", () => {
    const total = 1500;
    const lines = Array.from({ length: total }, (_, i) => `row${i + 1}`).join(
      "\n",
    );
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Bash",
          input: { command: "yes | head -1500" },
          result_content: lines,
        })}
      />,
    );
    // Tail is the last 10 lines — row1500 visible, row1 not.
    expect(screen.getByText(/row1500/)).toBeInTheDocument();
    expect(screen.queryByText(/^row1$/)).not.toBeInTheDocument();
    // Hidden count = total - tail (10).
    expect(
      screen.getByText(new RegExp(`\\+ ${total - 10} earlier line`)),
    ).toBeInTheDocument();
  });
});

describe("ReadToolBody edge cases", () => {
  it("renders no range label when neither offset nor limit is set", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Read",
          input: { file_path: "src/foo.ts" },
          result_content: "line1\nline2",
        })}
      />,
    );
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    // No L1+, no L1-50 — no range string at all.
    expect(screen.queryByText(/^L\d/)).not.toBeInTheDocument();
  });

  it("with empty file_path doesn't render the path block (and doesn't crash)", () => {
    const { container } = render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Read",
          input: { file_path: "" },
          result_content: null,
        })}
      />,
    );
    // No path text and no preview; container renders the wrapper div
    // but no children — stays mounted without throwing.
    expect(container.firstChild).not.toBeNull();
    expect(container.querySelectorAll("pre")).toHaveLength(0);
  });
});

describe("GrepToolBody edge cases", () => {
  it("with zero matches falls back to the raw ToolCallBlock", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Grep",
          input: { pattern: "needle", path: "src" },
          result_content: "Found 0 matches",
        })}
      />,
    );
    // The pattern/path header still renders.
    expect(screen.getByText("needle")).toBeInTheDocument();
    // The "Found ..." raw line is filtered out by parseGrepMatches
    // (lines starting with `Found` are skipped), so we hit the empty
    // branch and render the raw result via ToolCallBlock.
    expect(screen.getByText(/Found 0 matches/)).toBeInTheDocument();
    // No "X match(es)" structured-results pill should render — that
    // wrapper paragraph only appears in the populated branch (it would
    // be the inline-paragraph "<n> match(es)" header next to the <ul>).
    expect(
      screen.queryByText(/^\d+ match(es)?$/),
    ).not.toBeInTheDocument();
  });

  it("with files-with-matches output (just paths, no line:content) treats each line as a path-only match", () => {
    const result = ["src/a.ts", "src/b.ts", "src/c.ts"].join("\n");
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Grep",
          input: { pattern: "foo" },
          result_content: result,
        })}
      />,
    );
    expect(screen.getByText(/3 matches/)).toBeInTheDocument();
    expect(screen.getByText("src/a.ts")).toBeInTheDocument();
    expect(screen.getByText("src/b.ts")).toBeInTheDocument();
    expect(screen.getByText("src/c.ts")).toBeInTheDocument();
  });
});

describe("WebFetchToolBody edge cases", () => {
  it("with a prompt field but no result yet shows the prompt block (no title)", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "WebFetch",
          input: {
            url: "https://example.com/x",
            prompt: "Summarize the page",
          },
          status: "running",
          result_content: null,
        })}
      />,
    );
    const link = screen.getByRole("link") as HTMLAnchorElement;
    expect(link.href).toBe("https://example.com/x");
    expect(screen.getByText(/Summarize the page/)).toBeInTheDocument();
  });
});

describe("EditToolBody edge cases", () => {
  it("with no result_content (still running) shows only the path, no stats", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Edit",
          input: { file_path: "src/foo.ts" },
          status: "running",
          result_content: null,
        })}
      />,
    );
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    // No +/- stats block when the result hasn't landed yet.
    expect(screen.queryByText(/^\+\d/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^−\d/)).not.toBeInTheDocument();
  });

  it("with a result that has no diff markers falls back to the ToolCallBlock", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "Edit",
          input: { file_path: "src/foo.ts" },
          result_content: "File updated successfully.",
        })}
      />,
    );
    // Path renders.
    expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
    // Stats parser returns null for no `^+`/`^-` lines; fall through
    // to the ToolCallBlock with the raw content.
    expect(
      screen.getByText("File updated successfully."),
    ).toBeInTheDocument();
  });
});

describe("WebFetchToolBody URL scheme allowlist", () => {
  it.each([
    ["javascript:alert(1)"],
    ["data:text/html,<script>alert(1)</script>"],
    ["file:///etc/passwd"],
    ["vbscript:msgbox(1)"],
    ["chrome://settings"],
    ["not-even-a-url"],
  ])("renders %s as plain text, not a link", (url) => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "WebFetch",
          input: { url },
          result_content: "",
        })}
      />,
    );
    // The URL string still appears (so the user knows what was
    // attempted) but as a span, not an anchor.
    expect(screen.getByText(url)).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });

  it.each([
    ["http://example.com/x"],
    ["https://example.com/y"],
    ["https://sub.example.com:8443/path?q=1#frag"],
  ])("renders %s as a link with target=_blank rel=noopener noreferrer", (url) => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "WebFetch",
          input: { url },
          result_content: "",
        })}
      />,
    );
    const link = screen.getByRole("link") as HTMLAnchorElement;
    expect(link.href).toBe(new URL(url).href);
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toContain("noopener");
    expect(link.getAttribute("rel")).toContain("noreferrer");
  });
});

describe("GenericToolBody edge cases", () => {
  it("with both null result and null input renders nothing without crashing", () => {
    const { container } = render(
      <ToolCallBody
        item={makeTool({
          tool_name: "MysteryTool",
          input: null as unknown as Record<string, unknown>,
          result_content: null,
        })}
      />,
    );
    // GenericToolBody returns `null` when both result and input are
    // null — the surrounding wrapper from ToolCallBody is also a bare
    // return, so there is no rendered output at all.
    expect(container.firstChild).toBeNull();
  });
});

describe("Tool result images", () => {
  it("renders a valid image block as a thumbnail without dumping its base64 payload", () => {
    render(
      <ToolCallBody
        item={makeTool({
          result_content: [
            {
              type: "image",
              source: {
                type: "base64",
                media_type: "image/png",
                data: "AAAA",
              },
            },
          ],
        })}
      />,
    );

    expect(screen.getByRole("img")).toHaveAttribute(
      "src",
      "data:image/png;base64,AAAA",
    );
    expect(screen.queryByText(/AAAA/)).not.toBeInTheDocument();
  });

  it("keeps a rejected non-image block visible in a specialised tool body", () => {
    render(
      <ToolCallBody
        item={makeTool({
          result_content: [
            {
              type: "image",
              source: {
                type: "base64",
                media_type: "application/pdf",
                data: "document-data",
              },
            },
          ],
        })}
      />,
    );

    expect(screen.getByText(/application\/pdf/)).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });

  it("keeps a rejected unsafe URL visible in the generic fallback", () => {
    render(
      <ToolCallBody
        item={makeTool({
          tool_name: "MysteryTool",
          result_content: [
            {
              type: "image_url",
              image_url: { url: "javascript:alert(1)" },
            },
          ],
        })}
      />,
    );

    expect(screen.getByText(/javascript:alert\(1\)/)).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });
});
