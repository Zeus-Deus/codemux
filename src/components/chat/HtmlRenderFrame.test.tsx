/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ToolCallItem } from "@/lib/agent-chat/types";

const mocks = vi.hoisted(() => ({ openExternalUrl: vi.fn().mockResolvedValue("browser") }));

vi.mock("@/lib/open-url", () => ({ openExternalUrl: mocks.openExternalUrl }));

import { HtmlRenderFrame } from "./HtmlRenderFrame";

afterEach(() => {
  cleanup();
  mocks.openExternalUrl.mockClear();
});

function renderCall(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-page",
    seq: 0,
    tool_use_id: "tu-page",
    tool_name: "mcp__codemux__mcp__codemux__html_render",
    input: { title: "Weekly turns", html: "<p>hi</p>" },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function postLink(frame: HTMLIFrameElement, url: string, source: Window | null = frame.contentWindow) {
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        source,
        data: { jsonrpc: "2.0", method: "ui/open-link", params: { url } },
      }),
    );
  });
}

describe("HtmlRenderFrame", () => {
  it("renders the page in a frame that never shares the app's origin", () => {
    render(<HtmlRenderFrame item={renderCall()} />);
    const frame = screen.getByTitle("Weekly turns");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("srcdoc")).toContain("<p>hi</p>");
    expect(frame.getAttribute("srcdoc")).toContain('<style id="codemux-theme">');
  });

  it("fits the frame to the height the page reports, capped by the agent's height", () => {
    render(<HtmlRenderFrame item={renderCall({ input: { title: "Weekly turns", html: "<p>hi</p>", height: 500 } })} />);
    const frame = screen.getByTitle<HTMLIFrameElement>("Weekly turns");
    const post = (height: number) =>
      act(() => {
        window.dispatchEvent(
          new MessageEvent("message", {
            source: frame.contentWindow,
            data: { jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height } },
          }),
        );
      });
    post(420);
    expect(frame.style.height).toBe("420px");
    post(900);
    expect(frame.style.height).toBe("500px");
  });

  it("ignores size reports from other windows", () => {
    render(<HtmlRenderFrame item={renderCall()} />);
    const frame = screen.getByTitle<HTMLIFrameElement>("Weekly turns");
    const before = frame.style.height;
    act(() => {
      window.dispatchEvent(
        new MessageEvent("message", {
          source: window,
          data: { jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 999 } },
        }),
      );
    });
    expect(frame.style.height).toBe(before);
  });

  it("requires a parent-owned confirmation before opening a link requested by the page", async () => {
    render(<HtmlRenderFrame item={renderCall()} />);
    const frame = screen.getByTitle<HTMLIFrameElement>("Weekly turns");
    frame.focus();
    postLink(frame, "https://example.com/one");

    expect(screen.getByRole("alertdialog")).toHaveTextContent("https://example.com/one");
    expect(mocks.openExternalUrl).not.toHaveBeenCalled();

    // A frame script can send more requests while the dialog is open. It
    // cannot silently replace the address the reader is checking.
    postLink(frame, "https://example.com/two");
    expect(screen.getByRole("alertdialog")).not.toHaveTextContent("https://example.com/two");

    fireEvent.click(screen.getByRole("button", { name: "Open link" }));
    await waitFor(() => expect(mocks.openExternalUrl).toHaveBeenCalledWith("https://example.com/one"));
    expect(mocks.openExternalUrl).toHaveBeenCalledTimes(1);
  });

  it("does not open a link when the reader cancels or the message came from another window", () => {
    render(<HtmlRenderFrame item={renderCall()} />);
    const frame = screen.getByTitle<HTMLIFrameElement>("Weekly turns");
    frame.focus();
    postLink(frame, "https://example.com/foreign", window);
    expect(screen.queryByRole("alertdialog")).toBeNull();

    postLink(frame, "https://example.com/real");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(mocks.openExternalUrl).not.toHaveBeenCalled();
  });

  it("posts new font stacks to an open page without reloading it", async () => {
    render(<HtmlRenderFrame item={renderCall()} />);
    const frame = screen.getByTitle<HTMLIFrameElement>("Weekly turns");
    const srcdoc = frame.getAttribute("srcdoc");
    const posted: unknown[] = [];
    frame.contentWindow!.postMessage = ((message: unknown) => posted.push(message)) as Window["postMessage"];
    await act(async () => {
      document.documentElement.style.setProperty("--font-interface", "Test Sans");
      await Promise.resolve();
    });
    document.documentElement.style.removeProperty("--font-interface");
    expect(frame.getAttribute("srcdoc")).toBe(srcdoc);
    expect(posted).toContainEqual(
      expect.objectContaining({
        params: expect.objectContaining({
          styles: { variables: expect.objectContaining({ "--font-sans": "Test Sans" }) },
        }),
      }),
    );
  });

  it("holds a placeholder while the call is in flight", () => {
    render(<HtmlRenderFrame item={renderCall({ status: "running", input: {} })} />);
    expect(screen.getByText("Building visualization…")).toBeInTheDocument();
    expect(screen.queryByTitle("Weekly turns")).toBeNull();
  });
});
